//! On-phone protocol and TLS client. Android framework calls live behind JNI.
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message},
    Connector,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pairing {
    pub version: u8,
    pub endpoint: String,
    pub device_id: String,
    pub token: String,
    pub certificate: String,
    pub fingerprint: String,
}
impl Pairing {
    pub fn validate(&self) -> Result<rustls::ClientConfig, String> {
        let url = self.endpoint.strip_prefix("wss://").ok_or("TLS required")?;
        if !url.ends_with("/phone")
            || url.contains('@')
            || url.contains('?')
            || url.contains('#')
            || self.version != 1
            || self.device_id.len() != 32
            || !self.device_id.bytes().all(|c| c.is_ascii_hexdigit())
            || self.token.len() != 64
            || !self.token.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("Invalid pairing".into());
        }
        let cert = STANDARD
            .decode(&self.certificate)
            .map_err(|_| "Invalid certificate")?;
        if cert.len() > 16384 || format!("{:x}", Sha256::digest(&cert)) != self.fingerprint {
            return Err("Certificate fingerprint mismatch".into());
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(cert))
            .map_err(|_| "Invalid certificate")?;
        Ok(rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth())
    }
}

/// Refuse malformed/unsupported requests before crossing JNI, and never replay.
pub fn validate_call(call: &Value) -> Result<(), &'static str> {
    if call["jsonrpc"] != "2.0" || call["id"].as_u64().is_none() {
        return Err("invalid_request");
    }
    let p = call["params"].as_array().ok_or("invalid_parameters")?;
    let number = |v: &Value| v.as_i64().is_some_and(|n| (0..=100000).contains(&n));
    let valid = match call["method"].as_str() {
        Some("screen" | "screenshot") => p.is_empty(),
        Some("typeText") => {
            p.len() == 2
                && p[0].as_str().is_some_and(|s| s.chars().count() <= 20000)
                && p[1].is_boolean()
        }
        Some("click") => (p.len() == 2 || p.len() == 3) && p.iter().all(number),
        Some("swipe") => p.len() == 5 && p.iter().all(number),
        Some("pressKey") => p.len() == 1 && matches!(p[0].as_str(), Some("back" | "home")),
        Some("launchApp") => {
            p.len() == 1
                && p[0].as_str().is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 255
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"._".contains(&c))
                })
        }
        _ => return Err("unsupported_method"),
    };
    if valid {
        Ok(())
    } else {
        Err("invalid_parameters")
    }
}

/// Reconnection restores only the transport. A received command is executed once.
pub async fn run(
    pair: Pairing,
    stop: Arc<AtomicBool>,
    dispatch: Arc<dyn Fn(String) -> String + Send + Sync>,
) -> Result<(), String> {
    let tls = Arc::new(pair.validate()?);
    while !stop.load(Ordering::Acquire) {
        let mut request = pair
            .endpoint
            .clone()
            .into_client_request()
            .map_err(|_| "Invalid endpoint")?;
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", pair.token)
                .parse()
                .map_err(|_| "Invalid token")?,
        );
        request.headers_mut().insert(
            "x-device-id",
            pair.device_id.parse().map_err(|_| "Invalid id")?,
        );
        let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
            max_message_size: Some(24 * 1024 * 1024),
            max_frame_size: Some(24 * 1024 * 1024),
            ..Default::default()
        };
        if let Ok(Ok((mut ws, _))) = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio_tungstenite::connect_async_tls_with_config(
                request,
                Some(cfg),
                false,
                Some(Connector::Rustls(tls.clone())),
            ),
        )
        .await
        {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => if stop.load(Ordering::Acquire) { let _ = ws.close(None).await; break; },
                    message = ws.next() => {
                        let Some(Ok(message)) = message else { break; };
                        let Message::Text(text) = message else { if matches!(message, Message::Close(_)) { break; } continue; };
                        if text.len() > 1024 * 1024 { break; }
                        let Ok(call) = serde_json::from_str::<Value>(&text) else { break; };
                        let body = match validate_call(&call) {
                            Err(code) => json!({"error":{"code":-32001,"message":code,"data":{"dispatched":false}}}),
                            Ok(()) => {
                                let dispatch = dispatch.clone();
                                match tokio::task::spawn_blocking(move || dispatch(text)).await {
                                    Ok(result) => serde_json::from_str::<Value>(&result).unwrap_or(json!({"error":{"code":-32002,"message":"invalid_native_result"}})),
                                    Err(_) => json!({"error":{"code":-32002,"message":"native_dispatch_failed"}}),
                                }
                            }
                        };
                        let mut response = body;
                        response["jsonrpc"] = json!("2.0"); response["id"] = call["id"].clone();
                        if ws.send(Message::Text(response.to_string())).await.is_err() { break; }
                    }
                }
            }
        }
        for _ in 0..12 {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    Ok(())
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use jni::{
        objects::{JObject, JString, JValue},
        JNIEnv,
    };
    use std::sync::{Mutex, OnceLock};
    static RUNNING: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    #[no_mangle]
    pub extern "system" fn Java_com_androiduse_phone_LinkService_stopNative(
        _env: JNIEnv,
        _obj: JObject,
    ) {
        if let Some(flag) = RUNNING
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .take()
        {
            flag.store(true, Ordering::Release);
        }
    }
    #[no_mangle]
    pub extern "system" fn Java_com_androiduse_phone_LinkService_startNative(
        mut env: JNIEnv,
        obj: JObject,
        config: JString,
    ) {
        let Ok(config) = env.get_string(&config) else {
            return;
        };
        let Ok(pair) = serde_json::from_str::<Pairing>(&config.to_string_lossy()) else {
            return;
        };
        if pair.validate().is_err() {
            return;
        }
        let Ok(vm) = env.get_java_vm() else {
            return;
        };
        let Ok(object) = env.new_global_ref(obj) else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(old) = RUNNING
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .replace(stop.clone())
        {
            old.store(true, Ordering::Release);
        }
        let dispatch = Arc::new(move |call: String| -> String {
            let error = "{\"error\":{\"code\":-32002,\"message\":\"jni_failed\"}}".to_owned();
            let Ok(mut env) = vm.attach_current_thread() else {
                return error;
            };
            let result = env.with_local_frame(8, |env| -> jni::errors::Result<String> {
                let input = env.new_string(call)?;
                let value = env
                    .call_method(
                        object.as_obj(),
                        "execute",
                        "(Ljava/lang/String;)Ljava/lang/String;",
                        &[JValue::Object(&input)],
                    )?
                    .l()?;
                let value = JString::from(value);
                let output: String = env.get_string(&value)?.into();
                Ok(output)
            });
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            result.unwrap_or(error)
        });
        std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Runtime::new() {
                let _ = runtime.block_on(run(pair, stop, dispatch));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_calls_before_native_dispatch() {
        assert!(
            validate_call(&json!({"jsonrpc":"2.0","id":1,"method":"click","params":[1,-1]}))
                .is_err()
        );
        assert!(
            validate_call(&json!({"jsonrpc":"2.0","id":1,"method":"shell","params":[]})).is_err()
        );
        assert!(validate_call(
            &json!({"jsonrpc":"2.0","id":1,"method":"typeText","params":["日本🙂",true]})
        )
        .is_ok());
    }
}
