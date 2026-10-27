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
    /// Scripted diagnosis results, in call order; default healthy.
    health: Mutex<Vec<Value>>,
    diagnosed: Mutex<usize>,
    backend: Option<&'static str>,
    /// Scripted daemon status replies after the first (default healthy).
    statuses: Mutex<Vec<Value>>,
    /// record_trail flag of every batch.
    trail_flags: Mutex<Vec<bool>>,
}
impl FlowDevice for Mock {
    async fn actions(&self, batch: Value, record_trail: bool) -> Value {
        self.trail_flags.lock().unwrap().push(record_trail);
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
        let mut statuses = self.statuses.lock().unwrap();
        if !statuses.is_empty() {
            return statuses.remove(0);
        }
        json!({"ok":true,"device":"R5CT1234","app":"com.dice.game","drivable":true,"backend":self.backend.unwrap_or("adb")})
    }
    async fn diagnose(&self, serial: &str) -> Value {
        assert_eq!(serial, "R5CT1234");
        *self.diagnosed.lock().unwrap() += 1;
        let mut health = self.health.lock().unwrap();
        if health.is_empty() {
            json!({"ok":true,"problem":null,"state":"device","awake":true,"locked":false})
        } else {
            health.remove(0)
        }
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
            // The fixture serial is not a real adb device; skip the host-side check.
            preflight: false,
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

// ---------------------------------------------------------------------------
// Physical USB phone failure modes
// ---------------------------------------------------------------------------

use android_use::device;

#[test]
fn adb_device_list_keeps_every_state_with_a_hint() {
    let rows = device::parse_devices(
        "* daemon started successfully\nList of devices attached\n\
R5CT1234               device usb:1-1 product:o1s model:SM_G991B device:o1s transport_id:3\n\
9A1B2C                 unauthorized usb:1-2 transport_id:4\n\
ZY22                   offline transport_id:5\n\
0123ABC                no permissions (missing udev rules? user is in the plugdev group); see [http://developer.android.com/tools/device.html] usb:1-3\n\
192.168.1.5:5555       device product:x model:Pixel_7 device:panther transport_id:6\n\
emulator-5554          device product:sdk model:sdk transport_id:1\n",
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["state"], "device");
    assert_eq!(rows[0]["transport"], "usb");
    assert_eq!(rows[0]["model"], "SM_G991B");
    assert_eq!(rows[1]["state"], "unauthorized");
    assert!(rows[1]["hint"]
        .as_str()
        .unwrap()
        .contains("Allow USB debugging"));
    assert_eq!(rows[2]["state"], "offline");
    assert_eq!(rows[3]["state"], "no permissions");
    assert_eq!(rows[4]["transport"], "wifi");
    assert_eq!(rows[5]["transport"], "emulator");
    assert!(device::parse_devices("List of devices attached\n\n").is_empty());
}

#[test]
fn power_and_keyguard_dumps_are_read_across_android_versions() {
    assert_eq!(device::parse_awake("  mWakefulness=Asleep\n"), Some(false));
    assert_eq!(device::parse_awake("mWakefulness=Dozing"), Some(false));
    assert_eq!(device::parse_awake("mWakefulness=Awake"), Some(true));
    assert_eq!(device::parse_awake("Display Power: state=OFF"), Some(false));
    assert_eq!(device::parse_awake("nothing"), None);
    assert_eq!(
        device::parse_locked("    mShowingLockscreen=true mShowingDream=false"),
        Some(true)
    );
    assert_eq!(
        device::parse_locked("isStatusBarKeyguard=false mKeyguardShowing=false"),
        Some(false)
    );
    assert_eq!(device::parse_locked("mDreamingLockscreen=true"), Some(true));
    assert_eq!(
        device::parse_locked("KeyguardServiceDelegate\n    showing=true"),
        None
    );
    assert_eq!(
        device::verdict("device", Some(false), Some(true))["problem"],
        "asleep"
    );
    assert_eq!(
        device::verdict("device", Some(true), Some(true))["problem"],
        "locked"
    );
    assert_eq!(device::verdict("device", None, None)["ok"], true);
    assert_eq!(
        device::verdict("unauthorized", None, None)["problem"],
        "unauthorized"
    );
}

#[tokio::test]
async fn preflight_refuses_an_unauthorized_or_locked_phone_before_any_step() {
    for (health, error) in [
        (
            json!({"ok":false,"problem":"unauthorized","hint":"Accept the 'Allow USB debugging?' prompt"}),
            "device_unauthorized",
        ),
        (
            json!({"ok":false,"problem":"locked","hint":"unlock it"}),
            "device_locked",
        ),
        (
            json!({"ok":false,"problem":"asleep","hint":"wake it"}),
            "screen_off",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mock = Mock::default();
        mock.health.lock().unwrap().push(health);
        let result = flow::run(&mock, "x", &sample(), opts(dir.path())).await;
        assert_eq!(result["error"], error, "{result}");
        assert_eq!(result["retry_safe"], true);
        assert!(result["message"]
            .as_str()
            .unwrap()
            .contains("nothing was sent"));
        assert!(mock.batches.lock().unwrap().is_empty());
        assert!(dir.path().join("result.json").exists());
        assert!(events(dir.path()).iter().any(|e| e["event"] == "preflight"));
    }
    // --no-preflight skips the check.
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    mock.health
        .lock()
        .unwrap()
        .push(json!({"ok":false,"problem":"locked"}));
    let mut options = opts(dir.path());
    options.preflight = false;
    assert_eq!(flow::run(&mock, "x", &sample(), options).await["ok"], true);
}

#[tokio::test]
async fn usb_disconnect_mid_run_is_named_and_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    // Preflight healthy, then the cable is pulled during the label tap (batch 2).
    mock.health.lock().unwrap().extend([
        json!({"ok":true,"problem":null}),
        json!({"ok":false,"problem":"missing","state":"missing","hint":"The phone is not attached: check the cable"}),
    ]);
    mock.replies.lock().unwrap().insert(
        2,
        json!({"ok":false,"error":"adb_action_failed","message":"ADB command did not confirm completion; read the screen before continuing",
            "outcome":"unknown","retry_safe":false,"completed":0,"applied_actions":0}),
    );
    let result = flow::run(&mock, "x", &sample(), opts(dir.path())).await;
    assert_eq!(result["failed_step"], 4);
    assert_eq!(result["retry_safe"], false);
    assert_eq!(result["outcome"], "partial");
    assert_eq!(result["device"]["problem"], "missing");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("device_not_attached"),
        "{result}"
    );
    let hint = result["hint"].as_str().unwrap();
    assert!(
        hint.contains("android-use connect") && hint.contains("never replay"),
        "{hint}"
    );
    // No screen reads against a phone that is gone.
    assert_eq!(*mock.elements.lock().unwrap(), 0);
    assert!(!dir.path().join("failure-elements.json").exists());
    assert!(events(dir.path())
        .iter()
        .any(|e| e["event"] == "device_lost"));
    assert_eq!(mock.batches.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn a_wait_that_times_out_on_a_locked_phone_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    mock.health.lock().unwrap().extend([
        json!({"ok":true,"problem":null}),
        json!({"ok":false,"problem":"locked","hint":"unlock it"}),
    ]);
    // Every slice of the wait times out (a locked phone never shows the app).
    for n in 1..1000 {
        mock.replies.lock().unwrap().insert(
            n,
            json!({"ok":false,"error":"condition_timeout","message":"x","outcome":"not_sent","retry_safe":true,"applied_actions":0}),
        );
    }
    let mut d = sample();
    d.steps[1] = json!({"kind":"wait_for","expect":{"app":"com.dice.game"},"timeout_ms":1});
    let result = flow::run(&mock, "x", &d, opts(dir.path())).await;
    assert_eq!(result["failed_step"], 2, "{result}");
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("device_locked"));
    assert!(result["hint"].as_str().unwrap().contains("unlock it"));
    // Not lost: the screen is still captured for debugging.
    assert!(dir.path().join("failure-elements.json").exists());
}

#[tokio::test]
async fn accessibility_backend_skips_adb_diagnosis() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock {
        backend: Some("accessibility"),
        ..Default::default()
    };
    assert_eq!(
        flow::run(&mock, "x", &sample(), opts(dir.path())).await["ok"],
        true
    );
    assert_eq!(*mock.diagnosed.lock().unwrap(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn diagnose_runs_read_only_adb_commands() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls");
    let script = dir.path().join("adb");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> {}\ncase \"$*\" in\n\
  'devices -l') printf 'List of devices attached\\nR5CT1234 device usb:1-1 model:SM_G991B\\nOFF1 unauthorized usb:1-2\\n' ;;\n\
  *'dumpsys power') echo '  mWakefulness=Awake' ;;\n\
  *'dumpsys window policy') echo '    mShowingLockscreen=true' ;;\n\
  *) exit 1 ;;\nesac\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Only this test sets ANDROID_USE_ADB.
    std::env::set_var("ANDROID_USE_ADB", &script);
    let locked = device::diagnose("R5CT1234").await;
    let unauthorized = device::diagnose("OFF1").await;
    let missing = device::diagnose("GONE").await;
    std::env::remove_var("ANDROID_USE_ADB");
    assert_eq!(locked["problem"], "locked", "{locked}");
    assert_eq!(locked["awake"], true);
    assert_eq!(locked["model"], "SM_G991B");
    assert_eq!(unauthorized["problem"], "unauthorized");
    assert_eq!(missing["problem"], "missing");
    let calls = std::fs::read_to_string(log).unwrap();
    for line in calls.lines() {
        assert!(
            line == "devices -l" || line.contains("shell dumpsys"),
            "non read-only call: {line}"
        );
    }
    assert!(
        !calls.contains("OFF1 shell"),
        "never shells into an unauthorized device"
    );
}

// ---------------------------------------------------------------------------
// Preflight codes, daemon versions, lease holder, redaction, trail tagging
// ---------------------------------------------------------------------------

fn healthy_status() -> Value {
    json!({"ok":true,"device":"R5CT1234","app":"com.dice.game","drivable":true,"backend":"adb"})
}

#[test]
fn classify_gives_each_failure_its_own_code_and_next_action() {
    let ok = healthy_status();
    let h = |problem: &str| json!({"ok":false,"problem":problem,"serial":"R5CT1234","hint":device::hint_for(problem)});
    let cases = [
        (
            ok.clone(),
            Some(h("missing")),
            "device_not_attached",
            "cable",
            true,
        ),
        (
            ok.clone(),
            Some(h("unauthorized")),
            "device_unauthorized",
            "Allow USB debugging",
            true,
        ),
        (
            ok.clone(),
            Some(h("offline")),
            "device_offline",
            "replug",
            true,
        ),
        (
            ok.clone(),
            Some(json!({"ok":false,"problem":"asleep","hint":"wake"})),
            "screen_off",
            "wake",
            false,
        ),
        (
            ok.clone(),
            Some(json!({"ok":false,"problem":"locked","hint":"unlock"})),
            "device_locked",
            "unlock",
            false,
        ),
        (
            json!({"ok":false,"error":"daemon_unreachable"}),
            None,
            "daemon_not_running",
            "android-use connect",
            true,
        ),
        (
            json!({"ok":false,"drivable":false,"error":"runner_unreachable","message":"x","device":"R5CT1234"}),
            Some(json!({"ok":true,"problem":null})),
            "runner_not_running",
            "android-use disconnect",
            true,
        ),
        (
            json!({"ok":false,"drivable":false,"error":"control_stopping"}),
            None,
            "daemon_stopping",
            "connect",
            true,
        ),
    ];
    for (status, health, code, next, lost) in cases {
        let v = device::classify(&status, health.as_ref());
        assert_eq!(v["code"], code, "{v}");
        assert!(v["next"].as_str().unwrap().contains(next), "{v}");
        assert_eq!(v["lost"], lost, "{v}");
    }
    let outdated = android_use::mcp::outdated_daemon("/agent/flow/draft");
    let v = device::classify(&outdated, None);
    assert_eq!(v["code"], "daemon_outdated");
    assert!(v["next"].as_str().unwrap().contains(" disconnect && "));
    assert!(v["next"].as_str().unwrap().contains(" connect"));
    assert_eq!(
        device::classify(&ok, Some(&json!({"ok":true,"problem":null})))["ok"],
        true
    );
}

#[tokio::test]
async fn preflight_names_daemon_and_runner_problems() {
    for (status, code) in [
        (
            json!({"ok":false,"error":"daemon_unreachable","message":"Android daemon unavailable"}),
            "daemon_not_running",
        ),
        (
            json!({"ok":false,"drivable":false,"error":"runner_unreachable","message":"r","device":"R5CT1234","backend":"adb"}),
            "runner_not_running",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mock = Mock::default();
        mock.statuses.lock().unwrap().push(status);
        let result = flow::run(&mock, "x", &sample(), opts(dir.path())).await;
        assert_eq!(result["error"], code, "{result}");
        assert!(
            result["hint"].as_str().unwrap().contains("android-use"),
            "{result}"
        );
        assert!(mock.batches.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn disconnect_on_the_first_step_is_still_not_replayable() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    // The daemon died with the USB connection: the request never got an answer.
    mock.replies.lock().unwrap().insert(
        0,
        json!({"ok":false,"error":"daemon_unreachable","message":"Android daemon unavailable","outcome":"not_sent","retry_safe":true,"applied_actions":0}),
    );
    mock.statuses.lock().unwrap().extend([
        healthy_status(),
        json!({"ok":false,"error":"daemon_unreachable"}),
    ]);
    let result = flow::run(&mock, "x", &sample(), opts(dir.path())).await;
    assert_eq!(result["failed_step"], 1);
    assert_eq!(result["device_error"], "daemon_not_running");
    assert_eq!(
        result["retry_safe"], false,
        "a lost device is never auto-replayed"
    );
    assert_eq!(result["outcome"], "partial");
    let hint = result["hint"].as_str().unwrap();
    assert!(
        hint.contains("--from 1") && hint.contains("android-use connect"),
        "{hint}"
    );
    assert_eq!(*mock.elements.lock().unwrap(), 0);
}

#[tokio::test]
async fn timeout_scale_also_stretches_pauses() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    let d = doc(json!({"version":1,"name":"p","steps":[{"kind":"pause","ms":40}]}));
    let started = std::time::Instant::now();
    let options = RunOptions {
        run_dir: Some(dir.path().into()),
        timeout_scale: 3.0,
        ..Default::default()
    };
    assert_eq!(flow::run(&mock, "p", &d, options).await["ok"], true);
    assert!(started.elapsed() >= std::time::Duration::from_millis(120));
}

struct LeakyScreen(Mock);
impl FlowDevice for LeakyScreen {
    async fn actions(&self, batch: Value, record: bool) -> Value {
        let n = self.0.batches.lock().unwrap().len();
        let reply = self.0.actions(batch, record).await;
        if n == 1 {
            json!({"ok":false,"error":"ambiguous_element","message":"m","outcome":"not_sent","retry_safe":true,"applied_actions":0})
        } else {
            reply
        }
    }
    async fn elements(&self) -> Value {
        json!({"ok":true,"snapshot":"s","app":"com.dice.game","elements":[
            {"index":0,"kind":"TextField","label":"Nickname","value":"secret-name","visible":true},
            {"index":1,"kind":"StaticText","label":"Welcome secret-name!","visible":true},
            {"index":2,"kind":"TextField","label":"PIN","value":"1234","password":true,"visible":true}]})
    }
    async fn status(&self) -> Value {
        self.0.status().await
    }
    async fn screenshot(&self) -> Result<Vec<u8>, String> {
        self.0.screenshot().await
    }
    async fn diagnose(&self, serial: &str) -> Value {
        self.0.diagnose(serial).await
    }
}

#[tokio::test]
async fn failure_captures_redact_typed_inputs_and_field_values() {
    let dir = tempfile::tempdir().unwrap();
    let device = LeakyScreen(Mock::default());
    let d = doc(json!({"version":1,"name":"r","inputs":{"nick":{}},"steps":[
        {"kind":"type","input":"nick","clear":true},{"kind":"tap","label":"Go"}]}));
    let result = flow::run(&device, "r", &d, opts(dir.path())).await;
    assert_eq!(result["failed_step"], 2, "{result}");
    let saved = std::fs::read_to_string(dir.path().join("failure-elements.json")).unwrap();
    assert!(
        !saved.contains("secret-name") && !saved.contains("1234"),
        "{saved}"
    );
    assert!(saved.contains("<input:nick>") && saved.contains("<redacted>"));
    assert!(!result.to_string().contains("secret-name"));
    let all = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "json" || x == "jsonl")
        })
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(!all.contains("secret-name"));
}

#[tokio::test]
async fn flow_batches_stay_out_of_the_draft_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    flow::run(&mock, "x", &sample(), opts(dir.path())).await;
    assert!(mock.trail_flags.lock().unwrap().iter().all(|f| !f));
    let mock = Mock::default();
    let mut options = opts(dir.path());
    options.record_trail = true;
    flow::run(&mock, "x", &sample(), options).await;
    assert!(mock.trail_flags.lock().unwrap().iter().all(|f| *f));
}

