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

/// Mark a full element read whose app area holds almost nothing, which is what
/// GL canvases (Unity, Flutter games) look like. Deltas are sparse by design.
pub fn annotate_sparse_tree(screen: &mut Value) {
    let app = screen["app"].as_str().unwrap_or_default();
    let height = screen["screen"]["height"].as_f64().unwrap_or(0.0);
    if screen.get("delta").is_some()
        || app.is_empty()
        || height <= 0.0
        || app.contains("launcher")
        || app == "com.android.systemui"
    {
        return;
    }
    let Some(rows) = screen["elements"].as_array() else {
        return;
    };
    let content = rows
        .iter()
        .filter(|row| {
            let named = ["label", "identifier"]
                .iter()
                .any(|key| row[key].as_str().is_some_and(|text| !text.is_empty()));
            let top = row["rect"][1].as_f64().unwrap_or(0.0);
            let middle = (top + row["rect"][3].as_f64().unwrap_or(0.0) / 2.0) / height;
            // The outer bands hold the status and navigation bars.
            named && (0.06..=0.92).contains(&middle)
        })
        .count();
    if content <= 1 {
        screen["canvas_rendered"] = json!(true);
        screen["hint"] = json!("This app exposes almost no accessible nodes and probably draws on a canvas. Use android-use screenshot --max-width 720, or add --crop, and tap normalized coordinates");
    }
}

/// Crop (normalized `x,y,w,h`) and shrink a PNG so a screenshot costs less to read.
pub fn resize_png(
    bytes: &[u8],
    max_width: Option<u32>,
    crop: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Invalid screenshot: {e}"))?;
    if let Some(crop) = crop {
        let parts: Vec<f64> = crop
            .split(',')
            .map(|part| part.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| "--crop takes four numbers: x,y,w,h".to_string())?;
        let [x, y, w, h] = parts[..] else {
            return Err("--crop takes four numbers: x,y,w,h".into());
        };
        if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x + w > 1.0 || y + h > 1.0 {
            return Err("--crop must stay inside the normalized 0 to 1 screen".into());
        }
        let (width, height) = (f64::from(image.width()), f64::from(image.height()));
        image = image.crop_imm(
            (x * width) as u32,
            (y * height) as u32,
            ((w * width) as u32).max(1),
            ((h * height) as u32).max(1),
        );
    }
    if let Some(limit) = max_width {
        if limit == 0 {
            return Err("--max-width must be positive".into());
        }
        if image.width() > limit {
            image = image.resize(limit, u32::MAX, image::imageops::FilterType::Triangle);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("Unable to encode screenshot: {e}"))?;
    Ok(out.into_inner())
}
