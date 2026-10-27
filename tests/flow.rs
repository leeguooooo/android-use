use android_use::{
    flow::{self, FlowDevice, FlowDoc, RunOptions},
    http::{router, ApiState, Session},
    mcp::DaemonClient,
};
use phone_control::{BackendFuture, DeviceBackend, Dispatch, Element, Screen};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

// ---------------------------------------------------------------------------
// Runner against a scripted mock of the daemon API
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Mock {
    /// Every /agent/actions batch, in order.
    batches: Mutex<Vec<Value>>,
    /// Scripted responses by batch number (0-based); default ok/applied.
    replies: Mutex<BTreeMap<usize, Value>>,
    screenshots: Mutex<usize>,
    elements: Mutex<usize>,
}
impl FlowDevice for Mock {
    async fn actions(&self, batch: Value) -> Value {
        let mut batches = self.batches.lock().unwrap();
        let n = batches.len();
        let wait = batch["steps"][0]["type"] == "wait_for";
        batches.push(batch);
        self.replies.lock().unwrap().remove(&n).unwrap_or_else(|| {
            if wait {
                json!({"ok":true,"outcome":"not_sent","completed":1,"applied_actions":0})
            } else {
                json!({"ok":true,"outcome":"applied","completed":1,"applied_actions":1})
            }
        })
    }
    async fn elements(&self) -> Value {
        *self.elements.lock().unwrap() += 1;
        json!({"ok":true,"snapshot":"s1","app":"com.dice.game","elements":[
            {"index":0,"kind":"Button","label":"Settings","visible":true},
            {"index":1,"kind":"StaticText","label":"","visible":true}]})
    }
    async fn status(&self) -> Value {
        json!({"ok":true,"device":"emulator-5554","app":"com.dice.game","drivable":true})
    }
    async fn screenshot(&self) -> Result<Vec<u8>, String> {
        *self.screenshots.lock().unwrap() += 1;
        Ok(b"\x89PNG\r\n\x1a\nfake".to_vec())
    }
}

fn doc(value: Value) -> FlowDoc {
    serde_json::from_value(value).unwrap()
}
fn sample() -> FlowDoc {
    doc(json!({
    "version":1,"name":"Open match","app":"com.dice.game","category":"dice","risk":"navigation",
    "inputs":{"nick":{"description":"player name"}},
    "steps":[
        {"kind":"launch_app","app":"com.dice.game"},
        {"kind":"wait_for","expect":{"app":"com.dice.game"},"timeout_ms":15000},
        {"kind":"mark","name":"menu"},
        {"kind":"tap","label":"Play","element_kind":"Button","note":"main menu"},
        {"kind":"pause","ms":5},
        {"kind":"tap","x":0.5,"y":0.8},
        {"kind":"type","input":"nick","clear":true},
        {"kind":"screenshot","name":"lobby"}
    ]}))
}
fn opts(dir: &std::path::Path) -> RunOptions {
    RunOptions {
        inputs: BTreeMap::from([("nick".to_string(), "secret-name".to_string())]),
        run_dir: Some(dir.to_path_buf()),
        ..Default::default()
    }
}
fn events(dir: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn happy_path_sends_one_batch_per_device_step_and_logs_timed_events() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    let result = flow::run(&mock, "dice/open-match", &sample(), opts(dir.path())).await;
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["applied_actions"], 4);
    assert_eq!(result["retry_safe"], false);
    let batches = mock.batches.lock().unwrap();
    // launch, wait, tap label, tap xy, type — pause/mark/screenshot stay local.
    assert_eq!(batches.len(), 5);
    assert_eq!(
        batches[2]["steps"][0],
        json!({"type":"tap","label":"Play","kind":"Button"})
    );
    assert_eq!(batches[4]["steps"][0]["text"], "secret-name");
    assert!(batches
        .iter()
        .all(|b| b["steps"].as_array().unwrap().len() == 1));
    let lines = events(dir.path());
    let names: Vec<_> = lines.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(names.first(), Some(&"flow_start"));
    assert_eq!(names.last(), Some(&"flow_end"));
    assert_eq!(names.iter().filter(|n| **n == "step_start").count(), 8);
    assert_eq!(names.iter().filter(|n| **n == "step_end").count(), 8);
    assert!(names.contains(&"mark"));
    // Timestamps are epoch ms and never go backwards.
    let ts: Vec<u64> = lines.iter().map(|e| e["ts_ms"].as_u64().unwrap()).collect();
    assert!(ts.windows(2).all(|w| w[0] <= w[1]));
    assert!(lines
        .iter()
        .all(|e| e["ts"].as_str().unwrap().ends_with('Z')));
    // Typed input values never reach the log.
    let text = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
    assert!(!text.contains("secret-name"));
    assert!(dir.path().join("step-008-lobby.png").exists());
    assert!(dir.path().join("result.json").exists());
    assert_eq!(result["timings"].as_array().unwrap().len(), 8);
}

