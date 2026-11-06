use android_use::{accessibility::Bridge, backend::AndroidBackend};
use android_use_phone::Pairing;
use base64::{engine::general_purpose::STANDARD, Engine};
use phone_control::{DeviceBackend, Dispatch};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

async fn fixture() -> (
    Pairing,
    Bridge,
    axum_server::Handle,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let pair = Pairing {
        version: 1,
        endpoint: format!("wss://{}/phone", listener.local_addr().unwrap()),
        device_id: "a".repeat(32),
        token: "b".repeat(64),
        certificate: STANDARD.encode(cert.cert.der()),
        fingerprint: format!("{:x}", Sha256::digest(cert.cert.der())),
    };
    pair.validate().unwrap();
    let bridge = Bridge::new(pair.clone());
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.cert.pem().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let handle = axum_server::Handle::new();
    let h = handle.clone();
    let router = bridge.router();
    let task = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .handle(h)
            .serve(router.into_make_service())
            .await
    });
    (pair, bridge, handle, task)
}
fn response(text: String) -> String {
    let request: Value = serde_json::from_str(&text).unwrap();
    let result = match request["method"].as_str().unwrap() {
        "screen" => json!({"device":"","app":"fixture.app","width":100,"height":200,"elements":[]}),
        "typeText" => json!({"applied":true,"verified":true,"redacted":false}),
        "launchApp" | "click" | "recents" => json!(true),
        _ => json!(false),
    };
    json!({"result":result}).to_string()
}
async fn wait_ready(backend: &mut AndroidBackend) {
    for _ in 0..100 {
        if backend.read().await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("TLS phone failed to connect");
}
#[tokio::test]
async fn paired_tls_reads_and_dispatches_without_adb() {
    let (pair, bridge, handle, server) = fixture().await;
    let stop = Arc::new(AtomicBool::new(false));
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let client = tokio::spawn(android_use_phone::run(
        pair.clone(),
        stop.clone(),
        Arc::new(move |text| {
            c.fetch_add(1, Ordering::SeqCst);
            response(text)
        }),
    ));
    let mut backend = AndroidBackend::from_accessibility(pair.device_id.clone(), bridge).unwrap();
    assert!(backend.native_typing());
    wait_ready(&mut backend).await;
    let screen = backend.read().await.unwrap();
    assert_eq!(screen.device, pair.device_id);
    let before = count.load(Ordering::SeqCst);
    backend
        .dispatch(Dispatch::NativeType {
            text: "日本🙂".into(),
            clear: true,
        })
        .await
        .unwrap();
    backend
        .dispatch(Dispatch::LaunchApp("fixture.app".into()))
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), before + 2);
    stop.store(true, Ordering::Release);
    backend.close().await;
    handle.shutdown();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn wrong_token_never_reaches_native_dispatch() {
    let (mut pair, bridge, handle, server) = fixture().await;
    pair.token = "c".repeat(64);
    let stop = Arc::new(AtomicBool::new(false));
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let client = tokio::spawn(android_use_phone::run(
        pair.clone(),
        stop.clone(),
        Arc::new(move |text| {
            c.fetch_add(1, Ordering::SeqCst);
            response(text)
        }),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut backend = AndroidBackend::from_accessibility(pair.device_id, bridge).unwrap();
    let error = backend.dispatch(Dispatch::Tap(1., 2.)).await.unwrap_err();
    assert_eq!(error.error, "phone_disconnected");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    stop.store(true, Ordering::Release);
    handle.shutdown();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn pinned_certificate_rejects_replacement() {
    let (mut pair, _bridge, handle, server) = fixture().await;
    pair.fingerprint = "0".repeat(64);
    assert!(pair.validate().is_err());
    handle.shutdown();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn lost_ack_is_unknown_and_never_replayed_on_reconnect() {
    let (pair, bridge, handle, server) = fixture().await;
    let stop = Arc::new(AtomicBool::new(false));
    let mutations = Arc::new(AtomicUsize::new(0));
    let count = mutations.clone();
    let client = tokio::spawn(android_use_phone::run(
        pair.clone(),
        stop.clone(),
        Arc::new(move |text| {
            let call: Value = serde_json::from_str(&text).unwrap();
            if call["method"] == "click" {
                count.fetch_add(1, Ordering::SeqCst);
                // Device applies once but misses the host's acknowledgement window.
                std::thread::sleep(std::time::Duration::from_secs(13));
            }
            response(text)
        }),
    ));
    let mut backend = AndroidBackend::from_accessibility(pair.device_id, bridge).unwrap();
    wait_ready(&mut backend).await;
    let failure = backend.dispatch(Dispatch::Tap(10., 20.)).await.unwrap_err();
    assert_eq!(failure.error, "phone_ack_lost");
    assert_eq!(failure.json()["outcome"], "unknown");
    assert_eq!(failure.json()["retry_safe"], false);
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    wait_ready(&mut backend).await;
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
    stop.store(true, Ordering::Release);
    backend.close().await;
    handle.shutdown();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_different_tls_certificate_never_reaches_native_dispatch() {
    let (mut pair, bridge, handle, server) = fixture().await;
    let replacement = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    pair.certificate = STANDARD.encode(replacement.cert.der());
    pair.fingerprint = format!("{:x}", Sha256::digest(replacement.cert.der()));
    // The stored pin is internally valid, but the server presents another leaf.
    pair.validate().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let client = tokio::spawn(android_use_phone::run(
        pair.clone(),
        stop.clone(),
        Arc::new(move |text| {
            c.fetch_add(1, Ordering::SeqCst);
            response(text)
        }),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut backend = AndroidBackend::from_accessibility(pair.device_id, bridge).unwrap();
    assert_eq!(
        backend.read().await.unwrap_err().error,
        "phone_disconnected"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    stop.store(true, Ordering::Release);
    handle.shutdown();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn device_commands_open_recents_and_refuse_force_stop_without_adb() {
    use android_use::http::{router, ApiState, Session};
    let (pair, bridge, handle, server) = fixture().await;
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let s = seen.clone();
    let client = tokio::spawn(android_use_phone::run(
        pair.clone(),
        stop.clone(),
        Arc::new(move |text| {
            let request: Value = serde_json::from_str(&text).unwrap();
            s.lock()
                .unwrap()
                .push(request["method"].as_str().unwrap().into());
            response(text)
        }),
    ));
    let mut backend =
        AndroidBackend::from_accessibility(pair.device_id.clone(), bridge.clone()).unwrap();
    wait_ready(&mut backend).await;
    let api = router(
        ApiState::new(
            Arc::new(tokio::sync::Mutex::new(Session::new(backend))),
            "fixture-token".into(),
        )
        .with_phone_bridge(Some(bridge))
        .with_backend("accessibility"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/agent/device", listener.local_addr().unwrap());
    let daemon = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let post = |body: Value| {
        let url = url.clone();
        async move {
            reqwest::Client::new()
                .post(url)
                .bearer_auth("fixture-token")
                .json(&body)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let value = post(json!({"command":"recents"})).await;
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(seen.lock().unwrap().last().unwrap(), "recents");
    let before = seen.lock().unwrap().len();
    for command in ["force_stop", "restart_app"] {
        let value = post(json!({"command":command,"package":"a.b"})).await;
        assert_eq!(value["error"], "adb_required", "{value}");
        assert_eq!(value["outcome"], "not_sent", "{value}");
        assert!(value["message"].as_str().unwrap().contains("ADB"));
    }
    let value = post(json!({"command":"force_stop","package":"a.b;reboot"})).await;
    assert_eq!(value["error"], "invalid_command");
    let value = post(json!({"command":"reboot"})).await;
    assert_eq!(value["error"], "invalid_command");
    assert_eq!(seen.lock().unwrap().len(), before);
    daemon.abort();
    stop.store(true, Ordering::Release);
    handle.shutdown();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}
