//! Paired TLS/WebSocket transport. No command is queued for reconnection.
use android_use_phone::Pairing;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::StreamExt;
use phone_control::Failure;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};

pub struct Request {
    value: Value,
    response: oneshot::Sender<Result<Value, ()>>,
}
#[derive(Clone)]
pub struct Bridge {
    pair: Arc<Pairing>,
    peer: Arc<Mutex<Option<mpsc::Sender<Request>>>>,
    epoch: Arc<AtomicU64>,
    api: Arc<AtomicU32>,
}
impl Bridge {
    pub fn new(pair: Pairing) -> Self {
        Self {
            pair: Arc::new(pair),
            peer: Arc::new(Mutex::new(None)),
            epoch: Arc::new(AtomicU64::new(0)),
            api: Arc::new(AtomicU32::new(0)),
        }
    }
    pub fn router(&self) -> Router {
        Router::new()
            .route("/phone", get(upgrade))
            .with_state(self.clone())
    }
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        mutation: bool,
    ) -> Result<Value, Failure> {
        let peer = self.peer.lock().await.clone().ok_or_else(|| {
            Failure::not_sent(
                "phone_disconnected",
                "Open android-use on the phone and check Wi-Fi",
            )
        })?;
        let (tx, rx) = oneshot::channel();
        let request = Request {
            value: json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            response: tx,
        };
        peer.try_send(request).map_err(|_| {
            Failure::not_sent(
                "phone_disconnected",
                "Phone connection unavailable; no request queued",
            )
        })?;
        let failed = || {
            if mutation {
                Failure::unknown(
                    "phone_ack_lost",
                    "No confirmation received; inspect the screen, never replay",
                )
            } else {
                Failure::not_sent(
                    "phone_read_failed",
                    "Phone read failed; check Wi-Fi and accessibility permission",
                )
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(15), rx)
            .await
            .map_err(|_| failed())?
            .map_err(|_| failed())?
            .map_err(|_| failed())?;
        if method == "screen" {
            if let Some(api) = result.pointer("/result/api").and_then(Value::as_u64) {
                self.api.store(api as u32, Ordering::Release);
            }
        }
        Ok(result)
    }
    pub fn api(&self) -> u32 {
        self.api.load(Ordering::Acquire)
    }
    pub async fn close(&self) {
        let mut peer = self.peer.lock().await;
        self.epoch.fetch_add(1, Ordering::AcqRel);
        peer.take();
    }
    async fn close_epoch(&self, epoch: u64) {
        let mut peer = self.peer.lock().await;
        if self.epoch.load(Ordering::Acquire) == epoch {
            self.epoch.fetch_add(1, Ordering::AcqRel);
            peer.take();
        }
    }
}
async fn upgrade(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let expected = format!("Bearer {}", bridge.pair.token);
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !iu_core::auth::verify_password(token, &expected)
        || headers.get("x-device-id").and_then(|v| v.to_str().ok()) != Some(&bridge.pair.device_id)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (tx, rx) = mpsc::channel(1);
    let epoch;
    {
        let mut peer = bridge.peer.lock().await;
        if peer.is_some() {
            return StatusCode::CONFLICT.into_response();
        }
        epoch = bridge.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        *peer = Some(tx);
    }
    // Clear reservations even if the HTTP client never completes the upgrade.
    let failed_bridge = bridge.clone();
    ws.max_message_size(24 * 1024 * 1024)
        .max_frame_size(24 * 1024 * 1024)
        .on_failed_upgrade(move |_| {
            tokio::spawn(async move {
                failed_bridge.close_epoch(epoch).await;
            });
        })
        .on_upgrade(move |socket| serve_peer(bridge, socket, rx, epoch))
        .into_response()
}
async fn serve_peer(
    bridge: Bridge,
    mut socket: WebSocket,
    mut requests: mpsc::Receiver<Request>,
    epoch: u64,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(250)) => if bridge.epoch.load(Ordering::Acquire) != epoch { break; },
            request = requests.recv() => {
                let Some(request) = request else { break; };
                // A cancelled caller's request must never become a later mutation.
                if request.response.is_closed() { continue; }
                if socket.send(Message::Text(request.value.to_string())).await.is_err() { let _ = request.response.send(Err(())); break; }
                let response = tokio::time::timeout(Duration::from_secs(12), async {
                    loop {
                        match socket.next().await {
                            Some(Ok(Message::Text(text))) => return serde_json::from_str::<Value>(&text).map_err(|_| ()),
                            Some(Ok(Message::Ping(data))) => { socket.send(Message::Pong(data)).await.map_err(|_| ())?; },
                            Some(Ok(Message::Pong(_))) => {},
                            _ => return Err(()),
                        }
                    }
                }).await.unwrap_or(Err(()));
                let valid = response.as_ref().is_ok_and(|v| v["jsonrpc"] == "2.0" && v["id"] == 1 && (v.get("result").is_some() ^ v.get("error").is_some()));
                let _ = request.response.send(if valid { response } else { Err(()) });
                // Close on timeout/malformed response: late acknowledgements cannot
                // be consumed as the next command's result.
                if !valid { break; }
            },
            frame = socket.next() => match frame {
                Some(Ok(Message::Ping(data))) => if socket.send(Message::Pong(data)).await.is_err() { break; },
                Some(Ok(Message::Pong(_))) => {},
                _ => break,
            },
        }
    }
    bridge.close_epoch(epoch).await;
    let _ = socket.close().await;
}
fn random_hex(count: usize) -> anyhow::Result<String> {
    let mut bytes = vec![0; count];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("Random source unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn pairing_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::backend::data_dir()?.join("pairing.json"))
}
pub fn load_pairing() -> anyhow::Result<Pairing> {
    let path = pairing_path()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            std::fs::metadata(&path)?.permissions().mode() & 0o077 == 0,
            "Pairing file must be private"
        );
    }
    let pair: Pairing = serde_json::from_slice(&std::fs::read(path)?)?;
    pair.validate().map_err(anyhow::Error::msg)?;
    Ok(pair)
}
pub fn create_pairing(host: &str, port: u16) -> anyhow::Result<Pairing> {
    let ip: std::net::IpAddr = host.parse()?;
    anyhow::ensure!(
        !ip.is_unspecified() && !ip.is_multicast(),
        "Specify the computer's reachable LAN address"
    );
    let cert = rcgen::generate_simple_self_signed(vec![host.into()])?;
    let dir = crate::backend::data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let authority = if ip.is_ipv6() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let pair = Pairing {
        version: 1,
        endpoint: format!("wss://{authority}/phone"),
        device_id: random_hex(16)?,
        token: random_hex(32)?,
        certificate: STANDARD.encode(cert.cert.der()),
        fingerprint: format!("{:x}", Sha256::digest(cert.cert.der())),
    };
    private_write(&dir.join("phone-cert.pem"), cert.cert.pem().as_bytes())?;
    private_write(
        &dir.join("phone-key.pem"),
        cert.key_pair.serialize_pem().as_bytes(),
    )?;
    private_write(&pairing_path()?, &serde_json::to_vec(&pair)?)?;
    Ok(pair)
}
fn private_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