#[tokio::test]
async fn failure_names_the_step_captures_screen_and_forbids_blind_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    // Batch 2 is the label tap.
    mock.replies.lock().unwrap().insert(
        2,
        json!({"ok":false,"error":"ambiguous_element","message":"Label must match exactly one visible enabled element",
            "outcome":"not_sent","retry_safe":true,"completed":0,"applied_actions":0,"failed_step":0}),
    );
    let result = flow::run(&mock, "dice/open-match", &sample(), opts(dir.path())).await;
    assert_eq!(result["ok"], false);
    assert_eq!(result["failed_step"], 4);
    assert_eq!(result["error"], "ambiguous_element");
    let message = result["message"].as_str().unwrap();
    assert!(
        message
            .starts_with("step 4/8 (tap label=\"Play\" kind=\"Button\") failed: ambiguous_element"),
        "{message}"
    );
    // launch_app already applied, so the flow as a whole is not replayable.
    assert_eq!(result["retry_safe"], false);
    assert_eq!(result["outcome"], "partial");
    assert!(result["hint"].as_str().unwrap().contains("--from 4"));
    assert!(dir.path().join("failure.png").exists());
    assert!(dir.path().join("failure-elements.json").exists());
    assert_eq!(
        result["screen"]["visible_labels"],
        json!(["Button \"Settings\""])
    );
    assert_eq!(
        mock.batches.lock().unwrap().len(),
        3,
        "never retried, never continued"
    );
    let failed: Vec<_> = events(dir.path())
        .into_iter()
        .filter(|e| e["event"] == "step_end" && e["ok"] == false)
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["step"], 4);
}

#[tokio::test]
async fn unknown_outcome_tells_the_caller_not_to_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    mock.replies.lock().unwrap().insert(
        0,
        json!({"ok":false,"error":"daemon_unreachable","message":"Daemon response was lost; do not replay the action","outcome":"unknown","retry_safe":false}),
    );
    let result = flow::run(&mock, "x", &sample(), opts(dir.path())).await;
    assert_eq!(result["retry_safe"], false);
    assert_eq!(result["outcome"], "partial");
    assert!(result["hint"].as_str().unwrap().contains("never replay"));
}

#[tokio::test]
async fn from_and_to_run_only_the_slice() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    let mut options = opts(dir.path());
    options.from = Some(4);
    options.to = Some(6);
    let result = flow::run(&mock, "x", &sample(), options).await;
    assert_eq!(result["ok"], true);
    let batches = mock.batches.lock().unwrap().clone();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0]["steps"][0]["label"], "Play");
    assert_eq!(batches[1]["steps"][0]["x"], 0.5);
    let mut bad = opts(dir.path());
    bad.from = Some(9);
    assert_eq!(
        flow::run(&mock, "x", &sample(), bad).await["error"],
        "invalid_range"
    );
}

