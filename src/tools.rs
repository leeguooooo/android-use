//! Host-side helpers that sit beside the vendored control core: action schema
//! docs and error details.
use phone_control::Action;
use serde_json::{json, Value};

/// Human-readable action reference shown by `act --help`.
pub const ACT_HELP: &str = r#"Actions (key is "type"; coordinates are normalized 0 to 1):
  tap        {"type":"tap","x":0.5,"y":0.5}
             {"type":"tap","element":3,"snapshot":"ID"}
             {"type":"tap","label":"Save","kind":"Button"}   (kind is optional)
  longpress  {"type":"longpress","x":0.5,"y":0.5,"duration_ms":500}
  swipe      {"type":"swipe","x1":0.5,"y1":0.8,"x2":0.5,"y2":0.2,"duration_ms":500}
  scroll     {"type":"scroll","x":0.5,"y":0.5,"dx":0,"dy":-0.4}
  type       {"type":"type","text":"hello","clear":false}
  back       {"type":"back"}
  home       {"type":"home"}
  launch_app {"type":"launch_app","app":"com.android.settings"}
  wait_for   {"type":"wait_for","expect":{"app":"...","label":"...","identifier":"...","value":"..."},"timeout_ms":5000}

Batch: {"steps":[ACTION,...],"observe":true,"settle_ms":1500} (at most 32 steps).
duration_ms is 100 to 10000. An expect needs at least one field."#;

/// Machine-readable counterpart of `ACT_HELP`, published in capabilities.
pub fn action_schema() -> Value {
    json!({
        "key":"type",
        "coordinates":"normalized_0_to_1",
        "actions":{
            "tap":{"one_of":[["x","y"],["element","snapshot"],["label"]],"optional":["kind"]},
            "longpress":{"required":["x","y"],"optional":["duration_ms"]},
            "swipe":{"required":["x1","y1","x2","y2"],"optional":["duration_ms"]},
            "scroll":{"required":["x","y","dx","dy"]},
            "type":{"required":["text"],"optional":["clear"]},
            "back":{},
            "home":{},
            "launch_app":{"required":["app"]},
            "wait_for":{"required":["expect"],"optional":["timeout_ms"],
                "expect_fields":["app","label","identifier","value"]}
        },
        "batch":{"required":["steps"],"optional":["observe","settle_ms"]}
    })
}

/// Say what is wrong with one action, or `None` when it parses.
pub fn action_problem(value: &Value) -> Option<String> {
    let parsed = serde_json::from_value::<Action>(value.clone());
    let error = parsed.err()?;
    let Some(object) = value.as_object() else {
        return Some("an action must be a JSON object".into());
    };
    let has = |key: &str| object.contains_key(key);
    let kind = object.get("type").and_then(Value::as_str);
    if !has("type") {
        return Some(if has("action") {
            "use the key \"type\", not \"action\"".into()
        } else {
            "missing the \"type\" key".to_string()
        });
    }
    if kind == Some("scroll") && has("direction") {
        return Some(
            "scroll has no direction field; it takes x, y, dx and dy (negative dy scrolls down)"
                .into(),
        );
    }
    if kind == Some("swipe") && ["start", "end", "from", "to"].iter().any(|k| has(k)) {
        return Some("swipe takes flat fields x1, y1, x2 and y2".into());
    }
    Some(error.to_string())
}

/// The client-facing message for a rejected action or batch step.
pub fn action_message(problem: &str) -> String {
    format!("Invalid action, no action sent: {problem}. See android-use act --help")
}

/// Say what is wrong with a batch, naming the first bad step.
pub fn batch_problem(body: &Value, error: &str) -> String {
    if let Some(steps) = body.get("steps").and_then(Value::as_array) {
        for (index, step) in steps.iter().enumerate() {
            if let Some(problem) = action_problem(step) {
                return format!("step {}: {problem}", index + 1);
            }
        }
    }
    error.to_string()
}
