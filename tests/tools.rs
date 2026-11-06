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

#[test]
fn logcat_filters_become_safe_arguments() {
    use android_use::tools::logcat_args;
    let tags = vec!["MyTag".to_string()];
    assert_eq!(
        logcat_args(&tags, Some(7), Some("1793000000.5"), 10).unwrap(),
        [
            "logcat",
            "-d",
            "--pid=7",
            "-T",
            "1793000000.5",
            "-s",
            "MyTag"
        ]
    );
    assert_eq!(
        logcat_args(&[], None, None, 10).unwrap(),
        ["logcat", "-d", "-t", "10"]
    );
    assert!(logcat_args(&["a;rm".to_string()], None, None, 1).is_err());
    assert!(logcat_args(&[], None, Some("-f /sdcard/x"), 1).is_err());
}
#[test]
fn package_dumps_are_parsed() {
    use android_use::tools::{parse_package, parse_packages, valid_package};
    let dump = "Packages:\n  Package [a.b] (1):\n    versionCode=42 minSdk=26 targetSdk=34\n    versionName=1.2.3\n    firstInstallTime=2026-01-02 03:04:05\n    lastUpdateTime=2026-02-03 04:05:06\n    installerPackageName=com.android.vending\n    signatures=PackageSignatures{7d1 version:2, signatures:[e1f2a3b4], past signatures:[]}\n";
    let info = parse_package("a.b", dump).unwrap();
    assert_eq!(info["version_code"], 42);
    assert_eq!(info["version_name"], "1.2.3");
    assert_eq!(info["first_install_time"], "2026-01-02 03:04:05");
    assert_eq!(info["last_update_time"], "2026-02-03 04:05:06");
    assert!(info["label"].is_null());
    assert_eq!(info["installer"], "com.android.vending");
    assert_eq!(info["signatures"], json!(["e1f2a3b4"]));
    let bare = parse_package("a.b", "versionCode=1\n    installerPackageName=null\n").unwrap();
    assert!(bare["installer"].is_null());
    assert_eq!(bare["signatures"], json!([]));
    assert!(parse_package("a.b", "Unable to find package: a.b\n").is_none());
    let apps = parse_packages("package:a.b versionCode:7\npackage:c.d\njunk\n");
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0]["version_code"], 7);
    assert!(apps[1]["version_code"].is_null());
    assert!(valid_package("com.example_app.x"));
    assert!(!valid_package("noDot"));
    assert!(!valid_package("a.b;reboot"));
}
#[tokio::test]
async fn lease_route_refuses_a_second_owner() {
    let (url, task) = daemon().await;
    assert_eq!(post(&url, "/agent/lease", "a", json!({})).await["ok"], true);
    let other = post(&url, "/agent/lease", "b", json!({})).await;
    assert_eq!(other["error"], "device_busy");
    task.abort();
}

#[cfg(unix)]
mod fake_adb {
    use serde_json::{json, Value};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const SCRIPT: &str = r#"#!/bin/sh
echo "$*" >> "$ADB_LOG"
case "$*" in
  "devices -l") printf 'List of devices attached\nS1 device usb:1 model:X\n' ;;
  *"monkey"*) echo "Events injected: 1" ;;
  *"dumpsys package a.b"*) printf 'versionCode=9 minSdk=1\nversionName=2.0\n' ;;
  *"pm path a.b"*) echo "package:/data/app/a.b/base.apk" ;;
  *"pidof a.b"*) echo 123 ;;
  *"logcat"*) echo "line one" ;;
esac
"#;
    async fn cli(home: &Path, args: &[&str]) -> (Value, String) {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_android-use"))
            .args(args)
            .env("HOME", home)
            .env("ANDROID_USE_ADB", home.join("adb"))
            .env("ADB_LOG", home.join("adb.log"))
            .output()
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let log = std::fs::read_to_string(home.join("adb.log")).unwrap_or_default();
        (serde_json::from_str(&text).unwrap_or(json!(null)), log)
    }
    #[tokio::test]
    async fn commands_reach_adb_with_the_expected_arguments() {
        let home = tempfile::tempdir().unwrap();
        let adb = home.path().join("adb");
        std::fs::write(&adb, SCRIPT).unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (value, log) = cli(home.path(), &["adb", "recents"]).await;
        assert_eq!(value["ok"], true);
        assert!(log.contains("-s S1 shell input keyevent KEYCODE_APP_SWITCH"));
        let (value, log) = cli(home.path(), &["adb", "restart", "a.b"]).await;
        assert_eq!(value["restarted"], true);
        let stop = log.find("am force-stop a.b").unwrap();
        assert!(stop < log.find("monkey -p a.b").unwrap());
        let (value, _) = cli(home.path(), &["adb", "package", "a.b"]).await;
        assert_eq!(value["version_name"], "2.0");
        assert_eq!(value["running"], true);
        assert_eq!(value["apk_paths"][0], "/data/app/a.b/base.apk");
        let (value, _) = cli(home.path(), &["adb", "logcat", "--tag", "T"]).await;
        assert_eq!(value["log"], "line one\n");
        let (value, log) = cli(home.path(), &["adb", "force-stop", "a.b;reboot"]).await;
        assert_eq!(value["error"], "invalid_package");
        assert!(!log.contains("reboot"));
    }
    #[tokio::test]
    async fn no_adb_device_explains_the_requirement() {
        let home = tempfile::tempdir().unwrap();
        let (value, _) = cli(home.path(), &["adb", "recents"]).await;
        assert_eq!(value["error"], "adb_required");
    }
}
