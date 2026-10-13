use phone_control::{
    Action, BackendFuture, Batch, Controller, DeviceBackend, Dispatch, Element, Failure, Screen,
};
use serde_json::{json, Value};
use std::collections::VecDeque;

fn screen() -> Screen {
    Screen {
        device: "fixture-phone".into(),
        app: "fixture.app".into(),
        width: 1080.0,
        height: 1920.0,
        elements: vec![Element {
            kind: "TextField".into(),
            label: "".into(),
            identifier: Some("field".into()),
            rect: [10.0, 30.0, 400.0, 80.0],
            depth: 1,
            value: Some("旧内容".into()),
            enabled: true,
            visible: true,
            focused: true,
            password: false,
        }],
    }
}
struct Mock {
    native_typing: bool,
    reads: usize,
    screens: VecDeque<Result<Screen, Failure>>,
    actions: Vec<Dispatch>,
    fail_at: Option<usize>,
    slow_after: Option<usize>,
    stop_after_dispatch: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
impl Mock {
    fn new() -> Self {
        Self {
            native_typing: false,
            reads: 0,
            screens: VecDeque::new(),
            actions: vec![],
            fail_at: None,
            slow_after: None,
            stop_after_dispatch: None,
        }
    }
}
impl DeviceBackend for Mock {
    fn native_typing(&self) -> bool {
        self.native_typing
    }
    fn platform(&self) -> &'static str {
        "fixture"
    }
    fn read(&mut self) -> BackendFuture<'_, Screen> {
        Box::pin(async move {
            self.reads += 1;
            if self.slow_after.is_some_and(|n| self.reads > n) {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            self.screens.pop_front().unwrap_or_else(|| Ok(screen()))
        })
    }
    fn dispatch(&mut self, action: Dispatch) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.actions.push(action);
            if let Some(stop) = &self.stop_after_dispatch {
                stop.store(true, std::sync::atomic::Ordering::Release);
            }
            if self.fail_at == Some(self.actions.len()) {
                Err(Failure::unknown("lost_ack", "Synthetic lost response"))
            } else {
                Ok(())
            }
        })
    }
    fn screenshot(&mut self) -> BackendFuture<'_, Vec<u8>> {
        Box::pin(async { Ok(vec![]) })
    }
}
fn batch(steps: Value) -> Batch {
    serde_json::from_value(json!({"steps":steps,"observe":false})).unwrap()
}

