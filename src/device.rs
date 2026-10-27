//! Read-only USB device diagnosis: is the phone attached, authorized, awake
//! and unlocked? Used by `doctor`, flow preflight and flow failure reports so
//! a run that stops on a physical problem says which one and what to do.
use serde_json::{json, Value};
use std::time::Duration;

fn adb() -> tokio::process::Command {
    let program = std::env::var_os("ANDROID_USE_ADB").unwrap_or_else(|| "adb".into());
    let mut command = tokio::process::Command::new(program);
    // Same choice as the backend: the native macOS USB stack was unreliable.
    if cfg!(target_os = "macos") && std::env::var_os("ADB_LIBUSB").is_none() {
        command.env("ADB_LIBUSB", "1");
    }
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    command
}

async fn run(args: &[&str]) -> Option<(bool, String, String)> {
    let output = tokio::time::timeout(Duration::from_secs(8), adb().args(args).output())
        .await
        .ok()?
        .ok()?;
    Some((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

pub fn hint_for(state: &str) -> &'static str {
    match state {
        "device" => "Ready",
        "unauthorized" => "Unlock the phone and accept the 'Allow USB debugging?' prompt (tick 'Always allow from this computer'); if no prompt appears, replug the cable or revoke USB debugging authorizations in Developer options and replug",
        "offline" => "ADB sees the phone but cannot talk to it: replug the cable, try another port/cable, or run `adb kill-server` and retry",
        "no permissions" => "The host cannot open the USB device: check udev rules (Linux) or replug",
        "missing" => "The phone is not attached: check the cable (data, not charge-only), unlock the phone, select 'File transfer' or keep USB debugging on, then `android-use doctor`",
        "recovery" | "sideload" | "bootloader" | "rescue" => "The phone is not booted into Android",
        _ => "Unexpected ADB state; replug the phone and run `android-use doctor`",
    }
}

/// Parse `adb devices -l`. Keeps every state so problems are visible.
pub fn parse_devices(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|line| !line.starts_with("List of devices") && !line.starts_with('*'))
        .filter_map(|line| {
            let (serial, rest) = line.trim().split_once(char::is_whitespace)?;
            let rest = rest.trim();
            // "no permissions (...)" is the only multi-word state.
            let state = if rest.starts_with("no permissions") {
                "no permissions"
            } else {
                rest.split_whitespace().next()?
            };
            let field = |key: &str| {
                rest.split_whitespace()
                    .find_map(|f| f.strip_prefix(&format!("{key}:")).map(str::to_string))
            };
            let transport = if serial.starts_with("emulator-") {
                "emulator"
            } else if field("usb").is_some() {
                "usb"
            } else if serial.contains(':') || serial.contains("._adb-tls-connect") {
                "wifi"
            } else {
                "usb"
            };
            let mut row =
                json!({"serial":serial,"state":state,"transport":transport,"hint":hint_for(state)});
            for key in ["model", "product", "device", "transport_id"] {
                if let Some(value) = field(key) {
                    row[key] = json!(value);
                }
            }
            Some(row)
        })
        .collect()
}

/// `Some(true)` awake, `Some(false)` asleep/dozing, `None` unknown.
pub fn parse_awake(dumpsys_power: &str) -> Option<bool> {
    for line in dumpsys_power.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("mWakefulness=") {
            return Some(v.trim() == "Awake");
        }
    }
    for line in dumpsys_power.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("Display Power: state=") {
            return Some(v.trim() == "ON");
        }
    }
    None
}

/// `Some(true)` when the keyguard shows. Field names differ by Android version.
pub fn parse_locked(dumpsys_window: &str) -> Option<bool> {
    const KEYS: &[&str] = &[
        "mDreamingLockscreen=",
        "mShowingLockscreen=",
        "isKeyguardShowing=",
        "mKeyguardShowing=",
        "keyguardShowing=",
        "KeyguardShowing=",
    ];
    let mut seen = None;
    for token in dumpsys_window.split_whitespace() {
        for key in KEYS {
            if let Some(v) = token.strip_prefix(key) {
                if v.starts_with("true") {
                    return Some(true);
                }
                seen = Some(false);
            }
        }
    }
    seen
}

/// Classify a device into one problem (or none) with a hint.
pub fn verdict(state: &str, awake: Option<bool>, locked: Option<bool>) -> Value {
    let (problem, hint) = if state != "device" {
        (Some(state), hint_for(state))
    } else if awake == Some(false) {
        (Some("asleep"), "The screen is off: wake and unlock the phone (or set Developer options > Stay awake while charging), then rerun")
    } else if locked == Some(true) {
        (
            Some("locked"),
            "The phone shows its lock screen: unlock it, then rerun",
        )
    } else {
        (None, "Ready")
    };
    json!({"ok":problem.is_none(),"problem":problem,"hint":hint})
}

