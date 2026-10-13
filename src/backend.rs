//! ADB lifecycle + one-shot UIAutomator JSON-RPC. Mutations are never retried.
use phone_control::{BackendFuture, DeviceBackend, Dispatch, Element, Failure, Screen};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};

pub const RUNNER_URL: &str =
    "https://github.com/openatx/android-uiautomator-server-jar/releases/download/0.4.0/u2.jar";
pub const RUNNER_SHA256: &str = "0b74e83c55f443539a9f76f5ce023a51466b764b1100e4097a897053fdfc0eb6";
const REMOTE_JAR: &str = "/data/local/tmp/android-use-u2-0.4.0-adapter.jar";

async fn loopback_runner(source: &Path) -> Result<PathBuf, Failure> {
    let invalid = || {
        Failure::not_sent(
            "runner_file_failed",
            "Unable to read or cache the pinned runner",
        )
    };
    let metadata = tokio::fs::metadata(source).await.map_err(|_| invalid())?;
    if metadata.len() > 16 * 1024 * 1024 {
        return Err(invalid());
    }
    let original = tokio::fs::read(source).await.map_err(|_| invalid())?;
    let bytes = crate::runner::assemble(&original)?;
    let directory = data_dir().map_err(|_| invalid())?.join("runner");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|_| invalid())?;
    let path = directory.join("u2-0.4.0-adapter.jar");
    if tokio::fs::read(&path)
        .await
        .is_ok_and(|existing| existing == bytes)
    {
        return Ok(path);
    }
    let temporary = directory.join(format!("u2-loopback-{}.tmp", std::process::id()));
    tokio::fs::write(&temporary, &bytes)
        .await
        .map_err(|_| invalid())?;
    #[cfg(windows)]
    if path.exists() {
        tokio::fs::remove_file(&path).await.map_err(|_| invalid())?;
    }
    tokio::fs::rename(temporary, &path)
        .await
        .map_err(|_| invalid())?;
    Ok(path)
}

pub fn data_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| anyhow::anyhow!("HOME or USERPROFILE is required"))?;
    Ok(PathBuf::from(home).join(".android-use"))
}

pub async fn setup_runner() -> anyhow::Result<PathBuf> {
    let dir = data_dir()?.join("runner");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("u2-0.4.0.jar");
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if format!("{:x}", Sha256::digest(&bytes)) == RUNNER_SHA256 {
            return Ok(path);
        }
    }
    let client = Client::builder().timeout(Duration::from_secs(60)).build()?;
    let mut response = client.get(RUNNER_URL).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            anyhow::bail!("Runner download exceeds size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    if format!("{:x}", Sha256::digest(&bytes)) != RUNNER_SHA256 {
        anyhow::bail!("Runner checksum mismatch");
    }
    // Write atomically in a unique temporary path, including concurrent setup.
    let temp = dir.join(format!("u2-{}.tmp", std::process::id()));
    tokio::fs::write(&temp, &bytes).await?;
    tokio::fs::rename(&temp, &path).await?;
    Ok(path)
}

fn adb_program() -> std::ffi::OsString {
    std::env::var_os("ANDROID_USE_ADB").unwrap_or_else(|| "adb".into())
}
fn adb_command() -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(adb_program());
    // The native macOS USB backend crashed when the acceptance phone was
    // attached. Use libusb for new ADB servers without restarting existing
    // servers or overriding an explicit caller preference.
    #[cfg(target_os = "macos")]
    if std::env::var_os("ADB_LIBUSB").is_none() {
        command.env("ADB_LIBUSB", "1");
    }
    command
}
async fn adb(serial: &str, args: &[&str], mutation: bool) -> Result<Vec<u8>, Failure> {
    let mut command = adb_command();
    command.args(["-s", serial]).args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => Ok(output.stdout),
        Ok(Err(_)) => Err(Failure::not_sent(
            "adb_unavailable",
            "Unable to start adb; install Android platform tools",
        )),
        _ if mutation => Err(Failure::unknown(
            "adb_action_failed",
            "ADB command did not confirm completion; read the screen before continuing",
        )),
        _ => Err(Failure::not_sent(
            "device_unavailable",
            "ADB device is offline, unauthorized, or the command timed out",
        )),
    }
}