#[tokio::test]
async fn invalid_later_step_prevents_every_mutation_and_read() {
    let mut control = Controller::new(Mock::new());
    let result = control
        .execute(batch(json!([{"type":"home"},{"type":"tap","x":4,"y":0.5}])))
        .await;
    assert_eq!(result["outcome"], "not_sent");
    assert_eq!(control.backend.reads, 0);
    assert!(control.backend.actions.is_empty());
}
#[tokio::test]
async fn an_uncertain_mutation_is_never_replayed_and_stops_the_batch() {
    let mut backend = Mock::new();
    backend.fail_at = Some(2);
    let mut control = Controller::new(backend);
    let result = control
        .execute(batch(
            json!([{"type":"home"},{"type":"back"},{"type":"home"}]),
        ))
        .await;
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(result["completed"], 1);
    assert_eq!(result["failed_step"], 1);
    assert_eq!(result["retry_safe"], false);
    assert_eq!(control.backend.actions.len(), 2);
}
#[tokio::test]
async fn applied_action_stays_applied_when_observation_fails() {
    let mut backend = Mock::new();
    backend.screens.push_back(Ok(screen()));
    backend.screens.push_back(Err(Failure::not_sent(
        "read_failed",
        "Synthetic failed read",
    )));
    let mut control = Controller::new(backend);
    let result = control
        .execute(serde_json::from_value(json!({"steps":[{"type":"home"}]})).unwrap())
        .await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["outcome"], "applied");
    assert_eq!(result["completed"], 1);
    assert_eq!(result["retry_safe"], false);
    assert_eq!(result["settle"]["reason"], "read_failed");
    assert_eq!(control.backend.actions.len(), 1);
}
#[tokio::test]
async fn stale_snapshot_sends_nothing_including_after_rotation() {
    let old = screen().snapshot().unwrap();
    let mut rotated = screen();
    rotated.width = 1920.0;
    rotated.height = 1080.0;
    let mut backend = Mock::new();
    backend.screens.push_back(Ok(rotated));
    let mut control = Controller::new(backend);
    let result = control
        .execute(batch(json!([{"type":"tap","element":0,"snapshot":old}])))
        .await;
    assert_eq!(result["error"], "stale_snapshot");
    assert_eq!(result["retry_safe"], true);
    assert!(control.backend.actions.is_empty());
}
#[tokio::test]
async fn refusal_after_an_applied_step_does_not_allow_batch_replay() {
    let mut control = Controller::new(Mock::new());
    let result = control
        .execute(batch(
            json!([{"type":"home"},{"type":"tap","element":0,"snapshot":"old"}]),
        ))
        .await;
    assert_eq!(result["completed"], 1);
    assert_eq!(result["outcome"], "not_sent");
    assert_eq!(result["retry_safe"], false);
    assert_eq!(control.backend.actions.len(), 1);
}
#[tokio::test]
async fn unicode_text_is_resolved_without_host_input_or_clipboard() {
    let mut control = Controller::new(Mock::new());
    let result = control
        .execute(batch(
            json!([{"type":"type","text":"你好 👩‍💻\n日本語","clear":true}]),
        ))
        .await;
    assert_eq!(result["ok"], true);
    assert!(
        matches!(&control.backend.actions[0],Dispatch::SetText{text,..} if text=="你好 👩‍💻\n日本語")
    );
}
#[tokio::test]
async fn disabled_or_ambiguous_elements_do_not_dispatch() {
    for ambiguous in [false, true] {
        let mut s = screen();
        s.elements[0].label = "Save".into();
        if ambiguous {
            s.elements.push(s.elements[0].clone());
        } else {
            s.elements[0].enabled = false;
        }
        let mut backend = Mock::new();
        backend.screens.push_back(Ok(s.clone()));
        let mut control = Controller::new(backend);
        let steps = if ambiguous {
            json!([{"type":"tap","label":"Save"}])
        } else {
            json!([{"type":"tap","element":0,"snapshot":s.snapshot().unwrap()}])
        };
        assert_eq!(control.execute(batch(steps)).await["outcome"], "not_sent");
        assert!(control.backend.actions.is_empty());
    }
}
#[tokio::test]
async fn identical_reads_return_an_empty_delta_and_unknown_baseline_returns_full_tree() {
    let mut control = Controller::new(Mock::new());
    let first = control.elements(None).await.unwrap();
    let next = control.elements(first["snapshot"].as_str()).await.unwrap();
    assert_eq!(next["elements"], json!([]));
    assert_eq!(next["delta"]["unchanged"], 1);
    let full = control.elements(Some("unknown")).await.unwrap();
    assert_eq!(full["elements"][0]["index"], 0);
    assert!(full.get("delta").is_none());
}
#[test]
fn unknown_actions_and_parameters_are_rejected() {
    for step in [
        json!({"type":"shell","command":"unsafe"}),
        json!({"type":"tap","x":0.5,"y":0.5,"force":true}),
    ] {
        assert!(serde_json::from_value::<Action>(step).is_err());
    }
}

