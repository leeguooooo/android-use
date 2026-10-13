use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use phone_control::{Action, Batch, Controller, DeviceBackend, Failure};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub struct Session<B> {
    pub control: Controller<B>,
    owner: Option<(String, Instant)>,
}
impl<B: DeviceBackend> Session<B> {
    pub fn new(backend: B) -> Self {
        Self {
            control: Controller::new(backend),
            owner: None,
        }
    }
    fn acquire(&mut self, owner: &str) -> Result<(), Failure> {
        if let Some((held, until)) = &self.owner {
            if held != owner && *until > Instant::now() {
                return Err(Failure::not_sent(
                    "device_busy",
                    "Another caller holds the device; wait or ask that caller to release it",
                ));
            }
        }
        if self.owner.as_ref().is_none_or(|(held, _)| held != owner) {
            self.control.clear_history();
        }
        self.owner = Some((owner.into(), Instant::now() + Duration::from_secs(30)));
        Ok(())
    }
}
pub struct ApiState<B> {
    pub session: Arc<Mutex<Session<B>>>,
    token: Arc<String>,
}
impl<B> Clone for ApiState<B> {
    fn clone(&self) -> Self {
        Self {
            session: self.session.clone(),
            token: self.token.clone(),
        }
    }
}
impl<B> ApiState<B> {
    pub fn new(session: Arc<Mutex<Session<B>>>, token: String) -> Self {
        Self {
            session,
            token: Arc::new(token),
        }
    }
    fn caller(&self, headers: &HeaderMap) -> Result<String, Failure> {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if !token.is_some_and(|token| iu_core::auth::verify_password(token, &self.token)) {
            return Err(Failure::not_sent(
                "unauthorized",
                "A valid Android daemon bearer token is required",
            ));
        }
        let owner = headers
            .get("x-phone-owner")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("cli");
        if owner.is_empty()
            || owner.len() > 128
            || !owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(Failure::not_sent(
                "invalid_owner",
                "Use a nonempty owner containing letters, digits, - _ or .",
            ));
        }
        Ok(owner.into())
    }
}
fn failure(error: Failure) -> axum::response::Response {
    let status = match error.error.as_str() {
        "unauthorized" => StatusCode::UNAUTHORIZED,
        "device_busy" | "stale_snapshot" => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(error.json())).into_response()
}
pub fn capabilities() -> Value {
    json!({"ok":true,"platform":"android","version":env!("CARGO_PKG_VERSION"),
        "actions":["tap","longpress","swipe","scroll","type","back","home","launch_app","wait_for"],
        "elements":true,"non_bmp_tree_text":"preserved","screenshot":true,"batch":true,"delta":true,"snapshot_bound_tap":true,
        "input":{"coordinates":"normalized_0_to_1","text":"native_focused_field","append":true,"requires_clear_true":false,"insertion":"current_selection","max_characters":20000,"password_fields":"replace_only_redacted","webview_fields":"editable_accessibility_fields"},
        "max_batch_steps":32,"lease_seconds":30,"automatic_action_retry":false,
        "remote_video":false,"on_phone_daemon":false})
}
pub fn router<B: DeviceBackend + 'static>(state: ApiState<B>) -> Router {
    Router::new()
        .route("/agent/status", get(status::<B>))
        .route("/agent/capabilities", get(caps::<B>))
        .route("/agent/elements", get(elements::<B>))
        .route("/agent/screenshot", get(screenshot::<B>))
        .route("/agent/actions", post(actions::<B>))
        .route("/agent/input", post(input::<B>))
        .route("/agent/release", post(release::<B>))
        .route("/agent/disconnect", post(disconnect::<B>))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}
async fn status<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(e) = state.caller(&headers) {
        return failure(e);
    }
    Json(state.session.lock().await.control.status().await).into_response()
}
async fn caps<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(e) = state.caller(&headers) {
        return failure(e);
    }
    Json(capabilities()).into_response()
}
#[derive(Deserialize)]
struct ElementsQuery {
    since: Option<String>,
}
async fn elements<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
    Query(query): Query<ElementsQuery>,
) -> axum::response::Response {
    let owner = match state.caller(&headers) {
        Ok(o) => o,
        Err(e) => return failure(e),
    };
    let mut session = state.session.lock().await;
    if let Err(e) = session.acquire(&owner) {
        return failure(e);
    }
    match session.control.elements(query.since.as_deref()).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure(e),
    }
}
async fn screenshot<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let owner = match state.caller(&headers) {
        Ok(o) => o,
        Err(e) => return failure(e),
    };
    let mut session = state.session.lock().await;
    if let Err(e) = session.acquire(&owner) {
        return failure(e);
    }
    match session.control.backend.screenshot().await {
        Ok(bytes) => (
            [("content-type", "image/png"), ("cache-control", "no-store")],
            bytes,
        )
            .into_response(),
        Err(e) => failure(e),
    }
}
async fn actions<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let owner = match state.caller(&headers) {
        Ok(o) => o,
        Err(e) => return failure(e),
    };
    let batch = match serde_json::from_value::<Batch>(body) {
        Ok(b) => b,
        Err(_) => {
            return failure(Failure::not_sent(
                "invalid_batch",
                "Malformed or unsupported batch; no action sent",
            ))
        }
    };
    let mut session = state.session.lock().await;
    if let Err(e) = session.acquire(&owner) {
        return failure(e);
    }
    Json(session.control.execute(batch).await).into_response()
}
async fn input<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let step = match serde_json::from_value::<Action>(body) {
        Ok(s) => s,
        Err(_) => {
            return failure(Failure::not_sent(
                "invalid_action",
                "Malformed or unsupported action; no action sent",
            ))
        }
    };
    actions(State(state), headers, Json(json!({"steps":[step]}))).await
}
async fn release<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let owner = match state.caller(&headers) {
        Ok(o) => o,
        Err(e) => return failure(e),
    };
    let mut session = state.session.lock().await;
    if session
        .owner
        .as_ref()
        .is_some_and(|(held, _)| held != &owner)
    {
        return failure(Failure::not_sent(
            "device_busy",
            "Only the owner can release this device",
        ));
    }
    session.owner = None;
    session.control.clear_history();
    Json(json!({"ok":true,"released":true})).into_response()
}

async fn disconnect<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let owner = match state.caller(&headers) {
        Ok(owner) => owner,
        Err(e) => return failure(e),
    };
    let mut session = state.session.lock().await;
    if let Err(e) = session.acquire(&owner) {
        return failure(e);
    }
    session
        .control
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
    Json(json!({"ok":true,"disconnected":true})).into_response()
}