#[tokio::test]
async fn dry_run_touches_nothing_and_tolerates_missing_inputs() {
    let mock = Mock::default();
    let result = flow::run(
        &mock,
        "x",
        &sample(),
        RunOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["plan"].as_array().unwrap().len(), 8);
    assert!(result["warnings"].to_string().contains("nick"));
    assert!(mock.batches.lock().unwrap().is_empty());
    assert_eq!(*mock.screenshots.lock().unwrap(), 0);
    assert_eq!(*mock.elements.lock().unwrap(), 0);
    // A real run without the input refuses before touching the device.
    let real = flow::run(&mock, "x", &sample(), RunOptions::default()).await;
    assert_eq!(real["error"], "invalid_flow");
    assert!(real["message"].as_str().unwrap().contains("--input nick="));
    assert!(mock.batches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn long_waits_are_split_into_core_sized_waits() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    let timeout = json!({"ok":false,"error":"condition_timeout","message":"x","outcome":"not_sent","retry_safe":true,"applied_actions":0});
    mock.replies.lock().unwrap().insert(0, timeout.clone());
    let wait = doc(
        json!({"version":1,"name":"w","steps":[{"kind":"wait_for","expect":{"label":"Roll"},"timeout_ms":120000}]}),
    );
    let result = flow::run(
        &mock,
        "w",
        &wait,
        RunOptions {
            run_dir: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(result["ok"], true, "{result}");
    let batches = mock.batches.lock().unwrap();
    assert_eq!(batches.len(), 2);
    assert!(batches[0]["steps"][0]["timeout_ms"].as_u64().unwrap() <= 30_000);
}

#[tokio::test]
async fn optional_steps_skip_only_when_nothing_was_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    mock.replies.lock().unwrap().insert(
        0,
        json!({"ok":false,"error":"ambiguous_element","message":"none","outcome":"not_sent","retry_safe":true,"applied_actions":0}),
    );
    let flow_doc = doc(json!({"version":1,"name":"o","steps":[
        {"kind":"tap","label":"Close ad","optional":true},{"kind":"back"}]}));
    let result = flow::run(
        &mock,
        "o",
        &flow_doc,
        RunOptions {
            run_dir: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(result["ok"], true);
    assert!(events(dir.path())
        .iter()
        .any(|e| e["event"] == "step_skipped"));
}

#[tokio::test]
async fn side_effect_flows_need_confirm() {
    let mock = Mock::default();
    let mut d = sample();
    d.risk = Some(flow::Risk::SideEffect);
    let dir = tempfile::tempdir().unwrap();
    let result = flow::run(&mock, "x", &d, opts(dir.path())).await;
    assert_eq!(result["error"], "confirm_required");
    assert!(mock.batches.lock().unwrap().is_empty());
}

#[test]
fn validation_rejects_bad_flows_with_the_step_number() {
    let none = BTreeMap::new();
    let err = |v: Value| match serde_json::from_value::<FlowDoc>(v) {
        Ok(d) => flow::compile(&d, &none, true).err().unwrap(),
        Err(e) => e.to_string(),
    };
    assert!(err(json!({"version":1,"name":"a","steps":[],"extra":1})).contains("unknown field"));
    assert!(
        err(json!({"version":1,"name":"a","inputs":{"t":{}},"steps":[{"kind":"back"}]}))
            .contains("no step uses it")
    );
    assert!(
        err(json!({"version":1,"name":"a","steps":[{"kind":"back","input":"t"}]}))
            .contains("only a type step")
    );
    assert!(err(
        json!({"version":1,"name":"a","steps":[{"kind":"back"},{"kind":"pause","ms":600000}]})
    )
    .starts_with("step 2"));
    assert!(
        err(json!({"version":1,"name":"a","steps":[{"kind":"tap","x":2.0,"y":0.5}]}))
            .contains("invalid_action")
    );
    assert!(
        err(json!({"version":1,"name":"a","steps":[{"kind":"teleport"}]})).contains("unknown kind")
    );
    assert!(
        err(json!({"version":1,"name":"a","steps":[{"kind":"wait_for","expect":{}}]}))
            .contains("step 1")
    );
    assert!(
        err(json!({"version":1,"name":"a","steps":[{"kind":"screenshot","name":"../x"}]}))
            .contains("name must")
    );
    assert!(err(json!({"version":2,"name":"a","steps":[{"kind":"back"}]})).contains("version"));
    assert!(flow::compile(&sample(), &none, true).is_ok());
}

#[test]
fn iso_timestamps_are_utc() {
    assert_eq!(flow::iso_utc(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(flow::iso_utc(1_793_000_000_123), "2026-10-26T07:33:20.123Z");
    assert_eq!(flow::iso_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
}

#[test]
fn capture_commands_target_the_served_serial() {
    assert_eq!(
        flow::screenrecord_args("emulator-5554", "/sdcard/a.mp4", 180),
        [
            "-s",
            "emulator-5554",
            "shell",
            "screenrecord",
            "--time-limit",
            "180",
            "/sdcard/a.mp4"
        ]
    );
    assert_eq!(
        flow::logcat_args("emulator-5554", 1_700_000_000_042),
        [
            "-s",
            "emulator-5554",
            "logcat",
            "-v",
            "epoch",
            "-T",
            "1700000000.042"
        ]
    );
}

#[test]
fn registry_publish_list_info_report() {
    let registry = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let source = registry.path().join("draft.json");
    std::fs::write(&source, serde_json::to_string(&sample()).unwrap()).unwrap();
    let source_dir = tempfile::tempdir().unwrap();
    let source2 = source_dir.path().join("draft.json");
    std::fs::rename(&source, &source2).unwrap();
    // Unverified and unconfirmed publishes write nothing.
    assert_eq!(
        flow::publish(registry.path(), &source2, "dice/open-match", true, false)["error"],
        "unverified"
    );
    let preview = flow::publish(registry.path(), &source2, "dice/open-match", false, true);
    assert_eq!(preview["error"], "confirm_required");
    assert!(!registry.path().join("dice/open-match.json").exists());
    assert_eq!(
        flow::publish(registry.path(), &source2, "../evil", true, true)["error"],
        "invalid_id"
    );
    assert_eq!(
        flow::publish(registry.path(), &source2, "dice/open-match", true, true)["ok"],
        true
    );
    let listed = flow::list(
        registry.path(),
        Some(runs.path()),
        Some("com.dice.game"),
        None,
    );
    assert_eq!(listed["flows"][0]["id"], "dice/open-match");
    assert_eq!(listed["flows"][0]["inputs"], json!(["nick"]));
    assert_eq!(
        flow::list(registry.path(), None, Some("other.app"), None)["flows"],
        json!([])
    );
    let info = flow::info(registry.path(), None, "dice/open-match");
    assert_eq!(info["valid"]["ok"], true);
    assert_eq!(
        flow::info(registry.path(), None, "dice/missing")["error"],
        "flow_not_found"
    );
    // A recorded run shows up in report and list.
    let run = runs
        .path()
        .join("dice_open-match")
        .join("20261027T000000-000Z-1");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("result.json"),
        json!({"ok":false,"failed_step":4,"message":"step 4/8 failed","run_id":"r"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        run.join("events.jsonl"),
        format!("{}\n", json!({"event":"step_end","ok":false,"step":4})),
    )
    .unwrap();
    let report = flow::report(
        registry.path(),
        Some(runs.path()),
        "dice/open-match",
        None,
        false,
    );
    assert_eq!(report["result"]["failed_step"], 4);
    assert_eq!(report["failed_events"].as_array().unwrap().len(), 1);
    assert_eq!(
        flow::report(
            registry.path(),
            Some(runs.path()),
            "dice/open-match",
            Some("label changed"),
            false
        )["error"],
        "confirm_required"
    );
    let filed = flow::report(
        registry.path(),
        Some(runs.path()),
        "dice/open-match",
        Some("label changed"),
        true,
    );
    assert!(std::path::Path::new(filed["filed"].as_str().unwrap()).exists());
    // reports/ is not mistaken for flows.
    assert_eq!(
        flow::list(registry.path(), None, None, None)["flows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        flow::list(registry.path(), Some(runs.path()), None, None)["flows"][0]["last_run"]
            ["failed_step"],
        4
    );
}

// ---------------------------------------------------------------------------
// End to end: DaemonClient -> real router -> Controller -> fake backend
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Game {
    dispatched: Arc<Mutex<Vec<String>>>,
    in_match: Arc<Mutex<bool>>,
}
fn row(kind: &str, label: &str, rect: [f64; 4]) -> Element {
    Element {
        kind: kind.into(),
        label: label.into(),
        identifier: None,
        rect,
        depth: 1,
        value: None,
        enabled: true,
        visible: true,
        focused: false,
        password: false,
    }
}
impl DeviceBackend for Game {
    fn platform(&self) -> &'static str {
        "android"
    }
    fn read(&mut self) -> BackendFuture<'_, Screen> {
        let in_match = *self.in_match.lock().unwrap();
        Box::pin(async move {
            Ok(Screen {
                device: "emulator-fixture".into(),
                app: "com.dice.game".into(),
                width: 1080.0,
                height: 1920.0,
                elements: if in_match {
                    vec![row("Button", "Roll", [400.0, 1500.0, 280.0, 120.0])]
                } else {
                    vec![
                        row("Button", "Play", [400.0, 900.0, 280.0, 120.0]),
                        row("View", "", [0.0, 0.0, 200.0, 200.0]),
                    ]
                },
            })
        })
    }
    fn dispatch(&mut self, action: Dispatch) -> BackendFuture<'_, ()> {
        if let Dispatch::Tap(x, y) = action {
            if (400.0..680.0).contains(&x) && (900.0..1020.0).contains(&y) {
                *self.in_match.lock().unwrap() = true;
            }
        }
        self.dispatched.lock().unwrap().push(format!("{action:?}"));
        Box::pin(async { Ok(()) })
    }
    fn screenshot(&mut self) -> BackendFuture<'_, Vec<u8>> {
        Box::pin(async { Ok(b"\x89PNG\r\n\x1a\nfixture".to_vec()) })
    }
}

async fn serve(game: Game) -> (String, tokio::task::JoinHandle<()>) {
    let session = Arc::new(tokio::sync::Mutex::new(Session::new(game)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let api = router(ApiState::new(session, "fixture-token".into()));
    (
        url,
        tokio::spawn(async move { axum::serve(listener, api).await.unwrap() }),
    )
}

#[tokio::test]
async fn flow_runs_through_the_daemon_and_the_trail_becomes_a_draft() {
    let game = Game::default();
    let (url, server) = serve(game.clone()).await;
    let client = DaemonClient::new(url.clone(), "fixture-token".into(), "agent-a".into()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let d = doc(
        json!({"version":1,"name":"Start match","app":"com.dice.game","steps":[
        {"kind":"wait_for","expect":{"label":"Play"},"timeout_ms":2000},
        {"kind":"tap","label":"Play"},
        {"kind":"wait_for","expect":{"label":"Roll"},"timeout_ms":2000},
        {"kind":"tap","label":"Missing"}]}),
    );
    let result = flow::run(
        &client,
        "dice/start",
        &d,
        RunOptions {
            run_dir: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["failed_step"], 4);
    assert_eq!(result["error"], "ambiguous_element");
    assert_eq!(
        result["screen"]["visible_labels"],
        json!(["Button \"Roll\""])
    );
    assert!(dir.path().join("failure.png").exists());
    assert_eq!(game.dispatched.lock().unwrap().len(), 1);

    // The lease is per owner; release it so another session can record.
    client.request("/agent/release", Some(json!({}))).await;
    // A fresh recording: read, index tap on an unlabeled view, then draft.
    let recorder = DaemonClient::new(url, "fixture-token".into(), "agent-b".into()).unwrap();
    *game.in_match.lock().unwrap() = false;
    assert_eq!(
        recorder.request("/agent/flow/draft", None).await["error"],
        "empty_trail"
    );
    let screen = recorder.request("/agent/elements", None).await;
    let snapshot = screen["snapshot"].as_str().unwrap().to_string();
    let applied = recorder
        .request(
            "/agent/actions",
            Some(json!({"steps":[
            {"type":"tap","element":1,"snapshot":snapshot},
            {"type":"tap","element":0,"snapshot":snapshot},
            {"type":"type","text":"never-recorded","clear":true}],"observe":false})),
        )
        .await;
    // The type step fails (no focused field); only completed steps are recorded.
    assert_eq!(applied["completed"], 2, "{applied}");
    let draft = flow::finish_draft(recorder.request("/agent/flow/draft", None).await, None);
    assert_eq!(draft["ok"], true, "{draft}");
    let steps = draft["draft"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0], json!({"kind":"tap","x":0.0926,"y":0.0521}));
    assert_eq!(
        steps[1],
        json!({"kind":"tap","label":"Play","element_kind":"Button"})
    );
    assert_eq!(draft["draft"]["app"], "com.dice.game");
    assert_eq!(draft["valid"], true);
    assert!(!draft.to_string().contains("never-recorded"));
    assert!(draft["todo"].to_string().contains("unlabeled"));
    assert!(recorder.request("/agent/flow/reset", Some(json!({}))).await["ok"] == true);
    assert_eq!(
        recorder.request("/agent/flow/draft", None).await["error"],
        "empty_trail"
    );
    server.abort();
}

#[tokio::test]
async fn trail_keeps_order_and_survives_release() {
    let game = Game::default();
    let (url, server) = serve(game).await;
    let client = DaemonClient::new(url, "fixture-token".into(), "agent-c".into()).unwrap();
    // Wait + back complete; the draft keeps them in order.
    client
        .request("/agent/actions", Some(json!({"steps":[{"type":"wait_for","expect":{"label":"Play"},"timeout_ms":1000},{"type":"back"}],"observe":false})))
        .await;
    let draft = client.request("/agent/flow/draft", None).await;
    assert_eq!(
        draft["draft"]["steps"],
        json!([{"kind":"wait_for","expect":{"label":"Play"},"timeout_ms":1000},{"kind":"back"}])
    );
    // Release keeps the finished trail readable as `previous`.
    client.request("/agent/release", Some(json!({}))).await;
    assert_eq!(
        client.request("/agent/flow/draft", None).await["source"],
        "previous"
    );
    server.abort();
}

#[test]
fn typed_text_becomes_an_input_without_its_value() {
    let mut trail = android_use::trail::Trail::default();
    trail.note_batch(
        &[
            json!({"type":"launch_app","app":"com.dice.game"}),
            json!({"type":"type","text":"hunter2","clear":true}),
            json!({"type":"type","text":"x"}),
        ],
        &json!({"ok":true,"completed":3}),
    );
    let draft = trail.draft();
    assert_eq!(
        draft["draft"]["steps"][1],
        json!({"kind":"type","input":"text1","clear":true})
    );
    assert_eq!(
        draft["draft"]["steps"][2],
        json!({"kind":"type","input":"text2"})
    );
    assert_eq!(draft["draft"]["inputs"].as_object().unwrap().len(), 2);
    assert!(!draft.to_string().contains("hunter2"));
    let doc: FlowDoc = serde_json::from_value(draft["draft"].clone()).unwrap();
    assert!(flow::compile(&doc, &BTreeMap::new(), true).is_ok());
}
