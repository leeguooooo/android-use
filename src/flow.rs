//! Saved flows: a validated list of steps replayed through the daemon's
//! `/agent/actions` contract, one step per batch, so every step gets its own
//! outcome, timing event and failure capture. The shared control core is not
//! changed; runner-only steps (`pause`, `screenshot`, `mark`) never reach it.
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Longest `pause`. Prefer `wait_for`; games with sparse trees need sleeps.
pub const MAX_PAUSE_MS: u64 = 60_000;
/// Longest flow `wait_for`; the runner splits it into core-sized waits.
pub const MAX_WAIT_MS: u64 = 300_000;
const CORE_WAIT_MS: u64 = 30_000;
const MAX_STEPS: usize = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    #[serde(rename = "type", default = "string_type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}
fn string_type() -> String {
    "string".into()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    ReadOnly,
    Navigation,
    SideEffect,
}

/// Flow v1, Android flavour of the iphone-use format. Closed top-level fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowDoc {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Android package the flow drives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_inputs: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified_on: Vec<Value>,
    pub steps: Vec<Value>,
}

/// One validated step, ready to run.
#[derive(Debug, Clone)]
pub enum Op {
    /// A `phone_control::Action` as JSON (`type` key), sent as a one-step batch.
    Device(Value),
    /// `wait_for` with a flow-sized budget.
    Wait {
        expect: Value,
        timeout_ms: u64,
    },
    Pause(u64),
    Screenshot(Option<String>),
    Mark(String),
}

#[derive(Debug, Clone)]
pub struct Step {
    pub op: Op,
    pub kind: String,
    pub note: Option<String>,
    /// Failure that sent nothing is skipped instead of stopping the flow.
    pub optional: bool,
    /// Observe the screen after this step for up to this long.
    pub settle_ms: Option<u64>,
    /// Human description with input values redacted.
    pub desc: String,
}

const DEVICE_KINDS: &[&str] = &[
    "tap",
    "longpress",
    "swipe",
    "scroll",
    "type",
    "back",
    "home",
    "launch_app",
];
const FLOW_KEYS: &[&str] = &["kind", "note", "optional", "settle_ms", "input"];

fn valid_input_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Registry ids are `category/slug` (or a bare slug) of `[a-z0-9-]` segments.
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<_> = id.split('/').collect();
    (1..=3).contains(&parts.len())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 64
                && !p.starts_with('-')
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn describe(kind: &str, step: &Map<String, Value>) -> String {
    let field = |k: &str| step.get(k).map(|v| format!("{k}={v}"));
    let parts: Vec<String> = match kind {
        "tap" => ["label", "kind", "x", "y"]
            .iter()
            .filter_map(|k| {
                // `kind` here is the element kind, stored as `element_kind` in flows.
                if *k == "kind" {
                    step.get("element_kind").map(|v| format!("kind={v}"))
                } else {
                    field(k)
                }
            })
            .collect(),
        "type" => vec![match step.get("input") {
            Some(Value::String(name)) => format!("input={name}"),
            _ => "text=<literal>".into(),
        }],
        "launch_app" => field("app").into_iter().collect(),
        "wait_for" => field("expect").into_iter().collect(),
        "pause" => field("ms").into_iter().collect(),
        "screenshot" | "mark" => field("name").into_iter().collect(),
        _ => vec![],
    };
    if parts.is_empty() {
        kind.into()
    } else {
        format!("{kind} {}", parts.join(" "))
    }
}

