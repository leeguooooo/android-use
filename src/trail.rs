//! The daemon's record of what the current owner did, turned into a flow draft
//! on request. Only steps the core reports as completed are recorded; typed
//! text becomes a named input and is never stored.
use serde_json::{json, Value};
use std::collections::VecDeque;

const MAX_STEPS: usize = 500;
const SNAPSHOTS: usize = 16;

#[derive(Debug, Default, Clone)]
struct Run {
    app: Option<String>,
    steps: Vec<Value>,
    todo: Vec<String>,
    inputs: usize,
}

#[derive(Debug, Default)]
pub struct Trail {
    current: Run,
    previous: Option<Run>,
    /// Full element reads by snapshot, so an index tap can become a label tap.
    snapshots: VecDeque<(String, Vec<Value>, f64, f64)>,
    /// Foreground app of the latest read, for a trail that starts mid-app.
    last_app: Option<String>,
}

impl Trail {
    /// Remember a full (non-delta) screen read.
    pub fn note_screen(&mut self, screen: &Value) {
        if screen.get("baseline").is_some() {
            return;
        }
        if let (Some(id), Some(rows)) = (screen["snapshot"].as_str(), screen["elements"].as_array())
        {
            if self.snapshots.iter().any(|(s, ..)| s == id) {
                return;
            }
            let w = screen["screen"]["width"].as_f64().unwrap_or(0.0);
            let h = screen["screen"]["height"].as_f64().unwrap_or(0.0);
            self.snapshots.push_back((id.into(), rows.clone(), w, h));
            if self.snapshots.len() > SNAPSHOTS {
                self.snapshots.pop_front();
            }
        }
        if let Some(app) = screen["app"].as_str() {
            self.last_app = Some(app.into());
        }
    }

    /// Record the first `completed` steps of a batch that ran.
    pub fn note_batch(&mut self, steps: &[Value], result: &Value) {
        let completed = result["completed"].as_u64().unwrap_or(0) as usize;
        for step in steps.iter().take(completed) {
            if self.current.steps.len() >= MAX_STEPS {
                if !self
                    .current
                    .todo
                    .iter()
                    .any(|t| t.starts_with("trail full"))
                {
                    self.current.todo.push(format!(
                        "trail full at {MAX_STEPS} steps; later steps were not recorded"
                    ));
                }
                break;
            }
            self.convert(step);
        }
        if let Some(obs) = result.get("observation") {
            self.note_screen(obs);
        }
    }

    fn convert(&mut self, step: &Value) {
        if self.current.app.is_none() && step["type"] != "launch_app" {
            self.current.app = self.last_app.clone();
        }
        let typ = step["type"].as_str().unwrap_or("");
        let mut out = step.clone();
        let object = out.as_object_mut().expect("validated action is an object");
        object.remove("type");
        // In a flow `kind` is the step kind; the element kind moves aside.
        if let Some(kind) = object.remove("kind") {
            object.insert("element_kind".into(), kind);
        }
        let mut flow = serde_json::Map::new();
        flow.insert("kind".into(), json!(typ));
        let n = self.current.steps.len() + 1;
        match typ {
            "tap" if step.get("element").is_some() => {
                let index = step["element"].as_u64().unwrap_or(u64::MAX) as usize;
                let snapshot = step["snapshot"].as_str().unwrap_or("");
                let row = self
                    .snapshots
                    .iter()
                    .find(|(s, ..)| s == snapshot)
                    .and_then(|(_, rows, w, h)| rows.get(index).map(|r| (r.clone(), *w, *h)));
                match row {
                    Some((row, _, _))
                        if row["label"].as_str().is_some_and(|l| !l.trim().is_empty()) =>
                    {
                        flow.insert("label".into(), row["label"].clone());
                        if let Some(kind) = row["kind"].as_str() {
                            flow.insert("element_kind".into(), json!(kind));
                        }
                    }
                    Some((row, w, h)) if w > 0.0 && h > 0.0 => {
                        let r = &row["rect"];
                        let (x, y, rw, rh) = (
                            r[0].as_f64().unwrap_or(0.0),
                            r[1].as_f64().unwrap_or(0.0),
                            r[2].as_f64().unwrap_or(0.0),
                            r[3].as_f64().unwrap_or(0.0),
                        );
                        let round = |v: f64| (v * 10_000.0).round() / 10_000.0;
                        flow.insert("x".into(), json!(round((x + rw / 2.0) / w)));
                        flow.insert("y".into(), json!(round((y + rh / 2.0) / h)));
                        self.current.todo.push(format!(
                            "step {n}: tapped an unlabeled element; recorded as its center coordinates — check it, add a wait_for before it"
                        ));
                    }
                    _ => {
                        flow.insert("x".into(), json!(0.5));
                        flow.insert("y".into(), json!(0.5));
                        self.current.todo.push(format!(
                            "step {n}: tapped element {index} of a screen read the daemon no longer holds; replace the placeholder coordinates"
                        ));
                    }
                }
            }
            "type" => {
                self.current.inputs += 1;
                let name = format!("text{}", self.current.inputs);
                flow.insert("input".into(), json!(name));
                if step["clear"] == true {
                    flow.insert("clear".into(), json!(true));
                }
            }
            "launch_app" => {
                if let Some(app) = step["app"].as_str() {
                    self.current.app.get_or_insert_with(|| app.into());
                }
                flow.extend(object.clone());
            }
            _ => flow.extend(object.clone()),
        }
        if typ == "tap" && step.get("x").is_some() {
            self.current.todo.push(format!(
                "step {n}: coordinate tap; prefer a label when the element has one"
            ));
        }
        self.current.steps.push(json!(flow));
    }

    /// The owner released or changed: keep the finished trail as `previous`.
    pub fn rotate(&mut self) {
        let finished = std::mem::take(&mut self.current);
        if !finished.steps.is_empty() {
            self.previous = Some(finished);
        }
    }

    pub fn reset(&mut self) {
        self.current = Run::default();
        self.previous = None;
        self.last_app = None;
    }

    pub fn draft(&self) -> Value {
        let (run, source) = if !self.current.steps.is_empty() {
            (&self.current, "current")
        } else if let Some(previous) = &self.previous {
            (previous, "previous")
        } else {
            return json!({"ok":false,"error":"empty_trail","message":"No recorded steps yet; drive the device (act / phone_run_steps), then ask for a draft"});
        };
        let inputs: serde_json::Map<String, Value> = (1..=run.inputs)
            .map(|i| {
                (
                    format!("text{i}"),
                    json!({"type":"string","description":"typed text (rename me)"}),
                )
            })
            .collect();
        let mut draft = json!({"version":1,"name":"Draft flow","description":"Recorded by android-use; edit before saving.",
            "risk":"navigation","steps":run.steps});
        if let Some(app) = &run.app {
            draft["app"] = json!(app);
        }
        if !inputs.is_empty() {
            draft["inputs"] = Value::Object(inputs);
        }
        let mut todo = vec![
            "give the flow a real name, description, category and risk (side_effect for send/pay/delete)".to_string(),
            "add a wait_for after every screen change (or a pause on screens without elements)".to_string(),
            "run it once, then add verified_on".to_string(),
        ];
        if run.inputs > 0 {
            todo.push("rename text1, text2… inputs; the typed text itself was not recorded".into());
        }
        todo.extend(run.todo.iter().cloned());
        json!({"ok":true,"source":source,"steps":run.steps.len(),"draft":draft,"todo":todo})
    }
}
