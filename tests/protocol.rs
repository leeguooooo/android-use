use android_use::{
    backend::{parse_screen, AndroidBackend},
    http::{router, ApiState, Session},
};
use axum::{extract::State, routing::post, Json, Router};
use phone_control::{Controller, DeviceBackend, Dispatch};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const XML: &str = r#"<hierarchy rotation="0"><node class="android.widget.FrameLayout" bounds="[0,0][1080,1920]" enabled="true"><node class="android.widget.EditText" resource-id="fixture.app:id/input" text="你好 &amp; 日本語" content-desc="Input" bounds="[10,20][500,100]" enabled="true" focused="true" password="false"/><node class="android.widget.Button" text="Save" bounds="[10,120][200,200]" enabled="true" clickable="true"/></node></hierarchy>"#;
fn info() -> Value {
    json!({"currentPackageName":"fixture.app","displayWidth":1080,"displayHeight":1920,"displayRotation":0})
}
#[test]
fn xml_unicode_bounds_and_focused_values_are_preserved() {
    let screen = parse_screen("fixture", &info(), XML).unwrap();
    assert_eq!(screen.elements.len(), 2);
    assert_eq!(screen.elements[0].value.as_deref(), Some("你好 & 日本語"));
    assert_eq!(screen.elements[0].rect, [10.0, 20.0, 490.0, 80.0]);
    assert!(screen.elements[0].focused);
    assert_eq!(screen.elements[1].kind, "Button");
    assert!(parse_screen("fixture", &info(), "<hierarchy><node/></hierarchy>").is_err());
    assert!(parse_screen(
        "fixture",
        &info(),
        &XML.replace("[10,20][500,100]", "[500,100][10,20]")
    )
    .is_err());
    let autocomplete = parse_screen(
        "fixture",
        &info(),
        &XML.replace(
            "android.widget.EditText",
            "android.widget.AutoCompleteTextView",
        ),
    )
    .unwrap();
    assert_eq!(autocomplete.elements[0].kind, "TextField");
}
#[derive(Default)]
struct RpcState {
    calls: Vec<Value>,
    fail_mutation: bool,
    rotation_reads: usize,
    rotate: bool,
    text: Option<String>,
    corrupt_text: bool,
}
async fn rpc(State(state): State<Arc<Mutex<RpcState>>>, Json(request): Json<Value>) -> Json<Value> {
    let mut state = state.lock().unwrap();
    state.calls.push(request.clone());
    let value = match request["method"].as_str().unwrap() {
        "deviceInfo" => {
            state.rotation_reads += 1;
            let mut value = info();
            if state.rotate && state.rotation_reads % 2 == 0 {
                value["displayRotation"] = json!(1);
            }
            value
        }
        "dumpWindowHierarchy" => json!(XML),
        "setText" if !state.fail_mutation => {
            state.text = request["params"][1].as_str().map(str::to_string);
            json!(true)
        }
        "getText" => {
            if state.corrupt_text {
                json!("corrupted")
            } else {
                json!(state.text)
            }
        }
        "count" => json!(1),
        _ if state.fail_mutation => {
            return Json(
                json!({"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"Synthetic failure"}}),
            )
        }
        _ => json!(true),
    };
    Json(json!({"jsonrpc":"2.0","id":1,"result":value}))
}
async fn runner(state: Arc<Mutex<RpcState>>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/jsonrpc/0", post(rpc))
                .with_state(state),
        )
        .await
        .unwrap()
    });
    (url, task)
}
#[tokio::test]
async fn native_text_rpc_is_once_only_and_keeps_unicode() {
    let state = Arc::new(Mutex::new(RpcState::default()));
    let (url, task) = runner(state.clone()).await;
    let backend = AndroidBackend::from_endpoint("fixture".into(), url).unwrap();
    let mut control = Controller::new(backend);
    let result=control.execute(serde_json::from_value(json!({"steps":[{"type":"type","text":"中文 👩‍💻\n日本語","clear":true}],"observe":false})).unwrap()).await;
    assert_eq!(result["outcome"], "applied");
    let screen = control.elements(None).await.unwrap();
    assert_eq!(screen["elements"][0]["value"], "中文 👩‍💻\n日本語");
    let state = state.lock().unwrap();
    let mutations: Vec<_> = state
        .calls
        .iter()
        .filter(|r| r["method"] == "setText")
        .collect();
    assert_eq!(mutations.len(), 1);
    assert_eq!(mutations[0]["params"][1], "中文 👩‍💻\n日本語");
    assert_eq!(
        mutations[0]["params"][0]["resourceId"],
        "fixture.app:id/input"
    );
    task.abort();
}
#[tokio::test]
async fn rpc_error_is_unknown_and_never_retried() {
    let state = Arc::new(Mutex::new(RpcState {
        fail_mutation: true,
        ..Default::default()
    }));
    let (url, task) = runner(state.clone()).await;
    let mut backend = AndroidBackend::from_endpoint("fixture".into(), url).unwrap();
    let error = backend
        .dispatch(Dispatch::Tap(50.0, 60.0))
        .await
        .unwrap_err();
    assert_eq!(error.outcome, "unknown");
    assert!(!error.retry_safe);
    assert_eq!(state.lock().unwrap().calls.len(), 1);
    task.abort();
}
#[tokio::test]
async fn rotation_mid_read_rejects_the_observation() {
    let state = Arc::new(Mutex::new(RpcState {
        rotate: true,
        ..Default::default()
    }));
    let (url, task) = runner(state).await;
    let mut backend = AndroidBackend::from_endpoint("fixture".into(), url).unwrap();
    assert_eq!(
        backend.read().await.unwrap_err().error,
        "screen_changed_during_read"
    );
    task.abort();
}
#[tokio::test]
async fn http_auth_lease_stale_snapshot_and_cli_work_end_to_end() {
    let rpc_state = Arc::new(Mutex::new(RpcState::default()));
    let (url, runner) = runner(rpc_state.clone()).await;
    let backend = AndroidBackend::from_endpoint("fixture".into(), url).unwrap();
    let api = router(ApiState::new(
        Arc::new(tokio::sync::Mutex::new(Session::new(backend))),
        "fixture-token".into(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{url}/agent/elements"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let screen: Value = client
        .get(format!("{url}/agent/elements"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", "agent-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(screen["elements"][0]["index"], 0);
    assert_eq!(
        client
            .post(format!("{url}/agent/input"))
            .bearer_auth("fixture-token")
            .header("X-Phone-Owner", "agent-b")
            .json(&json!({"type":"home"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let stale: Value = client
        .post(format!("{url}/agent/input"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", "agent-a")
        .json(&json!({"type":"tap","element":1,"snapshot":"stale"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stale["outcome"], "not_sent");
    let result:Value=client.post(format!("{url}/agent/actions")).bearer_auth("fixture-token").header("X-Phone-Owner","agent-a").json(&json!({"steps":[{"type":"tap","element":1,"snapshot":screen["snapshot"]}],"observe":false})).send().await.unwrap().json().await.unwrap();
    assert_eq!(result["outcome"], "applied");
    assert_eq!(
        rpc_state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|r| r["method"] == "click")
            .count(),
        1
    );
    let home = tempfile::tempdir().unwrap();
    let cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_android-use"))
        .args(["--url", &url, "status"])
        .env("HOME", home.path())
        .env("ANDROID_USE_TOKEN", "fixture-token")
        .output()
        .await
        .unwrap();
    assert!(cli.status.success());
    let status: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(status["drivable"], true);
    server.abort();
    runner.abort();
}

#[tokio::test]
async fn mcp_stdio_discovers_tools_calls_the_daemon_and_releases_on_disconnect() {
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let rpc_state = Arc::new(Mutex::new(RpcState::default()));
    let (rpc_url, runner) = runner(rpc_state).await;
    let backend = AndroidBackend::from_endpoint("fixture".into(), rpc_url).unwrap();
    let api = router(ApiState::new(
        Arc::new(tokio::sync::Mutex::new(Session::new(backend))),
        "fixture-token".into(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let home = tempfile::tempdir().unwrap();
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_android-use"))
        .args(["--url", &url, "mcp"])
        .env("HOME", home.path())
        .env("ANDROID_USE_TOKEN", "fixture-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = process.stdin.take().unwrap();
    let mut lines = BufReader::new(process.stdout.take().unwrap()).lines();
    stdin.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"android-fixture","version":"1"}}})).as_bytes()).await.unwrap();
    let initialized: Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(initialized["result"]["serverInfo"]["name"], "android-use");
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n").await.unwrap();
    let listed: Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    for name in [
        "phone_elements",
        "phone_actions",
        "phone_type",
        "phone_release",
        "phone_screenshot",
    ] {
        assert!(tools.iter().any(|t| t["name"] == name));
    }
    stdin.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"phone_elements","arguments":{}}})).as_bytes()).await.unwrap();
    let called: Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(called["result"]["structuredContent"]["app"], "fixture.app");
    drop(stdin);
    assert!(tokio::time::timeout(Duration::from_secs(5), process.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    let result: Value = reqwest::Client::new()
        .get(format!("{url}/agent/elements"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", "next-agent")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["ok"], true);
    server.abort();
    runner.abort();
}

#[test]
fn runner_json_is_ascii_and_roundtrips_unicode_without_losing_surrogate_pairs() {
    let value = json!({"text":"中文 👩‍💻 日本語\n\"quoted\""});
    let encoded = android_use::backend::ascii_json(&value);
    assert!(encoded.is_ascii());
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
    assert!(encoded.contains("\\ud83d"));
}

#[tokio::test]
async fn text_ack_without_matching_field_value_never_reports_success_or_retries() {
    let state = Arc::new(Mutex::new(RpcState {
        corrupt_text: true,
        ..Default::default()
    }));
    let (url, task) = runner(state.clone()).await;
    let mut control =
        Controller::new(AndroidBackend::from_endpoint("fixture".into(), url).unwrap());
    let result = control
        .execute(
            serde_json::from_value(
                json!({"steps":[{"type":"type","text":"你好","clear":true}],"observe":false}),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"], "text_not_verified");
    assert_eq!(result["retry_safe"], false);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|r| r["method"] == "setText")
            .count(),
        1
    );
    task.abort();
}
#[tokio::test]
async fn appending_is_refused_before_mutation_when_native_hint_is_ambiguous() {
    let state = Arc::new(Mutex::new(RpcState::default()));
    let (url, task) = runner(state.clone()).await;
    let mut control =
        Controller::new(AndroidBackend::from_endpoint("fixture".into(), url).unwrap());
    let result = control
        .execute(
            serde_json::from_value(
                json!({"steps":[{"type":"type","text":"append"}],"observe":false}),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(result["error"], "append_input_unsupported");
    assert_eq!(result["outcome"], "not_sent");
    assert!(!state
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|r| r["method"] == "setText"));
    task.abort();
}

#[tokio::test]
async fn disconnect_is_authenticated_owner_checked_and_stops_new_mutations() {
    let state = Arc::new(Mutex::new(RpcState::default()));
    let (endpoint, runner_task) = runner(state.clone()).await;
    let backend = AndroidBackend::from_endpoint("fixture".into(), endpoint).unwrap();
    let session = Arc::new(tokio::sync::Mutex::new(Session::new(backend)));
    let stopping = session.lock().await.control.stopping.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let api = router(ApiState::new(session, "fixture-token".into()));
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{url}/agent/disconnect"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    client
        .get(format!("{url}/agent/elements"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", "agent-a")
        .send()
        .await
        .unwrap();
    assert_eq!(
        client
            .post(format!("{url}/agent/disconnect"))
            .bearer_auth("fixture-token")
            .header("X-Phone-Owner", "agent-b")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert!(!stopping.load(std::sync::atomic::Ordering::Acquire));
    assert!(client
        .post(format!("{url}/agent/disconnect"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", "agent-a")
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    assert!(stopping.load(std::sync::atomic::Ordering::Acquire));
    let response: Value = client
        .post(format!("{url}/agent/input"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", "agent-a")
        .json(&json!({"type":"home"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["error"], "control_stopping");
    assert!(!state
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|r| r["method"] == "pressKey"));
    server.abort();
    runner_task.abort();
}