/// Validate a flow and resolve its inputs. With `lenient`, missing inputs are
/// replaced by placeholders and reported in the returned warnings (dry run).
pub fn compile(
    doc: &FlowDoc,
    inputs: &BTreeMap<String, String>,
    lenient: bool,
) -> Result<(Vec<Step>, Vec<String>), String> {
    let mut warnings = vec![];
    if doc.version != 1 {
        return Err("version must be 1".into());
    }
    if doc.name.trim().is_empty() || doc.name.len() > 200 {
        return Err("name must be 1..200 characters".into());
    }
    if doc.steps.is_empty() || doc.steps.len() > MAX_STEPS {
        return Err(format!("a flow has 1..{MAX_STEPS} steps"));
    }
    if let Some(app) = &doc.app {
        let probe = json!({"type":"launch_app","app":app});
        if !serde_json::from_value::<phone_control::Action>(probe)
            .is_ok_and(|a| a.validate().is_ok())
        {
            return Err(format!("app {app:?} is not an Android package name"));
        }
    }
    for name in doc.inputs.keys() {
        if !valid_input_name(name) {
            return Err(format!("input name {name:?} must be [a-z0-9_]"));
        }
    }
    for name in inputs.keys() {
        if !doc.inputs.contains_key(name) {
            return Err(format!(
                "unknown input {name:?}; the flow declares {:?}",
                doc.inputs.keys().collect::<Vec<_>>()
            ));
        }
    }
    let mut used = std::collections::BTreeSet::new();
    let mut steps = vec![];
    for (index, raw) in doc.steps.iter().enumerate() {
        let n = index + 1;
        let bad = |why: String| format!("step {n}: {why}");
        let object = raw
            .as_object()
            .ok_or_else(|| bad("must be an object".into()))?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("missing \"kind\"".into()))?
            .to_string();
        let note = match object.get("note") {
            None => None,
            Some(Value::String(s)) if s.len() <= 1000 => Some(s.clone()),
            _ => return Err(bad("note must be a string up to 1000 bytes".into())),
        };
        let optional = match object.get("optional") {
            None => false,
            Some(Value::Bool(b)) => *b,
            _ => return Err(bad("optional must be a boolean".into())),
        };
        let settle_ms = match object.get("settle_ms") {
            None => None,
            Some(v) => match v.as_u64() {
                Some(ms) if ms <= 10_000 => Some(ms),
                _ => return Err(bad("settle_ms must be 0..10000".into())),
            },
        };
        if object.contains_key("input") && kind != "type" {
            return Err(bad("only a type step may reference an input".into()));
        }
        let rest: Map<String, Value> = object
            .iter()
            .filter(|(k, _)| !FLOW_KEYS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let only = |allowed: &[&str]| -> Result<(), String> {
            match rest.keys().find(|k| !allowed.contains(&k.as_str())) {
                Some(k) => Err(bad(format!("unknown field {k:?} for {kind}"))),
                None => Ok(()),
            }
        };
        let op = match kind.as_str() {
            "pause" => {
                only(&["ms"])?;
                match rest.get("ms").and_then(Value::as_u64) {
                    Some(ms) if (1..=MAX_PAUSE_MS).contains(&ms) => Op::Pause(ms),
                    _ => return Err(bad(format!("pause needs ms in 1..{MAX_PAUSE_MS}"))),
                }
            }
            "screenshot" | "mark" => {
                only(&["name"])?;
                let name = match rest.get("name") {
                    None => None,
                    Some(Value::String(s))
                        if !s.is_empty()
                            && s.len() <= 64
                            && s.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
                    {
                        Some(s.clone())
                    }
                    _ => return Err(bad("name must be 1..64 of [A-Za-z0-9_-]".into())),
                };
                if kind == "mark" {
                    Op::Mark(name.ok_or_else(|| bad("mark needs a name".into()))?)
                } else {
                    Op::Screenshot(name)
                }
            }
            "wait_for" => {
                only(&["expect", "timeout_ms"])?;
                let expect = rest
                    .get("expect")
                    .cloned()
                    .ok_or_else(|| bad("wait_for needs expect".into()))?;
                let timeout_ms = match rest.get("timeout_ms") {
                    None => 5_000,
                    Some(v) => match v.as_u64() {
                        Some(ms) if (1..=MAX_WAIT_MS).contains(&ms) => ms,
                        _ => return Err(bad(format!("timeout_ms must be 1..{MAX_WAIT_MS}"))),
                    },
                };
                // Let the core validate the predicate with a core-sized budget.
                let probe = json!({"type":"wait_for","expect":expect,"timeout_ms":timeout_ms.min(CORE_WAIT_MS)});
                check_action(&probe).map_err(bad)?;
                Op::Wait { expect, timeout_ms }
            }
            k if DEVICE_KINDS.contains(&k) => {
                let mut action = rest.clone();
                // A flow names the element kind `element_kind`; `kind` is the step kind.
                if let Some(element_kind) = action.remove("element_kind") {
                    action.insert("kind".into(), element_kind);
                }
                action.insert("type".into(), json!(k));
                if k == "type" {
                    match object.get("input") {
                        Some(Value::String(name)) => {
                            let def = doc.inputs.get(name).ok_or_else(|| {
                                bad(format!("input {name:?} is not declared in inputs"))
                            })?;
                            if action.contains_key("text") {
                                return Err(bad("use either text or input".into()));
                            }
                            used.insert(name.clone());
                            let value = match inputs.get(name).or(def.default.as_ref()) {
                                Some(v) => v.clone(),
                                None if lenient => {
                                    warnings.push(format!("step {n}: input {name:?} not given; dry run uses a placeholder"));
                                    format!("<{name}>")
                                }
                                None => {
                                    return Err(bad(format!(
                                        "input {name:?} is required (pass --input {name}=VALUE)"
                                    )))
                                }
                            };
                            action.insert("text".into(), json!(value));
                        }
                        Some(_) => return Err(bad("input must be a string".into())),
                        None => {}
                    }
                }
                let action = Value::Object(action);
                check_action(&action).map_err(bad)?;
                Op::Device(action)
            }
            other => {
                return Err(bad(format!(
                    "unknown kind {other:?}; use one of {}, wait_for, pause, screenshot, mark",
                    DEVICE_KINDS.join(", ")
                )))
            }
        };
        let desc = describe(&kind, object);
        steps.push(Step {
            op,
            kind,
            note,
            optional,
            settle_ms,
            desc,
        });
    }
    if let Some(unused) = doc.inputs.keys().find(|k| !used.contains(*k)) {
        return Err(format!("input {unused:?} is declared but no step uses it"));
    }
    if doc.verified_on.is_empty() {
        warnings.push("verified_on is empty: run the flow once and record the device".into());
    }
    Ok((steps, warnings))
}

fn check_action(action: &Value) -> Result<(), String> {
    let parsed: phone_control::Action =
        serde_json::from_value(action.clone()).map_err(|e| format!("invalid action: {e}"))?;
    parsed
        .validate()
        .map_err(|e| format!("{} — {}", e.error, e.message))
}

pub fn parse_inputs(pairs: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for pair in pairs {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| format!("--input {pair:?} must be NAME=VALUE"))?;
        map.insert(k.to_string(), v.to_string());
    }
    Ok(map)
}

// ---------------------------------------------------------------------------
// Device access
// ---------------------------------------------------------------------------

/// What the runner needs from the daemon. `DaemonClient` implements it; tests mock it.
pub trait FlowDevice: Sync {
    fn actions(&self, batch: Value) -> impl Future<Output = Value> + Send;
    fn elements(&self) -> impl Future<Output = Value> + Send;
    fn status(&self) -> impl Future<Output = Value> + Send;
    fn screenshot(&self) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
    /// Read-only host-side check of the phone (attached, authorized, awake, unlocked).
    fn diagnose(&self, serial: &str) -> impl Future<Output = Value> + Send;
}