pub struct AndroidBackend {
    serial: String,
    rpc_url: String,
    client: Client,
    runner: Option<Child>,
    forward: Option<String>,
    adapter_token: Option<String>,
    remote_token: Option<String>,
    bridge: Option<crate::accessibility::Bridge>,
}
impl AndroidBackend {
    /// Used for externally managed runners and protocol fixtures. No ADB
    /// discovery, startup, restart or replay happens through this path.
    pub fn from_endpoint(serial: String, rpc_url: String) -> Result<Self, Failure> {
        let client = Client::builder()
            .http1_only()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Failure::not_sent("client_failed", "HTTP client initialization failed"))?;
        Ok(Self {
            serial,
            rpc_url,
            client,
            runner: None,
            forward: None,
            adapter_token: None,
            remote_token: None,
            bridge: None,
        })
    }
    pub fn from_accessibility(
        device_id: String,
        bridge: crate::accessibility::Bridge,
    ) -> Result<Self, Failure> {
        let mut backend = Self::from_endpoint(device_id, String::new())?;
        backend.bridge = Some(bridge);
        Ok(backend)
    }
    pub fn bridge(&self) -> Option<crate::accessibility::Bridge> {
        self.bridge.clone()
    }
    pub fn is_accessibility(&self) -> bool {
        self.bridge.is_some()
    }
    pub async fn connect(serial: String, jar: &Path) -> Result<Self, Failure> {
        let state = adb(&serial, &["get-state"], false).await?;
        if String::from_utf8_lossy(&state).trim() != "device" {
            return Err(Failure::not_sent(
                "device_unavailable",
                "Authorize USB debugging on the selected device",
            ));
        }
        let jar = loopback_runner(jar).await?;
        let jar = jar.to_str().ok_or_else(|| {
            Failure::not_sent("invalid_path", "Runner path must be valid Unicode")
        })?;
        adb(&serial, &["push", jar, REMOTE_JAR], false).await?;
        let port = adb(&serial, &["forward", "tcp:0", "tcp:19008"], false).await?;
        let port: u16 = String::from_utf8_lossy(&port).trim().parse().map_err(|_| {
            Failure::not_sent(
                "forward_failed",
                "ADB did not return a valid forwarded port",
            )
        })?;
        let forward = format!("tcp:{port}");
        let mut backend = Self::from_endpoint(serial, format!("http://127.0.0.1:{port}"))?;
        backend.forward = Some(forward);
        if backend
            .client
            .get(format!("{}/ping", backend.rpc_url))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            // Never reuse a runner whose binding and lifecycle are unknown.
            // Remove only the forward this process just created.
            backend.close().await;
            return Err(Failure::not_sent("runner_port_in_use", "Another runner owns device port 19008; release that session before starting android-use"));
        }
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| {
            Failure::not_sent(
                "random_unavailable",
                "Unable to create runner authorization",
            )
        })?;
        use base64::Engine as _;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        let local_token = data_dir()
            .map_err(|_| Failure::not_sent("runner_file_failed", "Data directory unavailable"))?
            .join(format!("runner/auth-{}.tmp", std::process::id()));
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&local_token).map_err(|_| {
                Failure::not_sent(
                    "runner_file_failed",
                    "Unable to create private authorization file",
                )
            })?;
            file.write_all(token.as_bytes()).map_err(|_| {
                Failure::not_sent("runner_file_failed", "Unable to write runner authorization")
            })?;
        }
        // Port 19008 was free, so no runner is starting; a live runner has
        // already read its token. Remove files left by a killed daemon.
        let _ = adb(
            &backend.serial,
            &["shell", "rm -f /data/local/tmp/android-use-auth-*.txt"],
            false,
        )
        .await;
        let remote_token = format!("/data/local/tmp/android-use-auth-{}.txt", &token[..12]);
        let pushed = adb(
            &backend.serial,
            &[
                "push",
                local_token.to_str().expect("data path"),
                &remote_token,
            ],
            false,
        )
        .await;
        let _ = tokio::fs::remove_file(local_token).await;
        backend.remote_token = Some(remote_token.clone());
        if let Err(error) = pushed {
            backend.close().await;
            return Err(error);
        }
        if let Err(error) = adb(
            &backend.serial,
            &["shell", "chmod", "600", &remote_token],
            false,
        )
        .await
        {
            backend.close().await;
            return Err(error);
        }
        backend.adapter_token = Some(token);
        let shell = format!("CLASSPATH=/data/local/tmp/android-use-u2-0.4.0-adapter.jar app_process / com.androiduse.Runner 19008 {remote_token}");
        let mut command = adb_command();
        command
            .args(["-s", &backend.serial, "shell", &shell])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        backend.runner = match command.spawn() {
            Ok(runner) => Some(runner),
            Err(_) => {
                backend.close().await;
                return Err(Failure::not_sent(
                    "runner_start_failed",
                    "Unable to start UIAutomator runner",
                ));
            }
        };
        for _ in 0..30 {
            if backend
                .runner
                .as_mut()
                .is_some_and(|p| p.try_wait().ok().flatten().is_some())
            {
                backend.close().await;
                return Err(Failure::not_sent("runner_exited", "UIAutomator runner exited; another accessibility automation service may own the device"));
            }
            if backend
                .client
                .get(format!("{}/ping", backend.rpc_url))
                .timeout(Duration::from_secs(1))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                // The runner reads its token once at startup.
                if let Some(token_file) = backend.remote_token.take() {
                    let _ = adb(&backend.serial, &["shell", "rm", "-f", &token_file], false).await;
                }
                return Ok(backend);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        backend.close().await;
        Err(Failure::not_sent(
            "runner_not_ready",
            "UIAutomator runner did not become ready",
        ))
    }
    async fn rpc(&self, method: &str, params: Value, mutation: bool) -> Result<Value, Failure> {
        let failure = |error: &str| {
            if mutation {
                Failure::unknown(
                    error,
                    "Device did not acknowledge the action; do not replay it",
                )
            } else {
                Failure::not_sent(
                    error,
                    "Device observation failed; reconnect only after checking device status",
                )
            }
        };
        let body: Value = if let Some(bridge) = &self.bridge {
            bridge.call(method, params, mutation).await?
        } else {
            let request = self.client.post(format!("{}/jsonrpc/0", self.rpc_url));
            let request = if let Some(token) = &self.adapter_token {
                request.bearer_auth(token)
            } else {
                request
            };
            let response = request
                .header("Accept-Encoding", "")
                .header("Content-Type", "application/json")
                .body(ascii_json(
                    &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
                ))
                .send()
                .await
                .map_err(|e| {
                    if e.is_connect() {
                        Failure::not_sent(
                            "runner_unreachable",
                            "No request reached the UIAutomator service",
                        )
                    } else {
                        failure("rpc_transport_failed")
                    }
                })?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(Failure::not_sent(
                    "runner_unauthorized",
                    "Runner authorization expired; reconnect the device session",
                ));
            }
            if !response.status().is_success() {
                return Err(failure("rpc_http_error"));
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| failure("rpc_body_failed"))?
            {
                if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                    return Err(failure("rpc_response_too_large"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: Value =
                serde_json::from_slice(&bytes).map_err(|_| failure("rpc_invalid_response"))?;
            body
        };
        if body.get("id") != Some(&json!(1))
            || body.get("jsonrpc") != Some(&json!("2.0"))
            || body.get("error").is_some()
        {
            if body.pointer("/error/data/dispatched") == Some(&json!(false)) {
                return Err(Failure::not_sent(
                    body.pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("runner_refused"),
                    if self.bridge.is_some() {
                        match body.pointer("/error/message").and_then(Value::as_str) {
                            Some("accessibility_unavailable") => "Enable android-use in Accessibility settings, or reopen the phone app after force-stop",
                            Some("screen_unavailable") => "Unlock the phone and check the accessibility service",
                            Some("screenshot_unavailable") => "Screenshots require Android 11/API 30 or newer and an unprotected screen",
                            _ => "Phone refused before dispatch; inspect the focused field or device state",
                        }
                    } else {
                        "Runner refused before dispatch; inspect the focused field or device state"
                    },
                ));
            }
            return Err(failure("rpc_error"));
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| failure("rpc_missing_result"))
    }
    async fn acknowledged(&self, method: &str, params: Value) -> Result<(), Failure> {
        let result = self.rpc(method, params, true).await?;
        if result == json!(true) {
            Ok(())
        } else {
            Err(Failure::unknown(
                "action_not_acknowledged",
                "Runner did not acknowledge success; read the screen before continuing",
            ))
        }
    }
    pub async fn close(&mut self) {
        if let Some(bridge) = &self.bridge {
            bridge.close().await;
        }
        if let Some(mut runner) = self.runner.take() {
            let _ = runner.kill().await;
            let _ = runner.wait().await;
            // Any app can stat this path; do not leave the runner behind.
            let _ = adb(&self.serial, &["shell", "rm", "-f", REMOTE_JAR], false).await;
        }
        if let Some(token_file) = self.remote_token.take() {
            let _ = adb(&self.serial, &["shell", "rm", &token_file], false).await;
        }
        if let Some(forward) = self.forward.take() {
            let _ = adb(&self.serial, &["forward", "--remove", &forward], false).await;
        }
    }
}
impl DeviceBackend for AndroidBackend {
    fn native_typing(&self) -> bool {
        self.adapter_token.is_some() || self.bridge.is_some()
    }
    fn supports_append(&self) -> bool {
        false
    }
    fn platform(&self) -> &'static str {
        "android"
    }
    fn read(&mut self) -> BackendFuture<'_, Screen> {
        Box::pin(async move {
            if self.native_typing() {
                let value = self.rpc("screen", json!([]), false).await?;
                let mut screen: Screen = serde_json::from_value(value).map_err(|_| {
                    Failure::not_sent("invalid_screen", "Runner returned invalid screen data")
                })?;
                screen.device = self.serial.clone();
                screen.snapshot()?;
                return Ok(screen);
            }
            let before = self.rpc("deviceInfo", json!([]), false).await?;
            let xml = self
                .rpc("dumpWindowHierarchy", json!([false, 50]), false)
                .await?;
            let after = self.rpc("deviceInfo", json!([]), false).await?;
            for key in [
                "currentPackageName",
                "displayWidth",
                "displayHeight",
                "displayRotation",
            ] {
                if before.get(key) != after.get(key) {
                    return Err(Failure::not_sent(
                        "screen_changed_during_read",
                        "Screen rotated or switched apps during the read",
                    ));
                }
            }
            let xml = xml.as_str().ok_or_else(|| {
                Failure::not_sent("invalid_hierarchy", "Runner did not return XML")
            })?;
            let mut screen = parse_screen(&self.serial, &after, xml)?;
            // The upstream XML dumper replaces UTF-16 surrogate halves with
            // dots, even when the input itself contains valid emoji. Read the
            // focused native field via JSON-RPC to preserve its actual value.
            let focused: Vec<usize> = screen
                .elements
                .iter()
                .enumerate()
                .filter(|(_, row)| row.kind == "TextField" && row.focused && !row.password)
                .map(|(index, _)| index)
                .collect();
            if focused.len() == 1 {
                let index = focused[0];
                let actual = self
                    .rpc(
                        "getText",
                        json!([focused_selector(&screen.elements[index])]),
                        false,
                    )
                    .await?;
                if let Some(value) = actual.as_str() {
                    screen.elements[index].value = Some(value.into());
                    screen.elements[index].label = value.into();
                } else {
                    screen.elements[index].value = None;
                }
            }
            Ok(screen)
        })
    }
    fn dispatch(&mut self, action: Dispatch) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let n = |v: f64| v.round() as i64;
            match action {
                Dispatch::NativeType { text, clear } => {
                    let result = self.rpc("typeText", json!([text, clear]), true).await?;
                    if result["applied"] != json!(true) {
                        return Err(Failure::unknown("text_not_applied", "Input result was not acknowledged; inspect the field before continuing"));
                    }
                    if result["verified"] != json!(true) && result["redacted"] != json!(true) {
                        return Err(Failure::unknown("text_not_verified", "Input was applied but its value could not be verified; do not replay it"));
                    }
                    Ok(())
                }
                Dispatch::Tap(x, y) => self.acknowledged("click", json!([n(x), n(y)])).await,
                Dispatch::Longpress(x, y, ms) => {
                    self.acknowledged("click", json!([n(x), n(y), ms])).await
                }
                Dispatch::Swipe(x1, y1, x2, y2, ms) => {
                    self.acknowledged(
                        "swipe",
                        json!([n(x1), n(y1), n(x2), n(y2), (ms / 5).clamp(1, 2000)]),
                    )
                    .await
                }
                Dispatch::Back => self.acknowledged("pressKey", json!(["back"])).await,
                Dispatch::Home => self.acknowledged("pressKey", json!(["home"])).await,
                Dispatch::SetText { element, text } => {
                    // Focus + resource id uniquely bind a native input. Reading
                    // selectors is safe, but setText is sent exactly once.
                    let selector = focused_selector(&element);
                    if self.rpc("count", json!([selector]), false).await? != json!(1) {
                        return Err(Failure::not_sent(
                            "ambiguous_input",
                            "Focused input selector must match exactly one field",
                        ));
                    }
                    self.acknowledged("setText", json!([selector, text]))
                        .await?;
                    let actual = self.rpc("getText", json!([selector]), false).await
                        .map_err(|_| Failure::unknown("text_not_verified", "Text was sent but its value could not be verified; read the field before continuing"))?;
                    if actual.as_str() != Some(text.as_str()) {
                        return Err(Failure::unknown("text_not_verified", "Text was sent but the field value differs; read the field and do not replay the whole text"));
                    }
                    Ok(())
                }
                Dispatch::LaunchApp(app) => {
                    if self.bridge.is_some() {
                        return self.acknowledged("launchApp", json!([app])).await;
                    }
                    let component = adb(
                        &self.serial,
                        &[
                            "shell",
                            "cmd",
                            "package",
                            "resolve-activity",
                            "--brief",
                            "-a",
                            "android.intent.action.MAIN",
                            "-c",
                            "android.intent.category.LAUNCHER",
                            &app,
                        ],
                        false,
                    )
                    .await?;
                    let component = String::from_utf8_lossy(&component);
                    let component = component
                        .lines()
                        .rev()
                        .find(|line| line.contains('/') && !line.contains(' '))
                        .filter(|line| {
                            line.starts_with(&format!("{app}/"))
                                && line
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"._/$".contains(&b))
                        })
                        .ok_or_else(|| {
                            Failure::not_sent(
                                "app_not_found",
                                "App has no launchable activity; use its package name",
                            )
                        })?;
                    let component = format!("'{component}'");
                    let result = adb(
                        &self.serial,
                        &["shell", "am", "start", "-W", "-n", &component],
                        true,
                    )
                    .await?;
                    if !String::from_utf8_lossy(&result).contains("Status: ok") {
                        return Err(Failure::unknown(
                            "launch_not_acknowledged",
                            "Activity launch was not confirmed",
                        ));
                    }
                    Ok(())
                }
            }
        })
    }
    fn screenshot(&mut self) -> BackendFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let bytes = if self.bridge.is_some() {
                use base64::Engine as _;
                let result = self.rpc("screenshot", json!([]), false).await?;
                base64::engine::general_purpose::STANDARD
                    .decode(result.as_str().ok_or_else(|| {
                        Failure::not_sent(
                            "screenshot_unavailable",
                            "Phone did not return a screenshot",
                        )
                    })?)
                    .map_err(|_| {
                        Failure::not_sent(
                            "screenshot_unavailable",
                            "Phone screenshot encoding invalid",
                        )
                    })?
            } else {
                adb(&self.serial, &["exec-out", "screencap", "-p"], false).await?
            };
            if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() > 32 * 1024 * 1024 {
                return Err(Failure::not_sent(
                    "screenshot_unavailable",
                    "Device did not return a valid PNG",
                ));
            }
            Ok(bytes)
        })
    }
}