#[tokio::test]
async fn daemon_skips_tagged_flow_batches_names_the_lease_holder_and_old_daemons_are_explained() {
    let game = Game::default();
    let (url, server) = serve(game.clone()).await;
    let a = DaemonClient::new(url.clone(), "fixture-token".into(), "agent-a".into()).unwrap();
    let d = doc(json!({"version":1,"name":"s","steps":[{"kind":"tap","label":"Play"}]}));
    let dir = tempfile::tempdir().unwrap();
    let options = RunOptions {
        run_dir: Some(dir.path().into()),
        preflight: false,
        ..Default::default()
    };
    assert_eq!(flow::run(&a, "s", &d, options).await["ok"], true);
    assert_eq!(
        a.request("/agent/flow/draft", None).await["error"],
        "empty_trail"
    );
    // Same owner, recorded on request.
    *game.in_match.lock().unwrap() = false;
    let options = RunOptions {
        run_dir: Some(dir.path().into()),
        preflight: false,
        record_trail: true,
        ..Default::default()
    };
    flow::run(&a, "s", &d, options).await;
    assert_eq!(a.request("/agent/flow/draft", None).await["steps"], 1);
    // Another owner hits the lease: the error says who holds it and how to free it.
    let b = DaemonClient::new(url.clone(), "fixture-token".into(), "agent-b".into()).unwrap();
    let busy = b.request("/agent/elements", None).await;
    assert_eq!(busy["error"], "device_busy");
    let message = busy["message"].as_str().unwrap();
    assert!(
        message.contains("'agent-a'") && message.contains("android-use --owner agent-a release"),
        "{message}"
    );
    let busy = flow::run(
        &b,
        "s",
        &d,
        RunOptions {
            run_dir: Some(dir.path().into()),
            preflight: false,
            ..Default::default()
        },
    )
    .await;
    assert!(
        busy["message"].as_str().unwrap().contains("agent-a"),
        "{busy}"
    );
    server.abort();

    // A daemon without the flow routes (older version) answers 404.
    let old = axum::Router::new().route(
        "/agent/status",
        axum::routing::get(|| async { axum::Json(json!({"ok":true})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_url = format!("http://{}", listener.local_addr().unwrap());
    let old_server = tokio::spawn(async move { axum::serve(listener, old).await.unwrap() });
    let client = DaemonClient::new(old_url, "fixture-token".into(), "agent-a".into()).unwrap();
    let draft = flow::finish_draft(client.request("/agent/flow/draft", None).await, None);
    assert_eq!(draft["error"], "daemon_outdated");
    let message = draft["message"].as_str().unwrap();
    assert!(
        message.starts_with("daemon is older than this binary")
            && message.contains("restart with: ")
            && message.contains("disconnect && ")
            && message.contains(" connect"),
        "{message}"
    );
    assert!(!message.contains("did not return JSON"));
    old_server.abort();
}
