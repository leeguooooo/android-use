//! Host-side helpers that sit beside the vendored control core: action schema
//! docs and error details.
use phone_control::Action;
use serde_json::{json, Value};

/// Human-readable action reference shown by `act --help`.
pub const ACT_HELP: &str = r#"Actions (key is "type"; coordinates are normalized 0 to 1):
  tap        {"type":"tap","x":0.5,"y":0.5}
             {"type":"tap","element":3,"snapshot":"ID"}
             {"type":"tap","label":"Save","kind":"Button"}   (kind is optional)
  longpress  {"type":"longpress","x":0.5,"y":0.5,"duration_ms":500}
  swipe      {"type":"swipe","x1":0.5,"y1":0.8,"x2":0.5,"y2":0.2,"duration_ms":500}
  scroll     {"type":"scroll","x":0.5,"y":0.5,"dx":0,"dy":-0.4}
  type       {"type":"type","text":"hello","clear":false}
  back       {"type":"back"}
  home       {"type":"home"}
  launch_app {"type":"launch_app","app":"com.android.settings"}
  wait_for   {"type":"wait_for","expect":{"app":"...","label":"...","identifier":"...","value":"..."},"timeout_ms":5000}

Batch: {"steps":[ACTION,...],"observe":true,"settle_ms":1500} (at most 32 steps).
duration_ms is 100 to 10000. An expect needs at least one field."#;

/// Machine-readable counterpart of `ACT_HELP`, published in capabilities.
pub fn action_schema() -> Value {
    json!({
        "key":"type",
        "coordinates":"normalized_0_to_1",
        "actions":{
            "tap":{"one_of":[["x","y"],["element","snapshot"],["label"]],"optional":["kind"]},
            "longpress":{"required":["x","y"],"optional":["duration_ms"]},
            "swipe":{"required":["x1","y1","x2","y2"],"optional":["duration_ms"]},
            "scroll":{"required":["x","y","dx","dy"]},
            "type":{"required":["text"],"optional":["clear"]},
            "back":{},
            "home":{},
            "launch_app":{"required":["app"]},
            "wait_for":{"required":["expect"],"optional":["timeout_ms"],
                "expect_fields":["app","label","identifier","value"]}
        },
        "batch":{"required":["steps"],"optional":["observe","settle_ms"]}
    })
}

/// Say what is wrong with one action, or `None` when it parses.
pub fn action_problem(value: &Value) -> Option<String> {
    let parsed = serde_json::from_value::<Action>(value.clone());
    let error = parsed.err()?;
    let Some(object) = value.as_object() else {
        return Some("an action must be a JSON object".into());
    };
    let has = |key: &str| object.contains_key(key);
    let kind = object.get("type").and_then(Value::as_str);
    if !has("type") {
        return Some(if has("action") {
            "use the key \"type\", not \"action\"".into()
        } else {
            "missing the \"type\" key".to_string()
        });
    }
    if kind == Some("scroll") && has("direction") {
        return Some(
            "scroll has no direction field; it takes x, y, dx and dy (negative dy scrolls down)"
                .into(),
        );
    }
    if kind == Some("swipe") && ["start", "end", "from", "to"].iter().any(|k| has(k)) {
        return Some("swipe takes flat fields x1, y1, x2 and y2".into());
    }
    Some(error.to_string())
}

/// The client-facing message for a rejected action or batch step.
pub fn action_message(problem: &str) -> String {
    format!("Invalid action, no action sent: {problem}. See android-use act --help")
}

/// Say what is wrong with a batch, naming the first bad step.
pub fn batch_problem(body: &Value, error: &str) -> String {
    if let Some(steps) = body.get("steps").and_then(Value::as_array) {
        for (index, step) in steps.iter().enumerate() {
            if let Some(problem) = action_problem(step) {
                return format!("step {}: {problem}", index + 1);
            }
        }
    }
    error.to_string()
}

