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
    trail: crate::trail::Trail,
    /// Whose steps the trail holds; a draft is only shown to them.
    trail_owner: Option<String>,
}
impl<B: DeviceBackend> Session<B> {
    pub fn new(backend: B) -> Self {
        Self {
            control: Controller::new(backend),
            owner: None,
            trail: Default::default(),
            trail_owner: None,
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
        if self.trail_owner.as_deref() != Some(owner) {
            self.trail.reset();
            self.trail_owner = Some(owner.into());
        }
        self.owner = Some((owner.into(), Instant::now() + Duration::from_secs(30)));
        Ok(())
    }
}
pub struct ApiState<B> {
    pub session: Arc<Mutex<Session<B>>>,
    token: Arc<String>,
    backend: &'static str,
    bridge: Option<crate::accessibility::Bridge>,
}
impl<B> Clone for ApiState<B> {
    fn clone(&self) -> Self {
        Self {
            session: self.session.clone(),
            token: self.token.clone(),
            backend: self.backend,
            bridge: self.bridge.clone(),
        }
    }
}
impl<B> ApiState<B> {
    pub fn new(session: Arc<Mutex<Session<B>>>, token: String) -> Self {
        Self {
            session,
            token: Arc::new(token),
            backend: "adb",
            bridge: None,
        }
    }
    pub fn with_phone_bridge(mut self, bridge: Option<crate::accessibility::Bridge>) -> Self {
        self.bridge = bridge;
        self
    }
    pub fn with_backend(mut self, backend: &'static str) -> Self {
        self.backend = backend;
        self
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
        .route("/agent/flow/draft", get(flow_draft::<B>))
        .route("/agent/flow/reset", post(flow_reset::<B>))
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
    let mut value = state.session.lock().await.control.status().await;
    value["backend"] = json!(state.backend);
    Json(value).into_response()
}
async fn caps<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(e) = state.caller(&headers) {
        return failure(e);
    }
    let mut value = capabilities();
    value["backend"] = json!(state.backend);
    value["on_phone_daemon"] = json!(state.backend == "accessibility");
    if state.backend == "accessibility" {
        value["screenshot"] = json!(state.bridge.as_ref().is_some_and(|b| b.api() >= 30));
        value["screenshot_min_api"] = json!(30);
    }
    Json(value).into_response()
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
        Ok(v) => {
            session.trail.note_screen(&v);
            Json(v).into_response()
        }
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
    let steps = body["steps"].as_array().cloned().unwrap_or_default();
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
    let result = session.control.execute(batch).await;
    session.trail.note_batch(&steps, &result);
    Json(result).into_response()
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
    session.trail.rotate();
    Json(json!({"ok":true,"released":true})).into_response()
}
/// The owner's recorded steps as a flow v1 draft (read-only).
async fn flow_draft<B: DeviceBackend>(
    State(state): State<ApiState<B>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let owner = match state.caller(&headers) {
        Ok(o) => o,
        Err(e) => return failure(e),
    };
    let session = state.session.lock().await;
    if session
        .owner
        .as_ref()
        .is_some_and(|(held, until)| held != &owner && *until > Instant::now())
    {
        return failure(Failure::not_sent(
            "device_busy",
            "Another caller holds the device and its trail",
        ));
    }
    if session.trail_owner.as_deref() != Some(owner.as_str()) {
        return Json(crate::trail::Trail::default().draft()).into_response();
    }
    Json(session.trail.draft()).into_response()
}
/// Start a fresh recording for this owner.
async fn flow_reset<B: DeviceBackend>(
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
    session.trail.reset();
    Json(json!({"ok":true,"reset":true})).into_response()
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