pub fn parse_screen(serial: &str, info: &Value, xml: &str) -> Result<Screen, Failure> {
    let bad = || {
        Failure::not_sent(
            "invalid_hierarchy",
            "Runner returned an invalid or oversized element tree",
        )
    };
    if xml.len() > 4 * 1024 * 1024 {
        return Err(bad());
    }
    let document = roxmltree::Document::parse(xml).map_err(|_| bad())?;
    if document.root_element().tag_name().name() != "hierarchy" {
        return Err(bad());
    }
    let mut elements = Vec::new();
    for (node_index, node) in document
        .descendants()
        .filter(|n| n.has_tag_name("node"))
        .enumerate()
    {
        if node_index >= 10_000 {
            return Err(bad());
        }
        let class = node
            .attribute("class")
            .filter(|c| !c.is_empty())
            .ok_or_else(bad)?;
        let text = node.attribute("text").unwrap_or("");
        let description = node.attribute("content-desc").unwrap_or("");
        let flag = |name| node.attribute(name) == Some("true");
        let kind = if class.ends_with("ProgressBar") {
            "ProgressIndicator"
        } else if class.ends_with("EditText") || class.ends_with("AutoCompleteTextView") {
            "TextField"
        } else if class.ends_with("Switch") || class.ends_with("CheckBox") {
            "Switch"
        } else if class.ends_with("Button") || flag("clickable") {
            "Button"
        } else if flag("scrollable") {
            "ScrollView"
        } else if !text.is_empty() || !description.is_empty() {
            "StaticText"
        } else {
            "Other"
        };
        // Keep focused input and interactive controls even without a label;
        // discard decorative containers to reduce agent response size.
        let bounds = node.attribute("bounds").ok_or_else(bad)?;
        let numbers: Vec<f64> = bounds
            .split(['[', ']', ','])
            .filter(|s| !s.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        if numbers.len() != 4
            || !numbers.iter().all(|n| n.is_finite())
            || numbers[2] < numbers[0]
            || numbers[3] < numbers[1]
        {
            return Err(bad());
        }
        if kind == "Other" && !flag("focused") {
            continue;
        }
        let identifier = node
            .attribute("resource-id")
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        elements.push(Element {
            kind: kind.into(),
            label: if flag("password") {
                "<password>"
            } else if text.is_empty() {
                description
            } else {
                text
            }
            .into(),
            identifier,
            rect: [
                numbers[0],
                numbers[1],
                numbers[2] - numbers[0],
                numbers[3] - numbers[1],
            ],
            depth: node
                .ancestors()
                .filter(|n| n.has_tag_name("node"))
                .count()
                .saturating_sub(1) as u32,
            value: (kind == "TextField").then(|| {
                if flag("password") {
                    String::new()
                } else {
                    text.into()
                }
            }),
            enabled: flag("enabled"),
            visible: node.attribute("visible-to-user") != Some("false"),
            focused: flag("focused"),
            password: flag("password"),
        });
    }
    let screen = Screen {
        device: serial.into(),
        app: info["currentPackageName"].as_str().unwrap_or("").into(),
        width: info["displayWidth"].as_f64().ok_or_else(bad)?,
        height: info["displayHeight"].as_f64().ok_or_else(bad)?,
        elements,
    };
    screen.snapshot()?;
    Ok(screen)
}

/// NanoHTTPD's body decoding loses raw non-ASCII characters on some runners.
/// ASCII JSON with UTF-16 escapes preserves Unicode, including surrogate pairs.
pub fn ascii_json(value: &Value) -> String {
    use std::fmt::Write as _;
    let json = value.to_string();
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let mut units = [0; 2];
            for unit in ch.encode_utf16(&mut units) {
                write!(&mut out, "\\u{unit:04x}").expect("write to String");
            }
        }
    }
    out
}

fn focused_selector(element: &Element) -> Value {
    let mut selector =
        json!({"mask":0x020000,"focused":true,"childOrSibling":[],"childOrSiblingSelector":[]});
    if let Some(id) = &element.identifier {
        selector["resourceId"] = json!(id);
        selector["mask"] = json!(0x020000 | 0x200000);
    }
    selector
}
