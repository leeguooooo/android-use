use android_use::{
    backend::AndroidBackend,
    http::{router, ApiState, Session},
    tools::{action_problem, batch_problem},
};
use serde_json::{json, Value};
use std::sync::Arc;

async fn daemon() -> (String, tokio::task::JoinHandle<()>) {
    let backend = AndroidBackend::from_endpoint("fixture".into(), "http://127.0.0.1:9".into());
    let api = router(ApiState::new(
        Arc::new(tokio::sync::Mutex::new(Session::new(backend.unwrap()))),
        "fixture-token".into(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    (url, task)
}
async fn post(url: &str, path: &str, owner: &str, body: Value) -> Value {
    reqwest::Client::new()
        .post(format!("{url}{path}"))
        .bearer_auth("fixture-token")
        .header("X-Phone-Owner", owner)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[test]
fn invalid_actions_name_the_problem() {
    let problem = |value| action_problem(&value).unwrap();
    assert!(problem(json!({"action":"tap"})).contains("\"type\""));
    assert!(problem(json!({"x":0.5})).contains("missing"));
    assert!(problem(json!({"type":"teleport"})).contains("teleport"));
    assert!(problem(json!({"type":"swipe","x1":0.1,"y1":0.1,"x2":0.1})).contains("y2"));
    assert!(problem(json!({"type":"longpress","x":0.1,"y":0.1,"extra":1})).contains("extra"));
    assert!(problem(json!({"type":"scroll","direction":"up"})).contains("dy"));
    assert!(problem(json!({"type":"swipe","start":{},"end":{}})).contains("x1"));
    assert!(problem(json!(["tap"])).contains("object"));
    assert_eq!(action_problem(&json!({"type":"home"})), None);
}
#[test]
fn batch_errors_name_the_first_bad_step() {
    let body = json!({"steps":[{"type":"home"},{"type":"nope"}]});
    assert!(batch_problem(&body, "fallback").starts_with("step 2: "));
    assert_eq!(batch_problem(&json!({"stepz":[]}), "fallback"), "fallback");
}
#[test]
fn every_serde_action_is_documented() {
    let schema = android_use::tools::action_schema();
    for action in [
        json!({"type":"tap","x":0.5,"y":0.5}),
        json!({"type":"longpress","x":0.5,"y":0.5}),
        json!({"type":"swipe","x1":0.5,"y1":0.5,"x2":0.1,"y2":0.1}),
        json!({"type":"scroll","x":0.5,"y":0.5,"dx":0,"dy":0.1}),
        json!({"type":"type","text":"a"}),
        json!({"type":"back"}),
        json!({"type":"home"}),
        json!({"type":"launch_app","app":"a.b"}),
        json!({"type":"wait_for","expect":{"app":"a.b"}}),
    ] {
        assert_eq!(action_problem(&action), None, "{action}");
        let name = action["type"].as_str().unwrap();
        assert!(schema["actions"].get(name).is_some(), "{name}");
        assert!(android_use::tools::ACT_HELP.contains(&format!("\"type\":\"{name}\"")));
    }
}
#[tokio::test]
async fn http_rejections_carry_the_detail() {
    let (url, task) = daemon().await;
    let result = post(&url, "/agent/input", "a", json!({"type":"swipe","x1":0.1})).await;
    assert_eq!(result["error"], "invalid_action");
    assert_eq!(result["outcome"], "not_sent");
    assert!(
        result["message"].as_str().unwrap().contains("y1"),
        "{result}"
    );
    let result = post(
        &url,
        "/agent/actions",
        "a",
        json!({"steps":[{"action":"tap"}]}),
    )
    .await;
    assert_eq!(result["error"], "invalid_batch");
    assert!(
        result["message"].as_str().unwrap().contains("step 1"),
        "{result}"
    );
    task.abort();
}

fn screen(rows: Value) -> Value {
    json!({"app":"com.game","screen":{"height":2000.0},"elements":rows})
}
#[test]
fn canvas_hint_ignores_system_bars_and_real_trees() {
    use android_use::tools::annotate_sparse_tree as annotate;
    let bar = json!({"label":"Battery","rect":[0.0,0.0,100.0,60.0]});
    let nav = json!({"label":"Back","rect":[0.0,1900.0,100.0,100.0]});
    let body = json!({"label":"Save","identifier":"","rect":[0.0,900.0,100.0,100.0]});
    let unnamed = json!({"label":"","rect":[0.0,900.0,100.0,100.0]});
    let mut canvas = screen(json!([bar, nav, unnamed]));
    annotate(&mut canvas);
    assert_eq!(canvas["canvas_rendered"], true);
    assert!(canvas["hint"].as_str().unwrap().contains("--max-width"));
    let mut tree = screen(json!([body, body]));
    annotate(&mut tree);
    assert!(tree.get("canvas_rendered").is_none());
    let mut delta = screen(json!([]));
    delta["delta"] = json!({});
    annotate(&mut delta);
    assert!(delta.get("canvas_rendered").is_none());
    let mut launcher = screen(json!([]));
    launcher["app"] = json!("com.sec.android.app.launcher");
    annotate(&mut launcher);
    assert!(launcher.get("canvas_rendered").is_none());
}
#[test]
fn screenshots_shrink_and_crop() {
    use android_use::tools::resize_png;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(1000, 2000)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let size = |png: Vec<u8>| {
        let image = image::load_from_memory(&png).unwrap();
        (image.width(), image.height())
    };
    assert_eq!(
        size(resize_png(&bytes, Some(500), None).unwrap()),
        (500, 1000)
    );
    assert_eq!(
        size(resize_png(&bytes, None, Some("0,0.5,1,0.5")).unwrap()),
        (1000, 1000)
    );
    assert_eq!(
        size(resize_png(&bytes, Some(2000), None).unwrap()),
        (1000, 2000)
    );
    assert!(resize_png(&bytes, None, Some("0,0,2,1")).is_err());
    assert!(resize_png(&bytes, None, Some("0,0,1")).is_err());
    assert!(resize_png(&bytes, Some(0), None).is_err());
}