/// Mark a full element read whose app area holds almost nothing, which is what
/// GL canvases (Unity, Flutter games) look like. Deltas are sparse by design.
pub fn annotate_sparse_tree(screen: &mut Value) {
    let app = screen["app"].as_str().unwrap_or_default();
    let height = screen["screen"]["height"].as_f64().unwrap_or(0.0);
    if screen.get("delta").is_some()
        || app.is_empty()
        || height <= 0.0
        || app.contains("launcher")
        || app == "com.android.systemui"
    {
        return;
    }
    let Some(rows) = screen["elements"].as_array() else {
        return;
    };
    let content = rows
        .iter()
        .filter(|row| {
            let named = ["label", "identifier"]
                .iter()
                .any(|key| row[key].as_str().is_some_and(|text| !text.is_empty()));
            let top = row["rect"][1].as_f64().unwrap_or(0.0);
            let middle = (top + row["rect"][3].as_f64().unwrap_or(0.0) / 2.0) / height;
            // The outer bands hold the status and navigation bars.
            named && (0.06..=0.92).contains(&middle)
        })
        .count();
    if content <= 1 {
        screen["canvas_rendered"] = json!(true);
        screen["hint"] = json!("This app exposes almost no accessible nodes and probably draws on a canvas. Use android-use screenshot --max-width 720, or add --crop, and tap normalized coordinates");
    }
}

/// Crop (normalized `x,y,w,h`) and shrink a PNG so a screenshot costs less to read.
pub fn resize_png(
    bytes: &[u8],
    max_width: Option<u32>,
    crop: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Invalid screenshot: {e}"))?;
    if let Some(crop) = crop {
        let parts: Vec<f64> = crop
            .split(',')
            .map(|part| part.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| "--crop takes four numbers: x,y,w,h".to_string())?;
        let [x, y, w, h] = parts[..] else {
            return Err("--crop takes four numbers: x,y,w,h".into());
        };
        if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x + w > 1.0 || y + h > 1.0 {
            return Err("--crop must stay inside the normalized 0 to 1 screen".into());
        }
        let (width, height) = (f64::from(image.width()), f64::from(image.height()));
        image = image.crop_imm(
            (x * width) as u32,
            (y * height) as u32,
            ((w * width) as u32).max(1),
            ((h * height) as u32).max(1),
        );
    }
    if let Some(limit) = max_width {
        if limit == 0 {
            return Err("--max-width must be positive".into());
        }
        if image.width() > limit {
            image = image.resize(limit, u32::MAX, image::imageops::FilterType::Triangle);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("Unable to encode screenshot: {e}"))?;
    Ok(out.into_inner())
}

/// Device tools that need USB debugging. They run on the computer through ADB,
/// so they are unavailable to a phone driven only by the accessibility companion.
#[derive(clap::Subcommand)]
pub enum AdbCommand {
    /// Open the recent apps screen (KEYCODE_APP_SWITCH).
    Recents,
    /// Stop a package with am force-stop.
    ForceStop { package: String },
    /// Force-stop a package, then start its launcher activity.
    Restart { package: String },
    /// Print recent log lines; read-only.
    Logcat {
        /// Keep only this tag; repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        pid: Option<u32>,
        /// Start time: epoch seconds or "MM-DD hh:mm:ss.mmm".
        #[arg(long)]
        since: Option<String>,
        /// Most recent lines to print when --since is absent.
        #[arg(long, default_value_t = 500)]
        lines: u32,
    },
    /// Copy a file or directory from the phone to the computer.
    Pull {
        remote: String,
        /// Destination; defaults to the remote name in the current directory.
        local: Option<String>,
    },
    /// Show version, install times, APK paths and run state of a package.
    Package {
        /// Defaults to the daemon's foreground app.
        package: Option<String>,
    },
    /// List installed packages with version codes.
    Apps {
        /// Include system packages.
        #[arg(long)]
        all: bool,
    },
}
impl AdbCommand {
    /// Changes phone state, so it must hold the daemon lease first.
    pub fn mutates(&self) -> bool {
        matches!(
            self,
            Self::Recents | Self::ForceStop { .. } | Self::Restart { .. }
        )
    }
}

/// Why force_stop/restart_app refuse on the accessibility backend.
pub const FORCE_STOP_NEEDS_ADB: &str = "force_stop and restart_app require ADB (USB debugging): Android does not let an accessibility service stop another app. Connect the phone over USB with debugging enabled, or stop it by hand in Settings > Apps";

/// Parse a `/agent/device` body: {"command":"recents"} or
/// {"command":"force_stop"|"restart_app","package":"com.example"}.
pub fn device_command(body: &Value) -> Result<AdbCommand, String> {
    let package = || match body["package"].as_str() {
        Some(p) if valid_package(p) => Ok(p.to_string()),
        Some(p) => Err(format!("Invalid package name: {p}")),
        None => Err("\"package\" is required".to_string()),
    };
    match body["command"].as_str() {
        Some("recents") => Ok(AdbCommand::Recents),
        Some("force_stop") => Ok(AdbCommand::ForceStop {
            package: package()?,
        }),
        Some("restart_app") => Ok(AdbCommand::Restart {
            package: package()?,
        }),
        other => Err(format!(
            "Unknown device command {other:?}; use recents, force_stop or restart_app"
        )),
    }
}

pub fn valid_package(name: &str) -> bool {
    name.contains('.')
        && name.len() <= 255
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
}

pub fn logcat_args(
    tags: &[String],
    pid: Option<u32>,
    since: Option<&str>,
    lines: u32,
) -> Result<Vec<String>, String> {
    let mut args = vec!["logcat".to_string(), "-d".into()];
    if let Some(pid) = pid {
        args.push(format!("--pid={pid}"));
    }
    match since {
        Some(since) => {
            let allowed = since
                .bytes()
                .all(|b| b.is_ascii_digit() || b" :.-".contains(&b));
            if since.is_empty() || since.starts_with('-') || !allowed {
                return Err("--since takes epoch seconds or \"MM-DD hh:mm:ss.mmm\"".into());
            }
            args.extend(["-T".into(), since.into()]);
        }
        None => args.extend(["-t".into(), lines.max(1).to_string()]),
    }
    if !tags.is_empty() {
        args.push("-s".into());
    }
    for tag in tags {
        let allowed = tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-$".contains(&b));
        if tag.is_empty() || tag.starts_with('-') || !allowed {
            return Err(format!("Invalid log tag: {tag}"));
        }
        args.push(tag.clone());
    }
    Ok(args)
}

/// Read one package's block of `dumpsys package NAME`.
pub fn parse_package(name: &str, dumpsys: &str) -> Option<Value> {
    let field = |key: &str| {
        dumpsys.lines().find_map(|line| {
            let line = line.trim();
            let at = line.find(key)?;
            let boundary = at == 0 || line.as_bytes()[at - 1] == b' ';
            boundary.then(|| &line[at + key.len()..])
        })
    };
    let first = |key: &str| field(key).and_then(|text| text.split_whitespace().next());
    let whole = |key: &str| field(key).map(str::trim);
    let version_code = first("versionCode=")?;
    Some(
        json!({"package":name,"label":null,"version_name":first("versionName="),
        "version_code":version_code.parse::<u64>().ok(),
        "first_install_time":whole("firstInstallTime="),
        "last_update_time":whole("lastUpdateTime=")}),
    )
}

/// Parse `pm list packages --show-versioncode`.
pub fn parse_packages(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?.strip_prefix("package:")?;
            let version = fields.find_map(|f| f.strip_prefix("versionCode:"));
            Some(json!({"package":name,"version_code":version.and_then(|v| v.parse::<u64>().ok())}))
        })
        .collect()
}