impl FlowDevice for crate::mcp::DaemonClient {
    fn actions(&self, batch: Value) -> impl Future<Output = Value> + Send {
        let client = self.clone();
        async move { client.request("/agent/actions", Some(batch)).await }
    }
    fn elements(&self) -> impl Future<Output = Value> + Send {
        self.request("/agent/elements", None)
    }
    fn status(&self) -> impl Future<Output = Value> + Send {
        self.request("/agent/status", None)
    }
    fn screenshot(&self) -> impl Future<Output = Result<Vec<u8>, String>> + Send {
        crate::mcp::DaemonClient::screenshot(self)
    }
    fn diagnose(&self, serial: &str) -> impl Future<Output = Value> + Send {
        let serial = serial.to_string();
        async move { crate::device::diagnose(&serial).await }
    }
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

pub fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `2026-10-27T08:15:30.123Z` from epoch milliseconds (UTC, no dependency).
pub fn iso_utc(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

fn stamp(ms: u64) -> String {
    iso_utc(ms).replace([':', '-'], "").replace('.', "-")
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// JSON-lines event sink: `events.jsonl` in the run dir, optionally stderr.
pub struct Events {
    started: Instant,
    run_id: String,
    flow: String,
    file: Option<std::fs::File>,
    echo: bool,
    pub lines: Vec<Value>,
}
impl Events {
    pub fn new(run_id: String, flow: String, path: Option<&Path>, echo: bool) -> Self {
        let file = path.and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        Self {
            started: Instant::now(),
            run_id,
            flow,
            file,
            echo,
            lines: vec![],
        }
    }
    pub fn emit(&mut self, event: &str, fields: Value) -> Value {
        use std::io::Write;
        let ts = epoch_ms();
        let mut line = json!({"ts_ms":ts,"ts":iso_utc(ts),"t_ms":self.started.elapsed().as_millis() as u64,
            "event":event,"run_id":self.run_id,"flow":self.flow});
        if let (Some(target), Some(extra)) = (line.as_object_mut(), fields.as_object()) {
            for (k, v) in extra {
                target.insert(k.clone(), v.clone());
            }
        }
        let text = line.to_string();
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{text}");
        }
        if self.echo {
            eprintln!("{text}");
        }
        self.lines.push(line.clone());
        line
    }
}

// ---------------------------------------------------------------------------
// Optional host-side capture: screenrecord, logcat, clock offset
// ---------------------------------------------------------------------------

/// `adb` argument lists, kept separate so they are testable without a device.
pub fn screenrecord_args(serial: &str, remote: &str, limit_s: u32) -> Vec<String> {
    [
        "-s",
        serial,
        "shell",
        "screenrecord",
        "--time-limit",
        &limit_s.to_string(),
        remote,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
pub fn logcat_args(serial: &str, since_ms: u64) -> Vec<String> {
    // `-T sssss.mmm` starts at an epoch time; `-v epoch` prints epoch stamps
    // that line up with the events' ts_ms.
    let since = format!("{}.{:03}", since_ms / 1000, since_ms % 1000);
    ["-s", serial, "logcat", "-v", "epoch", "-T", &since]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[derive(Default)]
pub struct CaptureOptions {
    pub record: bool,
    pub logcat: bool,
}

struct Capture {
    serial: String,
    record: Option<(tokio::process::Child, String)>,
    logcat: Option<tokio::process::Child>,
}

fn adb() -> tokio::process::Command {
    let program = std::env::var_os("ADB").unwrap_or_else(|| "adb".into());
    let mut command = tokio::process::Command::new(program);
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    command
}

impl Capture {
    async fn start(
        options: &CaptureOptions,
        serial: &str,
        dir: &Path,
        run_id: &str,
        events: &mut Events,
    ) -> Result<Self, String> {
        let mut capture = Capture {
            serial: serial.into(),
            record: None,
            logcat: None,
        };
        // Device clock vs host clock: on an emulator they normally agree.
        let before = epoch_ms();
        if let Ok(out) = adb()
            .args(["-s", serial, "shell", "date", "+%s%3N"])
            .output()
            .await
        {
            let after = epoch_ms();
            if let Ok(device) = String::from_utf8_lossy(&out.stdout).trim().parse::<i64>() {
                let host = ((before + after) / 2) as i64;
                events.emit(
                    "clock",
                    json!({"device_epoch_ms":device,"host_epoch_ms":host,"offset_ms":device-host,"rtt_ms":after-before}),
                );
            }
        }
        if options.logcat {
            let file = std::fs::File::create(dir.join("logcat.txt")).map_err(|e| e.to_string())?;
            let child = adb()
                .args(logcat_args(serial, before))
                .stdout(file)
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("adb logcat failed to start: {e}"))?;
            events.emit("logcat_start", json!({"path":dir.join("logcat.txt")}));
            capture.logcat = Some(child);
        }
        if options.record {
            let remote = format!("/sdcard/android-use-{run_id}.mp4");
            let child = adb()
                .args(screenrecord_args(serial, &remote, 180))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("adb screenrecord failed to start: {e}"))?;
            events.emit(
                "record_start",
                json!({"remote":remote,"note":"screenrecord stops by itself after 180 s"}),
            );
            capture.record = Some((child, remote));
        }
        Ok(capture)
    }
    async fn stop(mut self, dir: &Path, events: &mut Events) -> Value {
        let mut out = json!({});
        if let Some((mut child, remote)) = self.record.take() {
            let _ = adb()
                .args(["-s", &self.serial, "shell", "pkill", "-INT", "screenrecord"])
                .status()
                .await;
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            // The encoder finalizes the file after SIGINT.
            tokio::time::sleep(Duration::from_millis(800)).await;
            let local = dir.join("screen.mp4");
            let pulled = adb()
                .arg("-s")
                .arg(&self.serial)
                .arg("pull")
                .arg(&remote)
                .arg(&local)
                .output()
                .await
                .is_ok_and(|o| o.status.success());
            let _ = adb()
                .args(["-s", &self.serial, "shell", "rm", "-f", &remote])
                .status()
                .await;
            events.emit("record_end", json!({"path":local,"pulled":pulled}));
            out["recording"] = if pulled { json!(local) } else { Value::Null };
        }
        if let Some(mut child) = self.logcat.take() {
            let _ = child.kill().await;
            events.emit("logcat_end", json!({}));
            out["logcat"] = json!(dir.join("logcat.txt"));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

pub struct RunOptions {
    pub inputs: BTreeMap<String, String>,
    /// 1-based inclusive range of steps to run.
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub dry_run: bool,
    pub confirm: bool,
    /// Multiplies every wait_for budget (slow emulators).
    pub timeout_scale: f64,
    /// Directory for events, captures and the result; None picks one under data_dir.
    pub run_dir: Option<PathBuf>,
    pub echo_events: bool,
    pub capture: CaptureOptions,
    /// Capture a screenshot after every step (heavy; for debugging).
    pub screenshot_each: bool,
    /// Check the phone (USB state, awake, unlocked) before step 1.
    pub preflight: bool,
}
impl Default for RunOptions {
    fn default() -> Self {
        Self {
            inputs: BTreeMap::new(),
            from: None,
            to: None,
            dry_run: false,
            confirm: false,
            timeout_scale: 1.0,
            run_dir: None,
            echo_events: false,
            capture: CaptureOptions::default(),
            screenshot_each: false,
            preflight: true,
        }
    }
}

pub fn runs_root() -> anyhow::Result<PathBuf> {
    Ok(crate::backend::data_dir()?.join("flow-runs"))
}
fn run_key(flow_id: &str) -> String {
    flow_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

struct StepResult {
    ok: bool,
    applied: u64,
    failure: Value,
    observation: Option<Value>,
}

async fn run_step<D: FlowDevice>(
    device: &D,
    step: &Step,
    options: &RunOptions,
    dir: Option<&Path>,
    number: usize,
) -> StepResult {
    let done = |applied: u64, observation: Option<Value>| StepResult {
        ok: true,
        applied,
        failure: Value::Null,
        observation,
    };
    let fail = |failure: Value| StepResult {
        ok: false,
        applied: failure["applied_actions"].as_u64().unwrap_or(0),
        failure,
        observation: None,
    };
    match &step.op {
        Op::Pause(ms) => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
            done(0, None)
        }
        Op::Mark(_) => done(0, None),
        Op::Screenshot(name) => match device.screenshot().await {
            Ok(bytes) => {
                let mut observation = json!({"bytes":bytes.len()});
                if let Some(dir) = dir {
                    let file = dir.join(format!(
                        "step-{number:03}-{}.png",
                        name.as_deref().unwrap_or("screenshot")
                    ));
                    if let Err(e) = std::fs::write(&file, bytes) {
                        return fail(
                            json!({"ok":false,"error":"screenshot_write_failed","message":e.to_string(),"outcome":"not_sent","retry_safe":true}),
                        );
                    }
                    observation["path"] = json!(file);
                }
                done(0, Some(observation))
            }
            Err(message) => fail(
                json!({"ok":false,"error":"screenshot_failed","message":message,"outcome":"not_sent","retry_safe":true}),
            ),
        },
        Op::Wait { expect, timeout_ms } => {
            let budget = ((*timeout_ms as f64) * options.timeout_scale.clamp(0.1, 20.0)) as u64;
            let deadline = Instant::now() + Duration::from_millis(budget.max(1));
            loop {
                let remaining = deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64;
                let slice = remaining.clamp(1, CORE_WAIT_MS);
                let result = device
                    .actions(json!({"steps":[{"type":"wait_for","expect":expect,"timeout_ms":slice}],"observe":false}))
                    .await;
                if result["ok"] == true {
                    return done(0, None);
                }
                if result["error"] == "condition_timeout" && Instant::now() < deadline {
                    continue;
                }
                let mut result = result;
                if result["error"] == "condition_timeout" {
                    result["message"] = json!(format!(
                        "Expected {expect} was not observed within {budget} ms"
                    ));
                }
                return fail(result);
            }
        }
        Op::Device(action) => {
            let observe = step.settle_ms.is_some();
            let result = device
                .actions(json!({"steps":[action],"observe":observe,"settle_ms":step.settle_ms.unwrap_or(0)}))
                .await;
            if result["ok"] == true {
                let applied = result["applied_actions"].as_u64().unwrap_or(0);
                let observation = result.get("observation").map(
                    |o| json!({"app":o["app"],"snapshot":o["snapshot"],"settle":result["settle"]}),
                );
                done(applied, observation)
            } else {
                fail(result)
            }
        }
    }
}

/// Run (or with `dry_run` only plan) a flow. Never retries a step.
pub async fn run<D: FlowDevice>(
    device: &D,
    flow_id: &str,
    doc: &FlowDoc,
    options: RunOptions,
) -> Value {
    let (steps, warnings) = match compile(doc, &options.inputs, options.dry_run) {
        Ok(v) => v,
        Err(message) => {
            return json!({"ok":false,"error":"invalid_flow","message":message,"outcome":"not_sent","retry_safe":true,"flow":flow_id})
        }
    };
    let total = steps.len();
    let from = options.from.unwrap_or(1);
    let to = options.to.unwrap_or(total);
    if from < 1 || to > total || from > to {
        return json!({"ok":false,"error":"invalid_range","message":format!("--from/--to must satisfy 1 <= from <= to <= {total}"),"outcome":"not_sent","retry_safe":true,"flow":flow_id});
    }
    let plan: Vec<Value> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({"step":i+1,"kind":s.kind,"desc":s.desc,"note":s.note,"optional":s.optional,
                "selected":(from..=to).contains(&(i+1)),
                "mutates":matches!(s.op, Op::Device(_))})
        })
        .collect();
    if options.dry_run {
        return json!({"ok":true,"dry_run":true,"flow":flow_id,"name":doc.name,"app":doc.app,"risk":doc.risk,
            "steps":total,"from":from,"to":to,"plan":plan,"warnings":warnings,
            "hint":"Dry run: validated and planned; nothing was sent to the device"});
    }
    if doc.risk == Some(Risk::SideEffect) && !options.confirm {
        return json!({"ok":false,"error":"confirm_required","message":"This flow has risk side_effect; rerun with --confirm / confirm=true after the user approved its target and inputs","outcome":"not_sent","retry_safe":true,"flow":flow_id});
    }
    let started_ms = epoch_ms();
    let run_id = format!("{}-{}", stamp(started_ms), std::process::id());
    let dir = match options.run_dir.clone() {
        Some(dir) => Some(dir),
        None => runs_root()
            .ok()
            .map(|root| root.join(run_key(flow_id)).join(&run_id)),
    };
    if let Some(dir) = &dir {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut events = Events::new(
        run_id.clone(),
        flow_id.into(),
        dir.as_ref().map(|d| d.join("events.jsonl")).as_deref(),
        options.echo_events,
    );
    let status = device.status().await;
    events.emit(
        "flow_start",
        json!({"name":doc.name,"app":doc.app,"steps":total,"from":from,"to":to,
            "device":status["device"],"foreground":status["app"],"drivable":status["drivable"],"dir":dir}),
    );
    // Host-side diagnosis needs an ADB serial; the accessibility backend has none.
    let serial = match (status["backend"].as_str(), status["device"].as_str()) {
        (Some("accessibility"), _) | (_, None) => None,
        (_, Some(serial)) => Some(serial.to_string()),
    };
    if options.preflight {
        let health = match &serial {
            Some(serial) => device.diagnose(serial).await,
            None => json!({"ok":status["drivable"]==true}),
        };
        events.emit("preflight", json!({"status":status,"device":health}));
        if status["drivable"] != true || health["ok"] == false {
            let problem = health["problem"]
                .as_str()
                .or(status["error"].as_str())
                .unwrap_or("not_drivable");
            let hint = health["hint"]
                .as_str()
                .or(status["message"].as_str())
                .unwrap_or(
                    "Run `android-use doctor`, then `android-use connect` once the phone is ready",
                );
            let mut result = json!({"ok":false,"error":format!("device_{}", problem.replace(' ', "_")),
                "message":format!("preflight: device not ready ({problem}); nothing was sent"),
                "hint":hint,"device":health,"status":status,"outcome":"not_sent","retry_safe":true,
                "flow":flow_id,"run_id":run_id,"applied_actions":0});
            events.emit(
                "flow_end",
                json!({"ok":false,"error":result["error"],"applied_actions":0}),
            );
            if let Some(dir) = &dir {
                result["dir"] = json!(dir);
                let _ = std::fs::write(
                    dir.join("result.json"),
                    serde_json::to_vec_pretty(&result).unwrap_or_default(),
                );
            }
            return result;
        }
    }
    let mut capture = None;
    if options.capture.record || options.capture.logcat {
        let serial = status["device"].as_str().unwrap_or_default().to_string();
        let started = match (&dir, serial.is_empty()) {
            (Some(dir), false) => {
                Capture::start(&options.capture, &serial, dir, &run_id, &mut events).await
            }
            _ => Err("recording needs a run directory and an ADB serial from status".into()),
        };
        match started {
            Ok(c) => capture = Some(c),
            Err(message) => {
                events.emit("capture_error", json!({"message":message}));
            }
        }
    }
    let mut applied_total = 0u64;
    let mut timings = vec![];
    let mut failure: Option<Value> = None;
    for (i, step) in steps.iter().enumerate() {
        let number = i + 1;
        if !(from..=to).contains(&number) {
            continue;
        }
        let begin = events.emit(
            "step_start",
            json!({"step":number,"index":i,"kind":step.kind,"desc":step.desc,"note":step.note}),
        );
        let clock = Instant::now();
        let result = run_step(device, step, &options, dir.as_deref(), number).await;
        let elapsed = clock.elapsed().as_millis() as u64;
        applied_total += result.applied;
        if let Op::Mark(name) = &step.op {
            events.emit("mark", json!({"step":number,"name":name}));
        }
        if result.ok {
            let end = events.emit(
                "step_end",
                json!({"step":number,"index":i,"kind":step.kind,"ok":true,"elapsed_ms":elapsed,
                    "applied_actions":result.applied,"observation":result.observation}),
            );
            timings.push(json!({"step":number,"kind":step.kind,"desc":step.desc,
                "start_ms":begin["ts_ms"],"end_ms":end["ts_ms"],"elapsed_ms":elapsed}));
            if options.screenshot_each {
                if let (Some(dir), Ok(bytes)) = (&dir, device.screenshot().await) {
                    let _ = std::fs::write(dir.join(format!("after-{number:03}.png")), bytes);
                }
            }
            continue;
        }
        let f = &result.failure;
        let sent_nothing = f["outcome"] == "not_sent" && result.applied == 0;
        if step.optional && sent_nothing {
            events.emit(
                "step_skipped",
                json!({"step":number,"index":i,"kind":step.kind,"elapsed_ms":elapsed,"error":f["error"],"message":f["message"]}),
            );
            timings.push(
                json!({"step":number,"kind":step.kind,"desc":step.desc,"skipped":true,
                "start_ms":begin["ts_ms"],"elapsed_ms":elapsed}),
            );
            continue;
        }
        events.emit(
            "step_end",
            json!({"step":number,"index":i,"kind":step.kind,"ok":false,"elapsed_ms":elapsed,
                "error":f["error"],"message":f["message"],"outcome":f["outcome"],"retry_safe":f["retry_safe"]}),
        );
        // Ask the host about the phone first: after a USB disconnect, reading
        // the screen would only wait for timeouts.
        let error = f["error"].as_str().unwrap_or("");
        let health = match &serial {
            Some(serial) if crate::device::device_suspect(error) => {
                Some(device.diagnose(serial).await)
            }
            _ => None,
        };
        let lost = health.as_ref().is_some_and(|h| {
            matches!(
                h["problem"].as_str(),
                Some("missing" | "offline" | "unauthorized" | "no permissions" | "adb_unavailable")
            )
        });
        if let Some(h) = &health {
            events.emit(
                if lost { "device_lost" } else { "device_check" },
                json!({"step":number,"device":h}),
            );
        }
        // Capture what the screen looked like when the step failed.
        let mut artifacts = json!({});
        let elements = if lost {
            json!({})
        } else {
            device.elements().await
        };
        if let (Some(dir), false) = (&dir, lost) {
            let path = dir.join("failure-elements.json");
            if std::fs::write(
                &path,
                serde_json::to_vec_pretty(&elements).unwrap_or_default(),
            )
            .is_ok()
            {
                artifacts["elements"] = json!(path);
            }
            if let Ok(bytes) = device.screenshot().await {
                let path = dir.join("failure.png");
                if std::fs::write(&path, bytes).is_ok() {
                    artifacts["screenshot"] = json!(path);
                }
            }
        }
        let visible: Vec<Value> = elements["elements"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|r| {
                        r["visible"] == true && r["label"].as_str().is_some_and(|l| !l.is_empty())
                    })
                    .take(25)
                    .map(|r| {
                        json!(format!(
                            "{} {:?}",
                            r["kind"].as_str().unwrap_or(""),
                            r["label"].as_str().unwrap_or("")
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let unknown = f["outcome"] == "unknown";
        let mutates = matches!(step.op, Op::Device(_));
        let hint = if lost {
            let h = health.as_ref().unwrap();
            format!("The phone became {} during step {number}: {}. Then run `android-use connect` (the on-device runner does not survive a disconnect) and read the screen. {}",
                h["problem"].as_str().unwrap_or("unavailable"), h["hint"].as_str().unwrap_or(""),
                if mutates && (unknown || f["outcome"] != "not_sent") {
                    format!("Step {number} may or may not have acted; never replay it blindly — resume with --from {number} or --from {} depending on what the screen shows.", number + 1)
                } else {
                    format!("Resume with --from {number} once the screen matches what step {number} expects.")
                })
        } else if let Some(problem) = health.as_ref().and_then(|h| h["problem"].as_str()) {
            format!("The phone is {problem}: {}. Then read the screen and resume with --from {number} if it matches what step {number} expects.",
                health.as_ref().unwrap()["hint"].as_str().unwrap_or(""))
        } else if unknown {
            format!("Step {number} may have acted (outcome unknown). Read the screen before anything else; resume with --from {} only after confirming the step took effect, never replay it blindly.", number + 1)
        } else if applied_total == 0 {
            format!("Nothing was applied; after fixing the cause you may resume with --from {number} (or rerun).")
        } else {
            format!("Earlier steps already acted. Read the screen, then resume with --from {number} only if the screen matches what step {number} expects.")
        };
        failure = Some(json!({
            "ok":false,
            "error":f["error"],
            "message":match health.as_ref().and_then(|h| h["problem"].as_str()) {
                Some(problem) => format!("step {number}/{total} ({}) failed: device {problem} ({} — {})", step.desc, f["error"].as_str().unwrap_or("error"), f["message"].as_str().unwrap_or("")),
                None => format!("step {number}/{total} ({}) failed: {} — {}", step.desc, f["error"].as_str().unwrap_or("error"), f["message"].as_str().unwrap_or("")),
            },
            "device":health,
            "outcome":if applied_total>0 || unknown {"partial"} else {"not_sent"},
            "retry_safe": applied_total==0 && f["retry_safe"]==true,
            "failed_step":number,
            "step_kind":step.kind,
            "step_note":step.note,
            "step_error":f,
            "screen":{"app":elements["app"],"snapshot":elements["snapshot"],"visible_labels":visible},
            "artifacts":artifacts,
            "hint":hint,
        }));
        break;
    }
    let capture_out = match capture {
        Some(c) => {
            c.stop(dir.as_deref().unwrap_or(Path::new(".")), &mut events)
                .await
        }
        None => json!({}),
    };
    let ended_ms = epoch_ms();
    let mut result = failure.unwrap_or_else(|| {
        json!({"ok":true,"outcome":if applied_total>0 {"applied"} else {"not_sent"},"retry_safe":applied_total==0})
    });
    result["flow"] = json!(flow_id);
    result["run_id"] = json!(run_id);
    result["steps"] = json!(total);
    result["from"] = json!(from);
    result["to"] = json!(to);
    result["applied_actions"] = json!(applied_total);
    result["started_ms"] = json!(started_ms);
    result["ended_ms"] = json!(ended_ms);
    result["elapsed_ms"] = json!(ended_ms.saturating_sub(started_ms));
    result["timings"] = json!(timings);
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
    if let Some(obj) = capture_out.as_object() {
        for (k, v) in obj {
            result[k.as_str()] = v.clone();
        }
    }
    events.emit(
        "flow_end",
        json!({"ok":result["ok"],"failed_step":result.get("failed_step"),"applied_actions":applied_total,"elapsed_ms":result["elapsed_ms"]}),
    );
    if let Some(dir) = &dir {
        result["dir"] = json!(dir);
        result["events"] = json!(dir.join("events.jsonl"));
        let _ = std::fs::write(
            dir.join("result.json"),
            serde_json::to_vec_pretty(&result).unwrap_or_default(),
        );
    }
    result
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

pub fn registry_dir() -> anyhow::Result<PathBuf> {
    match std::env::var_os("ANDROID_USE_FLOWS_DIR") {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(crate::backend::data_dir()?.join("flows")),
    }
}

pub fn load(path: &Path) -> Result<FlowDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A registry id or a file path. Returns (id, path).
pub fn resolve(registry: &Path, target: &str) -> Result<(String, PathBuf), String> {
    let as_path = PathBuf::from(target);
    if target.ends_with(".json") || as_path.is_file() {
        if !as_path.is_file() {
            return Err(format!("flow file {target} not found"));
        }
        let id = as_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        return Ok((format!("file:{id}"), as_path));
    }
    if !valid_id(target) {
        return Err(format!(
            "{target:?} is neither a .json file nor a registry id like dice/open-match"
        ));
    }
    let path = registry.join(format!("{target}.json"));
    if !path.is_file() {
        return Err(format!(
            "no flow {target} in {}; `android-use flow list` shows what is installed",
            registry.display()
        ));
    }
    Ok((target.into(), path))
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with('.') || name == "reports" {
            continue;
        }
        if path.is_dir() {
            walk(&path, root, out);
        } else if name.ends_with(".json") {
            if let Ok(rel) = path.strip_prefix(root) {
                let id = rel.with_extension("").to_string_lossy().replace('\\', "/");
                out.push((id, path));
            }
        }
    }
}

fn last_run(runs: Option<&Path>, id: &str) -> Option<PathBuf> {
    let dir = runs?.join(run_key(id));
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    entries.pop()
}

fn summary(id: &str, path: &Path, doc: &FlowDoc, runs: Option<&Path>) -> Value {
    let last = last_run(runs, id).and_then(|d| {
        let r: Value = serde_json::from_slice(&std::fs::read(d.join("result.json")).ok()?).ok()?;
        Some(json!({"run_id":r["run_id"],"ok":r["ok"],"failed_step":r.get("failed_step"),"elapsed_ms":r["elapsed_ms"],"dir":d}))
    });
    json!({"id":id,"name":doc.name,"app":doc.app,"category":doc.category,"risk":doc.risk,
        "verified":!doc.verified_on.is_empty(),"inputs":doc.inputs.keys().collect::<Vec<_>>(),
        "steps":doc.steps.len(),"path":path,"last_run":last})
}

pub fn list(
    registry: &Path,
    runs: Option<&Path>,
    app: Option<&str>,
    category: Option<&str>,
) -> Value {
    let mut found = vec![];
    walk(registry, registry, &mut found);
    let mut rows = vec![];
    let mut broken = vec![];
    for (id, path) in found {
        match load(&path) {
            Ok(doc) => {
                if app.is_some_and(|a| doc.app.as_deref() != Some(a))
                    || category.is_some_and(|c| {
                        doc.category.as_deref() != Some(c) && !id.starts_with(&format!("{c}/"))
                    })
                {
                    continue;
                }
                rows.push(summary(&id, &path, &doc, runs));
            }
            Err(e) => broken.push(json!({"id":id,"error":e})),
        }
    }
    json!({"ok":true,"registry":registry,"flows":rows,"invalid":broken})
}

pub fn info(registry: &Path, runs: Option<&Path>, target: &str) -> Value {
    let (id, path) = match resolve(registry, target) {
        Ok(v) => v,
        Err(message) => return json!({"ok":false,"error":"flow_not_found","message":message}),
    };
    match load(&path) {
        Ok(doc) => {
            let mut out = summary(&id, &path, &doc, runs);
            out["ok"] = json!(true);
            out["valid"] = match compile(&doc, &BTreeMap::new(), true) {
                Ok((_, warnings)) => json!({"ok":true,"warnings":warnings}),
                Err(e) => json!({"ok":false,"error":e}),
            };
            out["flow"] = serde_json::to_value(&doc).unwrap_or_default();
            out
        }
        Err(message) => json!({"ok":false,"error":"invalid_flow","message":message}),
    }
}

/// Copy a validated flow into the registry. Writes only with `confirm`.
pub fn publish(
    registry: &Path,
    source: &Path,
    id: &str,
    confirm: bool,
    allow_unverified: bool,
) -> Value {
    if !valid_id(id) {
        return json!({"ok":false,"error":"invalid_id","message":"Use an id like dice/open-match ([a-z0-9-] segments)"});
    }
    let doc = match load(source) {
        Ok(d) => d,
        Err(message) => return json!({"ok":false,"error":"invalid_flow","message":message}),
    };
    if let Err(message) = compile(&doc, &BTreeMap::new(), true) {
        return json!({"ok":false,"error":"invalid_flow","message":message});
    }
    if doc.verified_on.is_empty() && !allow_unverified {
        return json!({"ok":false,"error":"unverified","message":"Run the flow once, add verified_on, or pass --allow-unverified"});
    }
    let target = registry.join(format!("{id}.json"));
    let replaces = target.exists();
    if !confirm {
        return json!({"ok":false,"error":"confirm_required","message":"Publishing writes the registry; rerun with --confirm / confirm=true after the user agreed","would_write":target,"replaces":replaces});
    }
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return json!({"ok":false,"error":"write_failed","message":e.to_string()});
        }
    }
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n";
    match std::fs::write(&target, text) {
        Ok(()) => json!({"ok":true,"id":id,"path":target,"replaced":replaces}),
        Err(e) => json!({"ok":false,"error":"write_failed","message":e.to_string()}),
    }
}

/// Show the latest run of a flow; with a note and `confirm`, file a report
/// under `<registry>/reports/`.
pub fn report(
    registry: &Path,
    runs: Option<&Path>,
    target: &str,
    note: Option<&str>,
    confirm: bool,
) -> Value {
    let id = match resolve(registry, target) {
        Ok((id, _)) => id,
        Err(_) if valid_id(target) => target.to_string(),
        Err(message) => return json!({"ok":false,"error":"flow_not_found","message":message}),
    };
    let Some(dir) = last_run(runs, &id) else {
        return json!({"ok":false,"error":"no_runs","message":format!("{id} has no recorded runs")});
    };
    let result: Value = std::fs::read(dir.join("result.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let failed: Vec<Value> = std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "step_end" && e["ok"] == false)
        .collect();
    let mut out = json!({"ok":true,"id":id,"run_dir":dir,"result":{
        "ok":result["ok"],"message":result["message"],"failed_step":result.get("failed_step"),
        "retry_safe":result["retry_safe"],"artifacts":result.get("artifacts"),"elapsed_ms":result["elapsed_ms"]},
        "failed_events":failed});
    if let Some(note) = note {
        if !confirm {
            out["ok"] = json!(false);
            out["error"] = json!("confirm_required");
            out["message"] = json!("Filing a report writes the registry; rerun with --confirm");
            return out;
        }
        let reports = registry.join("reports");
        let _ = std::fs::create_dir_all(&reports);
        let path = reports.join(format!("{}.jsonl", run_key(&id)));
        let line = json!({"ts":iso_utc(epoch_ms()),"id":id,"note":note,"run_dir":dir,"result":out["result"]});
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{line}");
                out["filed"] = json!(path);
            }
            Err(e) => return json!({"ok":false,"error":"write_failed","message":e.to_string()}),
        }
    }
    out
}

/// Refresh the registry: `git pull --ff-only` when it is a checkout, a clone of
/// `ANDROID_USE_FLOWS_REPO` when it is missing, otherwise just re-index.
pub async fn update(registry: &Path) -> Value {
    let repo = std::env::var("ANDROID_USE_FLOWS_REPO").ok();
    let git = |args: Vec<String>| async move {
        tokio::process::Command::new("git")
            .args(args)
            .output()
            .await
            .map(|o| {
                (
                    o.status.success(),
                    String::from_utf8_lossy(&o.stderr).trim().to_string(),
                )
            })
            .map_err(|e| e.to_string())
    };
    let action = if registry.join(".git").exists() {
        let r = git(vec![
            "-C".into(),
            registry.display().to_string(),
            "pull".into(),
            "--ff-only".into(),
        ])
        .await;
        Some(("pull", r))
    } else if let (Some(repo), false) = (&repo, registry.exists()) {
        let r = git(vec![
            "clone".into(),
            "--depth=1".into(),
            repo.clone(),
            registry.display().to_string(),
        ])
        .await;
        Some(("clone", r))
    } else {
        None
    };
    let mut out = list(registry, None, None, None);
    let count = out["flows"].as_array().map_or(0, Vec::len);
    out["flows"] = json!(count);
    match action {
        Some((name, Ok((true, _)))) => out["synced"] = json!(name),
        Some((name, Ok((false, err)))) | Some((name, Err(err))) => {
            out["ok"] = json!(false);
            out["error"] = json!("sync_failed");
            out["message"] = json!(format!("git {name} failed: {err}"));
        }
        None => {
            out["synced"] = json!(false);
            out["hint"] = json!("Local registry only (no remote android-use flows registry yet). Make it a git checkout or set ANDROID_USE_FLOWS_REPO to sync.");
        }
    }
    out
}

/// The CLI and MCP view of a draft: write it if asked, validate it.
pub fn finish_draft(response: Value, save_as: Option<&Path>) -> Value {
    if response["ok"] != true {
        return response;
    }
    let mut out = response;
    match serde_json::from_value::<FlowDoc>(out["draft"].clone()) {
        Ok(doc) => {
            out["valid"] = match compile(&doc, &BTreeMap::new(), true) {
                Ok(_) => json!(true),
                Err(e) => json!(e),
            };
            if let Some(path) = save_as {
                let text = serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n";
                match std::fs::write(path, text) {
                    Ok(()) => out["saved"] = json!(path),
                    Err(e) => {
                        return json!({"ok":false,"error":"write_failed","message":e.to_string()})
                    }
                }
            }
        }
        Err(e) => out["valid"] = json!(e.to_string()),
    }
    out
}