/// Read-only diagnosis of one serial. Never wakes, taps or unlocks.
pub async fn diagnose(serial: &str) -> Value {
    let listed = match run(&["devices", "-l"]).await {
        Some((true, stdout, _)) => parse_devices(&stdout),
        _ => {
            return json!({"serial":serial,"ok":false,"problem":"adb_unavailable",
                "hint":"adb did not answer: install Android platform-tools or run `adb kill-server` and retry"})
        }
    };
    let row = listed.iter().find(|r| r["serial"] == serial).cloned();
    let state = row
        .as_ref()
        .and_then(|r| r["state"].as_str())
        .unwrap_or("missing")
        .to_string();
    let (mut awake, mut locked) = (None, None);
    if state == "device" {
        if let Some((true, out, _)) = run(&["-s", serial, "shell", "dumpsys", "power"]).await {
            awake = parse_awake(&out);
        }
        if let Some((true, out, _)) =
            run(&["-s", serial, "shell", "dumpsys", "window", "policy"]).await
        {
            locked = parse_locked(&out);
        }
        if locked.is_none() {
            if let Some((true, out, _)) = run(&["-s", serial, "shell", "dumpsys", "window"]).await {
                locked = parse_locked(&out);
            }
        }
    }
    let mut out = verdict(&state, awake, locked);
    out["serial"] = json!(serial);
    out["state"] = json!(state);
    out["awake"] = json!(awake);
    out["locked"] = json!(locked);
    if let Some(row) = row {
        out["transport"] = row["transport"].clone();
        out["model"] = row.get("model").cloned().unwrap_or(Value::Null);
    }
    out
}

/// Errors after which the device itself is the first suspect.
pub fn device_suspect(error: &str) -> bool {
    matches!(
        error,
        "daemon_unreachable"
            | "daemon_invalid_response"
            | "device_unavailable"
            | "adb_action_failed"
            | "adb_unavailable"
            | "screen_unavailable"
            | "condition_timeout"
            | "screenshot_failed"
            | "screenshot_unavailable"
            | "action_not_acknowledged"
            | "launch_not_acknowledged"
            | "forward_failed"
    ) || error.starts_with("runner_")
        || error.starts_with("rpc_")
}

/// One verdict from the daemon's status and the host-side diagnosis:
/// `{ok, code, message, next, lost}`. `lost` means the device or its control
/// path went away, so any step in flight may or may not have acted.
pub fn classify(status: &Value, health: Option<&Value>) -> Value {
    let verdict = |code: &str, message: String, next: String, lost: bool| json!({"ok":false,"code":code,"message":message,"next":next,"lost":lost});
    let status_message = status["message"].as_str().unwrap_or("").to_string();
    match status["error"].as_str() {
        Some("daemon_unreachable") => {
            return verdict(
                "daemon_not_running",
                "The android-use daemon is not running (or died with the USB connection)".into(),
                "Run `android-use connect` (add --serial SERIAL when several phones are attached), then `android-use status`".into(),
                true,
            )
        }
        Some("daemon_outdated") => {
            return verdict(
                "daemon_outdated",
                status_message,
                status["next"].as_str().unwrap_or("").into(),
                true,
            )
        }
        _ => {}
    }
    if let Some(problem) = health.and_then(|h| h["problem"].as_str()) {
        let hint = health
            .and_then(|h| h["hint"].as_str())
            .unwrap_or("")
            .to_string();
        let serial = health
            .and_then(|h| h["serial"].as_str())
            .unwrap_or("unknown serial");
        let (code, message, lost) = match problem {
            "missing" => (
                "device_not_attached",
                format!("Phone not attached over ADB ({serial})"),
                true,
            ),
            "unauthorized" => (
                "device_unauthorized",
                format!("Phone has not authorized this computer for USB debugging ({serial})"),
                true,
            ),
            "offline" => (
                "device_offline",
                format!("Phone is attached but offline to ADB ({serial})"),
                true,
            ),
            "no permissions" => (
                "device_no_permissions",
                format!("The host has no permission to open the phone ({serial})"),
                true,
            ),
            "adb_unavailable" => ("adb_unavailable", "adb did not answer".to_string(), true),
            "asleep" => (
                "screen_off",
                format!("Phone screen is off ({serial})"),
                false,
            ),
            "locked" => (
                "device_locked",
                format!("Phone shows its lock screen ({serial})"),
                false,
            ),
            other => {
                return verdict(
                    &format!("device_{}", other.replace(' ', "_")),
                    format!("Phone is {other} ({serial})"),
                    hint,
                    true,
                )
            }
        };
        return verdict(code, message, hint, lost);
    }
    if status["drivable"] != true {
        let error = status["error"].as_str().unwrap_or("not_drivable");
        if error == "control_stopping" {
            return verdict(
                "daemon_stopping",
                "The daemon is shutting down".into(),
                "Wait a few seconds, then run `android-use connect`".into(),
                true,
            );
        }
        return verdict(
            "runner_not_running",
            format!("The phone is attached but the daemon cannot read its screen ({error}: {status_message})"),
            "Restart control: `android-use disconnect` then `android-use connect`; if it repeats, replug the phone and run `android-use doctor`".into(),
            true,
        );
    }
    json!({"ok":true,"code":null,"lost":false})
}

/// Replace typed input values and editable-field contents in an element tree,
/// so failure captures never keep what the flow typed.
pub fn redact(mut tree: Value, secrets: &[(String, String)]) -> Value {
    let scrub = |text: &str| {
        let mut text = text.to_string();
        for (name, value) in secrets {
            if !value.is_empty() {
                text = text.replace(value.as_str(), &format!("<input:{name}>"));
            }
        }
        text
    };
    if let Some(rows) = tree.get_mut("elements").and_then(Value::as_array_mut) {
        for row in rows {
            let editable = row["kind"] == "TextField" || row["password"] == true;
            if let Some(value) = row.get_mut("value") {
                if editable && value.is_string() {
                    *value = json!("<redacted>");
                } else if let Some(text) = value.as_str() {
                    *value = json!(scrub(text));
                }
            }
            if let Some(label) = row.get("label").and_then(Value::as_str) {
                let label = scrub(label);
                row["label"] = json!(label);
            }
        }
    }
    tree
}