#[tokio::test]
async fn observation_timeout_respects_the_budget_without_replaying() {
    let mut backend = Mock::new();
    backend.slow_after = Some(1);
    let mut control = Controller::new(backend);
    let start = std::time::Instant::now();
    let result = control
        .execute(serde_json::from_value(json!({"steps":[{"type":"home"}],"settle_ms":30})).unwrap())
        .await;
    assert_eq!(result["outcome"], "applied");
    assert_eq!(result["settle"]["reason"], "budget_exhausted");
    assert!(start.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(control.backend.actions.len(), 1);
}
#[tokio::test]
async fn launch_does_not_report_the_previous_app_as_a_settled_result() {
    let mut control = Controller::new(Mock::new());
    let result = control
        .execute(
            serde_json::from_value(
                json!({"steps":[{"type":"launch_app","app":"different.app"}],"settle_ms":300}),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(result["outcome"], "applied");
    assert_eq!(result["settle"]["settled"], false);
    assert_eq!(result["observation"]["app"], "fixture.app");
}
#[tokio::test]
async fn repeated_settled_unchanged_actions_share_the_iphone_progress_advice() {
    let mut s = screen();
    s.elements.push(s.elements[0].clone());
    s.elements.push(s.elements[0].clone());
    let mut backend = Mock::new();
    // Enough reads for two settled observations; all use the same full tree.
    for _ in 0..12 {
        backend.screens.push_back(Ok(s.clone()));
    }
    let mut control = Controller::new(backend);
    control.elements(None).await.unwrap();
    let steps = json!([{"type":"tap","x":0.5,"y":0.5}]);
    let first = control
        .execute(serde_json::from_value(json!({"steps":steps,"settle_ms":600})).unwrap())
        .await;
    assert!(first.get("advice").is_none());
    let second = control
        .execute(serde_json::from_value(json!({"steps":steps,"settle_ms":600})).unwrap())
        .await;
    assert!(second["advice"].as_str().unwrap().contains("no_progress"));
    assert_eq!(control.backend.actions.len(), 2);
}

#[tokio::test]
async fn navigation_batch_does_not_require_a_tree_between_each_transition() {
    let mut backend = Mock::new();
    backend.screens.push_back(Err(Failure::not_sent(
        "screen_changed_during_read",
        "Synthetic transition",
    )));
    let mut control = Controller::new(backend);
    let result = control
        .execute(batch(
            json!([{"type":"back"},{"type":"home"},{"type":"launch_app","app":"fixture.app"}]),
        ))
        .await;
    assert_eq!(result["completed"], 3);
    assert_eq!(result["outcome"], "applied");
    assert_eq!(control.backend.reads, 0);
}

#[tokio::test]
async fn shutdown_stops_the_remaining_batch_and_does_not_allow_replay() {
    let mut control = Controller::new(Mock::new());
    control.backend.stop_after_dispatch = Some(control.stopping.clone());
    let result = control
        .execute(batch(json!([{"type":"home"},{"type":"back"}])))
        .await;
    assert_eq!(result["error"], "control_stopping");
    assert_eq!(result["completed"], 1);
    assert_eq!(result["retry_safe"], false);
    assert_eq!(control.backend.actions.len(), 1);
}
#[tokio::test]
async fn stopping_daemon_dispatches_no_new_action() {
    let mut control = Controller::new(Mock::new());
    control
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
    let result = control.execute(batch(json!([{"type":"home"}]))).await;
    assert_eq!(result["outcome"], "not_sent");
    assert_eq!(result["retry_safe"], true);
    assert!(control.backend.actions.is_empty());
}

#[tokio::test]
async fn status_polling_does_not_replace_the_owners_last_observation_baseline() {
    let mut control = Controller::new(Mock::new());
    let old = control.elements(None).await.unwrap();
    let mut changed = screen();
    changed.elements[0].value = Some("changed".into());
    for _ in 0..6 {
        control.backend.screens.push_back(Ok(changed.clone()));
    }
    assert_eq!(control.status().await["drivable"], true);
    let result = control
        .execute(
            serde_json::from_value(json!({"steps":[{"type":"home"}],"settle_ms":600})).unwrap(),
        )
        .await;
    assert_eq!(result["observation"]["baseline"], old["snapshot"]);
    assert_ne!(result["observation"]["snapshot"], old["snapshot"]);
}

#[tokio::test]
async fn native_input_is_dispatched_once_without_stale_host_text() {
    let mut backend = Mock::new();
    backend.native_typing = true;
    let mut control = Controller::new(backend);
    let result = control
        .execute(batch(
            json!([{"type":"type","text":"日本語 🦊","clear":false}]),
        ))
        .await;
    assert_eq!(result["ok"], true);
    assert_eq!(control.backend.reads, 0);
    assert_eq!(control.backend.actions.len(), 1);
    assert!(
        matches!(&control.backend.actions[0], Dispatch::NativeType{text,clear:false} if text == "日本語 🦊")
    );
}
#[tokio::test]
async fn wait_only_is_read_only_and_does_not_inflate_applied_actions() {
    let mut backend = Mock::new();
    let mut wrong = screen();
    wrong.app = "old.app".into();
    backend.screens.push_back(Ok(wrong));
    let mut control = Controller::new(backend);
    let result = control.execute(batch(json!([{"type":"wait_for","expect":{"app":"fixture.app","identifier":"field","value":"旧内容"},"timeout_ms":300}]))).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["outcome"], "not_sent");
    assert_eq!(result["completed"], 1);
    assert_eq!(result["applied_actions"], 0);
    assert_eq!(control.backend.reads, 2);
    assert!(control.backend.actions.is_empty());
}
#[tokio::test]
async fn wait_timeout_after_mutation_stops_batch_without_allowing_replay() {
    let mut control = Controller::new(Mock::new());
    let result = control.execute(batch(json!([{"type":"home"},{"type":"wait_for","expect":{"label":"Missing"},"timeout_ms":10},{"type":"back"}]))).await;
    assert_eq!(result["error"], "condition_timeout");
    assert_eq!(result["retry_safe"], false);
    assert_eq!(result["completed"], 1);
    assert_eq!(result["applied_actions"], 1);
    assert_eq!(control.backend.actions.len(), 1);
}
#[tokio::test]
async fn empty_wait_or_invalid_timeout_refuses_entire_batch() {
    for wait in [
        json!({"type":"wait_for","expect":{}}),
        json!({"type":"wait_for","expect":{"app":"fixture.app"},"timeout_ms":0}),
    ] {
        let mut control = Controller::new(Mock::new());
        let result = control.execute(batch(json!([{"type":"home"},wait]))).await;
        assert_eq!(result["outcome"], "not_sent");
        assert_eq!(control.backend.reads, 0);
        assert!(control.backend.actions.is_empty());
    }
}

#[tokio::test]
async fn stopping_status_never_reports_a_dying_daemon_as_ready() {
    let mut control = Controller::new(Mock::new());
    control
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
    let result = control.status().await;
    assert_eq!(result["drivable"], false);
    assert_eq!(result["error"], "control_stopping");
    assert_eq!(control.backend.reads, 0);
}
