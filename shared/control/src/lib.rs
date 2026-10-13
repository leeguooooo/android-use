//! Shared control runtime. Backends perform each mutation once; this layer
//! validates targets, serializes batches, observes outcomes and never retries.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Failure>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Element {
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub identifier: Option<String>,
    pub rect: [f64; 4],
    pub depth: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<String>,
    pub enabled: bool,
    pub visible: bool,
    pub focused: bool,
    #[serde(default)]
    pub password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screen {
    pub device: String,
    pub app: String,
    pub width: f64,
    pub height: f64,
    pub elements: Vec<Element>,
}

impl Screen {
    pub fn snapshot(&self) -> Result<String, Failure> {
        if !self.width.is_finite()
            || !self.height.is_finite()
            || self.width < 1.0
            || self.height < 1.0
        {
            return Err(Failure::not_sent(
                "invalid_screen",
                "Screen dimensions unavailable",
            ));
        }
        iu_core::control::snapshot_id(self)
            .map_err(|_| Failure::not_sent("invalid_screen", "Screen cannot be serialized"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub error: String,
    pub message: String,
    pub outcome: &'static str,
    pub retry_safe: bool,
}
impl Failure {
    pub fn not_sent(error: &str, message: &str) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            outcome: "not_sent",
            retry_safe: true,
        }
    }
    pub fn unknown(error: &str, message: &str) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            outcome: "unknown",
            retry_safe: false,
        }
    }
    pub fn json(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("failure serialization");
        value["ok"] = json!(false);
        value
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    WaitFor {
        expect: Expected,
        #[serde(default = "wait_default")]
        timeout_ms: u64,
    },
    Tap {
        x: Option<f64>,
        y: Option<f64>,
        element: Option<usize>,
        snapshot: Option<String>,
        label: Option<String>,
        /// Optional exact UI kind to disambiguate a label (for example StaticText versus Switch).
        kind: Option<String>, // profile-use: allow; UI element type declaration
    },
    Longpress {
        x: f64,
        y: f64,
        #[serde(default = "default_duration")]
        duration_ms: u64,
    },
    Swipe {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        #[serde(default = "default_duration")]
        duration_ms: u64,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Type {
        text: String,
        #[serde(default)]
        clear: bool,
    },
    Back,
    Home,
    LaunchApp {
        app: String,
    },
}
fn default_duration() -> u64 {
    500
}
fn wait_default() -> u64 {
    5000
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub app: Option<String>,
    pub label: Option<String>,
    pub identifier: Option<String>,
    pub value: Option<String>,
}
impl Expected {
    fn matches(&self, screen: &Screen) -> bool {
        if self.app.as_ref().is_some_and(|app| app != &screen.app) {
            return false;
        }
        if self.label.is_none() && self.identifier.is_none() && self.value.is_none() {
            return true;
        }
        screen.elements.iter().any(|row| {
            let [x, y, w, h] = row.rect;
            row.visible
                && [x, y, w, h].iter().all(|v| v.is_finite())
                && w > 0.0
                && h > 0.0
                && (0.0..screen.width).contains(&(x + w / 2.0))
                && (0.0..screen.height).contains(&(y + h / 2.0))
                && self.label.as_ref().is_none_or(|label| label == &row.label)
                && self
                    .identifier
                    .as_ref()
                    .is_none_or(|id| row.identifier.as_ref() == Some(id))
                && self
                    .value
                    .as_ref()
                    .is_none_or(|value| row.value.as_ref() == Some(value))
        })
    }
}
fn point(x: f64, y: f64) -> bool {
    [x, y]
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}
impl Action {
    pub fn validate(&self) -> Result<(), Failure> {
        let valid = match self {
            Self::WaitFor { expect, timeout_ms } => {
                [
                    expect.app.as_ref(),
                    expect.label.as_ref(),
                    expect.identifier.as_ref(),
                    expect.value.as_ref(),
                ]
                .iter()
                .any(Option::is_some)
                    && [
                        expect.app.as_ref(),
                        expect.label.as_ref(),
                        expect.identifier.as_ref(),
                        expect.value.as_ref(),
                    ]
                    .iter()
                    .flatten()
                    .all(|s| !s.is_empty() && s.len() <= 4096)
                    && (1..=30_000).contains(timeout_ms)
            }
            Self::Tap {
                x,
                y,
                element,
                snapshot,
                label,
                kind,
            } => {
                let coords = x.is_some() || y.is_some();
                let indexed = element.is_some() || snapshot.is_some();
                let named = label.is_some();
                (coords as u8 + indexed as u8 + named as u8 == 1)
                    && kind
                        .as_ref()
                        .is_none_or(|kind| named && !kind.is_empty() && kind.len() <= 128)
                    && ((!coords || x.zip(*y).is_some_and(|(x, y)| point(x, y)))
                        && (!indexed
                            || element.is_some()
                                && snapshot
                                    .as_ref()
                                    .is_some_and(|s| !s.is_empty() && s.len() <= 128))
                        && (!named
                            || label
                                .as_ref()
                                .is_some_and(|s| !s.is_empty() && s.len() <= 4096)))
            }
            Self::Longpress { x, y, duration_ms } => {
                point(*x, *y) && (100..=10_000).contains(duration_ms)
            }
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => point(*x1, *y1) && point(*x2, *y2) && (100..=10_000).contains(duration_ms),
            Self::Scroll { x, y, dx, dy } => {
                point(*x, *y) && dx.is_finite() && dy.is_finite() && (*dx != 0.0 || *dy != 0.0)
            }
            Self::Type { text, clear } => {
                (*clear || !text.is_empty()) && text.chars().count() <= 20_000
            }
            Self::LaunchApp { app } => {
                app.contains('.')
                    && app.len() <= 255
                    && app
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
            }
            Self::Back | Self::Home => true,
        };
        if valid {
            Ok(())
        } else {
            Err(Failure::not_sent(
                "invalid_action",
                "Invalid or conflicting action parameters",
            ))
        }
    }
}

/// Resolved against a live screen immediately before dispatch. Coordinates
/// are in backend screen units, not host display units.
#[derive(Debug, Clone)]
pub enum Dispatch {
    NativeType { text: String, clear: bool },
    Tap(f64, f64),
    Longpress(f64, f64, u64),
    Swipe(f64, f64, f64, f64, u64),
    SetText { element: Element, text: String },
    Back,
    Home,
    LaunchApp(String),
}
pub trait DeviceBackend: Send {
    fn native_typing(&self) -> bool {
        false
    }
    fn platform(&self) -> &'static str;
    fn supports_append(&self) -> bool {
        true
    }
    fn read(&mut self) -> BackendFuture<'_, Screen>;
    fn dispatch(&mut self, action: Dispatch) -> BackendFuture<'_, ()>;
    fn screenshot(&mut self) -> BackendFuture<'_, Vec<u8>>;
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub steps: Vec<Action>,
    #[serde(default = "observe_default")]
    pub observe: bool,
    #[serde(default = "settle_default")]
    pub settle_ms: u64,
}
fn observe_default() -> bool {
    true
}
fn settle_default() -> u64 {
    1500
}

pub struct Controller<B> {
    pub backend: B,
    pub stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    history: VecDeque<(String, Screen)>,
    progress: std::sync::Arc<std::sync::Mutex<iu_core::progress::ProgressTracker>>,
}
impl<B: DeviceBackend> Controller<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            stopping: Default::default(),
            history: VecDeque::new(),
            progress: Default::default(),
        }
    }
    pub fn clear_history(&mut self) {
        self.history.clear();
        if let Ok(mut progress) = self.progress.lock() {
            progress.reset();
        }
    }
    fn remember(&mut self, screen: Screen) -> Result<String, Failure> {
        let id = screen.snapshot()?;
        if self.history.back().is_none_or(|(last, _)| last != &id) {
            self.history.push_back((id.clone(), screen));
            if self.history.len() > 16 {
                self.history.pop_front();
            }
        }
        Ok(id)
    }
    fn response(&self, screen: &Screen, snapshot: &str, since: Option<&str>) -> Value {
        let rows: Vec<Value> = screen
            .elements
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let mut row = serde_json::to_value(row).expect("element serialization");
                row["index"] = json!(index);
                row
            })
            .collect();
        let mut result = json!({"ok":true,"snapshot":snapshot,"device":screen.device,"app":screen.app,
            "screen":{"width":screen.width,"height":screen.height,"coordinate_space":"device","input_coordinates":"normalized"},"elements":rows});
        if let Some((id, old)) =
            since.and_then(|since| self.history.iter().find(|(id, _)| id == since))
        {
            let delta = iu_core::control::diff_rows(&old.elements, &screen.elements, |row| {
                (&row.kind, &row.label, &row.identifier)
            });
            result["baseline"] = json!(id);
            result["delta"] = serde_json::to_value(&delta).expect("delta serialization");
            // Send only changed/added rows while retaining their full-screen indexes.
            result["elements"] = json!(rows
                .iter()
                .enumerate()
                .filter(|(i, _)| delta.added.contains(i) || delta.changed.contains(i))
                .map(|(_, r)| r)
                .collect::<Vec<_>>());
        }
        result
    }
    pub async fn elements(&mut self, since: Option<&str>) -> Result<Value, Failure> {
        let screen = self.backend.read().await?;
        let id = self.remember(screen.clone())?;
        let full = self.response(&screen, &id, None);
        if let Ok(mut progress) = self.progress.lock() {
            progress.note_screen(Some(&full));
        }
        Ok(self.response(&screen, &id, since))
    }
    pub async fn status(&mut self) -> Value {
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return json!({"ok":false,"platform":self.backend.platform(),"drivable":false,"error":"control_stopping","message":"This daemon is stopping; wait for its device lock to be released"});
        }
        match self.backend.read().await {
            Ok(screen) => match screen.snapshot() {
                Ok(_) => {
                    json!({"ok":true,"platform":self.backend.platform(),"device":screen.device,"app":screen.app,"drivable":true})
                }
                Err(e) => e.json(),
            },
            Err(error) => {
                json!({"ok":false,"platform":self.backend.platform(),"drivable":false,"error":error.error,"message":error.message})
            }
        }
    }
    fn resolve(
        action: &Action,
        screen: &Screen,
        supports_append: bool,
    ) -> Result<Dispatch, Failure> {
        let px = |x: f64| (x * screen.width).min(screen.width - 1.0);
        let py = |y: f64| (y * screen.height).min(screen.height - 1.0);
        Ok(match action {
            Action::WaitFor { .. } => {
                return Err(Failure::not_sent("invalid_dispatch", "Wait is read-only"))
            }
            Action::Tap {
                x,
                y,
                element,
                snapshot,
                label,
                kind,
            } => {
                let row = if let Some(index) = element {
                    if snapshot.as_deref() != Some(screen.snapshot()?.as_str()) {
                        return Err(Failure::not_sent(
                            "stale_snapshot",
                            "Read the screen again before tapping",
                        ));
                    }
                    Some(screen.elements.get(*index).ok_or_else(|| {
                        Failure::not_sent("element_not_found", "Element index out of range")
                    })?)
                } else if let Some(label) = label {
                    let matches: Vec<_> = screen
                        .elements
                        .iter()
                        .filter(|r| {
                            &r.label == label
                                && r.visible
                                && r.enabled
                                && kind.as_ref().is_none_or(|kind| kind == &r.kind)
                        })
                        .collect();
                    if matches.len() != 1 {
                        return Err(Failure::not_sent(
                            "ambiguous_element",
                            "Label must match exactly one visible enabled element",
                        ));
                    }
                    Some(matches[0])
                } else {
                    None
                };
                if let Some(row) = row {
                    let [x, y, w, h] = row.rect;
                    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
                    if !row.enabled
                        || !row.visible
                        || ![x, y, w, h].iter().all(|v| v.is_finite())
                        || w <= 0.0
                        || h <= 0.0
                        || cx < 0.0
                        || cy < 0.0
                        || cx >= screen.width
                        || cy >= screen.height
                    {
                        return Err(Failure::not_sent(
                            "invalid_target",
                            "Element is not actionable",
                        ));
                    }
                    Dispatch::Tap(cx, cy)
                } else {
                    Dispatch::Tap(px(x.expect("validated x")), py(y.expect("validated y")))
                }
            }
            Action::Longpress { x, y, duration_ms } => {
                Dispatch::Longpress(px(*x), py(*y), *duration_ms)
            }
            Action::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => Dispatch::Swipe(px(*x1), py(*y1), px(*x2), py(*y2), *duration_ms),
            Action::Scroll { x, y, dx, dy } => {
                let (x, y) = (px(*x), py(*y));
                Dispatch::Swipe(
                    x,
                    y,
                    (x - iu_core::control::swipe_travel(*dx, screen.width))
                        .clamp(0.0, screen.width - 1.0),
                    (y - iu_core::control::swipe_travel(*dy, screen.height))
                        .clamp(0.0, screen.height - 1.0),
                    500,
                )
            }
            Action::Type { text, clear } => {
                if !clear && !supports_append {
                    return Err(Failure::not_sent("append_input_unsupported", "Use clear:true to replace text; this backend cannot distinguish empty-field hints from existing text for appending"));
                }
                let fields: Vec<_> = screen
                    .elements
                    .iter()
                    .filter(|r| r.kind == "TextField" && r.focused && r.visible && r.enabled)
                    .collect();
                if fields.len() != 1 {
                    return Err(Failure::not_sent(
                        "input_not_focused",
                        "Focus exactly one native text field before typing",
                    ));
                }
                let row = fields[0];
                if row.password {
                    return Err(Failure::not_sent(
                        "password_input_unsupported",
                        "Password fields are unsupported by this text route",
                    ));
                }
                let text = if *clear {
                    text.clone()
                } else {
                    let value = row.value.as_ref().ok_or_else(|| {
                        Failure::not_sent(
                            "input_value_unavailable",
                            "Use clear:true to replace a field whose value is unknown",
                        )
                    })?;
                    format!("{value}{text}")
                };
                if text.chars().count() > 20_000 {
                    return Err(Failure::not_sent(
                        "text_too_long",
                        "Resulting field value exceeds 20000 characters",
                    ));
                }
                Dispatch::SetText {
                    element: row.clone(),
                    text,
                }
            }
            Action::Back => Dispatch::Back,
            Action::Home => Dispatch::Home,
            Action::LaunchApp { app } => Dispatch::LaunchApp(app.clone()),
        })
    }
    pub async fn execute(&mut self, batch: Batch) -> Value {
        if batch.steps.is_empty() || batch.steps.len() > 32 || batch.settle_ms > 10_000 {
            // profile-use: allow; technical settle budget
            return Failure::not_sent(
                "invalid_batch",
                "Use 1..32 actions; maximum settle budget is ten seconds",
            )
            .json();
        }
        // Validate the entire batch before any backend read or mutation.
        for step in &batch.steps {
            if let Err(error) = step.validate() {
                return error.json();
            }
        }
        let started = Instant::now();
        let baseline = self.history.back().map(|(id, _)| id.clone());
        let tracked = if batch.steps.len() == 1 && batch.observe {
            match &batch.steps[0] {
                Action::Tap {
                    element: Some(index),
                    snapshot: Some(snapshot),
                    ..
                } => Some(iu_core::progress::Action::TapElement {
                    index: *index as u64,
                    snapshot,
                }),
                Action::Tap {
                    label: Some(label), ..
                } => Some(iu_core::progress::Action::TapLabel(label)),
                Action::Tap {
                    x: Some(x),
                    y: Some(y),
                    ..
                } => Some(iu_core::progress::Action::TapPoint { x: *x, y: *y }),
                Action::Scroll { x, y, dx, dy } => Some(iu_core::progress::Action::Scroll {
                    x: *x,
                    y: *y,
                    dx: *dx,
                    dy: *dy,
                }),
                _ => None,
            }
        } else {
            None
        };
        let ticket = tracked
            .as_ref()
            .map(|action| iu_core::progress::ProgressGuard::begin(&self.progress, action));
        if ticket.is_none() {
            if let Ok(mut progress) = self.progress.lock() {
                progress.reset();
            }
        }
        let mut completed = 0;
        let mut applied = 0;
        for (index, action) in batch.steps.iter().enumerate() {
            let result = async {
                use std::sync::atomic::Ordering;
                if self.stopping.load(Ordering::Acquire) {
                    return Err(Failure::not_sent(
                        "control_stopping",
                        "Daemon is stopping; no further action was sent",
                    ));
                }
                if let Action::WaitFor { expect, timeout_ms } = action {
                    let deadline = Instant::now() + Duration::from_millis(*timeout_ms);
                    loop {
                        if self.stopping.load(Ordering::Acquire) {
                            return Err(Failure::not_sent(
                                "control_stopping",
                                "Daemon is stopping",
                            ));
                        }
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        match tokio::time::timeout(remaining, self.backend.read()).await {
                            Ok(Ok(screen)) if expect.matches(&screen) => {
                                self.remember(screen)?;
                                return Ok(());
                            }
                            Ok(Err(error))
                                if !matches!(
                                    error.error.as_str(),
                                    "screen_unavailable" | "screen_changed_during_read"
                                ) =>
                            {
                                return Err(error)
                            }
                            _ => {}
                        }
                        if Instant::now() >= deadline {
                            return Err(Failure::not_sent(
                                "condition_timeout",
                                "Expected app or element was not observed within the budget",
                            ));
                        }
                        tokio::time::sleep(
                            Duration::from_millis(75)
                                .min(deadline.saturating_duration_since(Instant::now())),
                        )
                        .await;
                    }
                }
                // Navigation has no screen-relative target. A transient tree
                // during Back/Home must not prevent the next navigation step.
                let action = match action {
                    Action::Back => Dispatch::Back,
                    Action::Home => Dispatch::Home,
                    Action::LaunchApp { app } => Dispatch::LaunchApp(app.clone()),
                    Action::Type { text, clear } if self.backend.native_typing() => {
                        Dispatch::NativeType {
                            text: text.clone(),
                            clear: *clear,
                        }
                    }
                    _ => {
                        let screen = self.backend.read().await?;
                        screen.snapshot()?;
                        Self::resolve(action, &screen, self.backend.supports_append())?
                    }
                };
                if self.stopping.load(Ordering::Acquire) {
                    return Err(Failure::not_sent(
                        "control_stopping",
                        "Daemon stopped during the read; no action was sent",
                    ));
                }
                self.backend.dispatch(action).await
            }
            .await;
            if let Err(error) = result {
                let mut result = error.json();
                result["completed"] = json!(completed);
                result["failed_step"] = json!(index);
                result["applied_actions"] = json!(applied);
                // Even a refused later step cannot make replaying the batch safe.
                result["retry_safe"] = json!(applied == 0 && error.retry_safe);
                result["elapsed_ms"] = json!(started.elapsed().as_millis() as u64);
                return result;
            }
            completed += 1;
            if !matches!(action, Action::WaitFor { .. }) {
                applied += 1;
            }
        }
        let mut result = json!({"ok":true,"outcome":if applied>0 {"applied"} else {"not_sent"},"retry_safe":applied==0,"completed":completed,"applied_actions":applied});
        if batch.observe {
            let expected_app = batch.steps.last().and_then(|step| match step {
                Action::LaunchApp { app } => Some(app.as_str()),
                _ => None,
            });
            let deadline = Instant::now() + Duration::from_millis(batch.settle_ms);
            let mut previous = None;
            let mut stable_since = None;
            let mut last: Option<(Screen, String)> = None;
            loop {
                if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
                    result["settle"] = json!({"settled":false,"reason":"stopping"});
                    break;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                let read = tokio::time::timeout(remaining, self.backend.read()).await;
                let read = match read {
                    Ok(read) => read,
                    Err(_) => {
                        if let Some((screen, id)) = last.take() {
                            result["observation"] =
                                self.response(&screen, &id, baseline.as_deref());
                            let _ = self.remember(screen);
                        }
                        result["settle"] = json!({"settled":false,"reason":"budget_exhausted"});
                        break;
                    }
                };
                match read {
                    Ok(screen) => match screen.snapshot() {
                        Ok(id) => {
                            let now = Instant::now();
                            if previous.as_ref() != Some(&id) {
                                stable_since = Some(now);
                            }
                            let settled = expected_app.is_none_or(|app| app == screen.app)
                                && !screen.elements.iter().any(|row| {
                                    matches!(
                                        row.kind.as_str(),
                                        "ProgressIndicator" | "ActivityIndicator"
                                    )
                                })
                                && previous.as_ref() == Some(&id)
                                && stable_since.is_some_and(|at| {
                                    now.duration_since(at) >= Duration::from_millis(200)
                                });
                            previous = Some(id.clone());
                            last = Some((screen.clone(), id.clone()));
                            if settled || now >= deadline {
                                let observation = self.response(&screen, &id, baseline.as_deref());
                                let _ = self.remember(screen);
                                result["observation"] = observation;
                                result["settle"] = json!({"settled":settled,"reason":if settled {"stable"} else {"budget_exhausted"}});
                                break;
                            }
                        }
                        Err(error) => {
                            result["observation_error"] = error.json();
                            break;
                        }
                    },
                    Err(error) => {
                        result["observation_error"] = error.json();
                        break;
                    }
                }
                if Instant::now() >= deadline {
                    if let Some((screen, id)) = last {
                        result["observation"] = self.response(&screen, &id, baseline.as_deref());
                        let _ = self.remember(screen);
                    }
                    result["settle"] = json!({"settled":false,"reason":"budget_exhausted"});
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if result.get("observation_error").is_some() {
                result["settle"] = json!({"settled":false,"reason":"read_failed"});
            }
        }
        if let Some(ticket) = ticket {
            let mut observed = result.get("observation").cloned();
            if let Some(observed) = observed.as_mut() {
                observed["settle"] = result["settle"].clone();
            }
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if let Some(advice) = ticket.finish(observed.as_ref(), now_ms) {
                result["advice"] = json!(advice);
            }
        }
        result["elapsed_ms"] = json!(started.elapsed().as_millis() as u64);
        result
    }
}