struct AdbOutput {
    ok: bool,
    stdout: String,
    stderr: String,
}
async fn adb(serial: &str, args: &[&str], seconds: u64) -> Result<AdbOutput, Value> {
    let program = std::env::var_os("ANDROID_USE_ADB").unwrap_or_else(|| "adb".into());
    let mut command = tokio::process::Command::new(program);
    if cfg!(target_os = "macos") && std::env::var_os("ADB_LIBUSB").is_none() {
        command.env("ADB_LIBUSB", "1");
    }
    command
        .args(["-s", serial])
        .args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    let waited =
        tokio::time::timeout(std::time::Duration::from_secs(seconds), command.output()).await;
    match waited {
        Ok(Ok(output)) => Ok(AdbOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }),
        Ok(Err(_)) => Err(json!({"ok":false,"error":"adb_unavailable",
            "message":"Unable to start adb; install Android platform tools"})),
        Err(_) => Err(json!({"ok":false,"error":"adb_timeout",
            "message":format!("adb did not finish within {seconds} seconds")})),
    }
}
fn failed(error: &str, output: &AdbOutput) -> Value {
    let detail = if output.stderr.is_empty() {
        output.stdout.trim()
    } else {
        &output.stderr
    };
    json!({"ok":false,"error":error,"message":detail})
}
/// A state change may have landed even when it reports failure, so never retry.
fn unconfirmed(output: &AdbOutput) -> Value {
    let mut value = failed("adb_action_failed", output);
    value["outcome"] = json!("unknown");
    value["retry_safe"] = json!(false);
    value
}
fn bad_package(package: &str) -> Value {
    json!({"ok":false,"error":"invalid_package","message":format!("Invalid package name: {package}")})
}
async fn force_stop(serial: &str, package: &str) -> Result<AdbOutput, Value> {
    adb(serial, &["shell", "am", "force-stop", package], 15).await
}

pub async fn run_adb(serial: &str, command: AdbCommand, foreground: Option<String>) -> Value {
    match run_adb_inner(serial, command, foreground).await {
        Ok(value) | Err(value) => value,
    }
}
async fn run_adb_inner(
    serial: &str,
    command: AdbCommand,
    foreground: Option<String>,
) -> Result<Value, Value> {
    Ok(match command {
        AdbCommand::Recents => {
            let args = ["shell", "input", "keyevent", "KEYCODE_APP_SWITCH"];
            let output = adb(serial, &args, 15).await?;
            if output.ok {
                json!({"ok":true,"serial":serial})
            } else {
                unconfirmed(&output)
            }
        }
        AdbCommand::ForceStop { package } => {
            if !valid_package(&package) {
                return Err(bad_package(&package));
            }
            let output = force_stop(serial, &package).await?;
            if output.ok {
                json!({"ok":true,"serial":serial,"package":package,"stopped":true})
            } else {
                unconfirmed(&output)
            }
        }
        AdbCommand::Restart { package } => {
            if !valid_package(&package) {
                return Err(bad_package(&package));
            }
            let output = force_stop(serial, &package).await?;
            if !output.ok {
                return Err(unconfirmed(&output));
            }
            let start = [
                "shell",
                "monkey",
                "-p",
                &package,
                "-c",
                "android.intent.category.LAUNCHER",
                "1",
            ];
            let output = adb(serial, &start, 15).await?;
            // monkey can exit 0 without finding a launcher activity.
            if output.ok && output.stdout.contains("Events injected: 1") {
                json!({"ok":true,"serial":serial,"package":package,"restarted":true})
            } else {
                let mut value = unconfirmed(&output);
                value["stopped"] = json!(true);
                value
            }
        }
        AdbCommand::Logcat {
            tags,
            pid,
            since,
            lines,
        } => {
            let args = logcat_args(&tags, pid, since.as_deref(), lines).map_err(
                |message| json!({"ok":false,"error":"invalid_filter","message":message}),
            )?;
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = adb(serial, &args, 30).await?;
            if output.ok {
                json!({"ok":true,"serial":serial,"log":output.stdout})
            } else {
                failed("logcat_failed", &output)
            }
        }
        AdbCommand::Pull { remote, local } => {
            if !remote.starts_with('/') {
                return Err(json!({"ok":false,"error":"invalid_path",
                    "message":"The device path must be absolute"}));
            }
            let local = local.unwrap_or_else(|| {
                let name = remote.trim_end_matches('/').rsplit('/').next();
                name.filter(|n| !n.is_empty()).unwrap_or("pulled").into()
            });
            let output = adb(serial, &["pull", &remote, &local], 300).await?;
            if output.ok {
                json!({"ok":true,"serial":serial,"remote":remote,"local":local})
            } else {
                failed("pull_failed", &output)
            }
        }
        AdbCommand::Package { package } => {
            let Some(package) = package.or(foreground) else {
                return Err(json!({"ok":false,"error":"package_required",
                    "message":"Name a package; no daemon reported a foreground app"}));
            };
            if !valid_package(&package) {
                return Err(bad_package(&package));
            }
            let dump = adb(serial, &["shell", "dumpsys", "package", &package], 15).await?;
            let Some(mut info) = parse_package(&package, &dump.stdout) else {
                return Err(json!({"ok":false,"error":"package_not_found",
                    "message":format!("{package} is not installed")}));
            };
            let paths = adb(serial, &["shell", "pm", "path", &package], 15).await?;
            info["apk_paths"] = json!(paths
                .stdout
                .lines()
                .filter_map(|line| line.trim().strip_prefix("package:"))
                .collect::<Vec<_>>());
            let pid = adb(serial, &["shell", "pidof", &package], 15).await?;
            info["running"] = json!(pid.ok && !pid.stdout.trim().is_empty());
            info["ok"] = json!(true);
            info["serial"] = json!(serial);
            info
        }
        AdbCommand::Apps { all } => {
            let mut args = vec!["shell", "pm", "list", "packages", "--show-versioncode"];
            if !all {
                args.push("-3");
            }
            let output = adb(serial, &args, 30).await?;
            if output.ok {
                json!({"ok":true,"serial":serial,"apps":parse_packages(&output.stdout)})
            } else {
                failed("package_list_failed", &output)
            }
        }
    })
}
