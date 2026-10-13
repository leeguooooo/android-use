use base64::{engine::general_purpose::STANDARD, Engine as _};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, Implementation, ServerCapabilities},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone)]
pub struct DaemonClient {
    pub url: String,
    pub token: String,
    pub owner: String,
    client: reqwest::Client,
}
impl DaemonClient {
    pub fn new(url: String, token: String, owner: String) -> anyhow::Result<Self> {
        let parsed = reqwest::Url::parse(&url)?;
        if parsed.scheme() != "http"
            || !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        {
            anyhow::bail!("Android v0.1 clients require a loopback HTTP daemon URL");
        }
        if token.is_empty() {
            anyhow::bail!("Android daemon token is required");
        }
        Ok(Self {
            url: url.trim_end_matches('/').into(),
            token,
            owner,
            client: reqwest::Client::builder()
                .no_proxy()
                // A reconnect replaces the daemon; never reuse its old TCP stream.
                .pool_max_idle_per_host(0)
                .timeout(Duration::from_secs(120))
                .build()?,
        })
    }
    pub async fn request(&self, path: &str, body: Option<Value>) -> Value {
        let mutation = body.is_some() && path != "/agent/release";
        let request = match body {
            Some(ref value) => self.client.post(format!("{}{path}", self.url)).json(value),
            None => self.client.get(format!("{}{path}", self.url)),
        };
        let result = request
            .bearer_auth(&self.token)
            .header("X-Phone-Owner", &self.owner)
            .send()
            .await;
        match result {
            Ok(response) => match response.json::<Value>().await {
                Ok(value) => value,
                Err(_) => {
                    if mutation {
                        phone_control::Failure::unknown(
                            "daemon_invalid_response",
                            "Daemon response was lost; do not replay the action",
                        )
                        .json()
                    } else {
                        phone_control::Failure::not_sent(
                            "daemon_invalid_response",
                            "Daemon did not return JSON",
                        )
                        .json()
                    }
                }
            },
            Err(error) => {
                if mutation && !error.is_connect() {
                    phone_control::Failure::unknown(
                        "daemon_unreachable",
                        "Daemon response was lost; do not replay the action",
                    )
                    .json()
                } else {
                    phone_control::Failure::not_sent(
                        "daemon_unreachable",
                        "Android daemon unavailable; start android-use serve",
                    )
                    .json()
                }
            }
        }
    }
    pub async fn screenshot(&self) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(format!("{}/agent/screenshot", self.url))
            .bearer_auth(&self.token)
            .header("X-Phone-Owner", &self.owner)
            .send()
            .await
            .map_err(|_| "Screenshot request failed")?;
        if !response.status().is_success() {
            return Err("Screenshot unavailable or device owned by another caller".into());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Screenshot response incomplete")?;
        if bytes.len() > 32 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("Invalid screenshot".into());
        }
        Ok(bytes.to_vec())
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElementsParams {
    pub since: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionsParams {
    /// 1..32 actions. Types: tap (x,y OR element,snapshot OR label with optional kind),
    /// longpress (x,y,duration_ms), swipe (x1,y1,x2,y2,duration_ms),
    /// scroll (x,y,dx,dy), type (text,clear), back, home, launch_app (app), wait_for (expect: app/label/identifier/value, timeout_ms).
    pub steps: Vec<Value>,
    pub observe: Option<bool>,
    pub settle_ms: Option<u64>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TapParams {
    pub x: f64,
    pub y: f64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeParams {
    pub text: String,
    pub clear: Option<bool>,
}

fn compact(value: &Value) -> String {
    let mut summary = value.clone();
    let rows = summary.as_object_mut().and_then(|o| o.remove("elements"));
    let observation = summary
        .as_object_mut()
        .and_then(|o| o.remove("observation"));
    let mut output = summary.to_string();
    if let Some(rows) = rows.and_then(|v| v.as_array().cloned()) {
        for row in rows {
            output.push_str(&format!(
                "\n[{}] {} {}",
                row["index"],
                row["kind"].as_str().unwrap_or(""),
                serde_json::to_string(&row["label"]).unwrap()
            ));
            for key in ["identifier", "value"] {
                if let Some(v) = row.get(key) {
                    output.push_str(&format!(" {key}={v}"));
                }
            }
            if row["focused"] == true {
                output.push_str(" focused");
            }
            if row["enabled"] == false {
                output.push_str(" disabled");
            }
            if row["visible"] == false {
                output.push_str(" hidden");
            }
        }
    }
    if let Some(observation) = observation {
        output.push_str("\nobservation: ");
        output.push_str(&compact(&observation));
    }
    output
}

fn result(value: Value) -> CallToolResult {
    let failed = value.get("ok") == Some(&json!(false));
    let mut result = if failed {
        CallToolResult::error(vec![Content::text(compact(&value))])
    } else {
        CallToolResult::success(vec![Content::text(compact(&value))])
    };
    result.structured_content = Some(value);
    result
}
#[derive(Clone)]
pub struct AndroidHandler {
    client: DaemonClient,
}
#[tool_router]
impl AndroidHandler {
    pub fn new(client: DaemonClient) -> Self {
        Self { client }
    }
    #[tool(
        description = "Read selected Android device status. drivable=false means control is unavailable."
    )]
    async fn phone_status(&self) -> CallToolResult {
        result(self.client.request("/agent/status", None).await)
    }
    #[tool(
        description = "List supported Android controls and input limitations. Focused editable fields support Unicode insertion at the cursor. Password replacement is redacted."
    )]
    async fn phone_capabilities(&self) -> CallToolResult {
        result(self.client.request("/agent/capabilities", None).await)
    }
    #[tool(
        description = "Read Android UI as text. Use each row's index with this read's snapshot for a tap. since returns only changed/added rows plus removed baseline indexes. Missing baseline returns a full tree."
    )]
    async fn phone_elements(
        &self,
        Parameters(params): Parameters<ElementsParams>,
    ) -> CallToolResult {
        let path = match params.since {
            Some(id) => format!(
                "/agent/elements?since={}",
                id.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect::<String>()
            ),
            None => "/agent/elements".into(),
        };
        result(self.client.request(&path, None).await)
    }
    #[tool(
        description = "Execute 1..32 Android actions in order. All syntax is validated first; stop on first failure. Coordinates are normalized 0..1; positive scroll dy reveals content below. type inserts at the focused selection; clear:true replaces the field. wait_for confirms an app or element without mutation. launch_app needs a package name. Read results: outcome=unknown forbids replay; completed counts acknowledged actions. Applied actions stay applied if observation fails."
    )]
    async fn phone_actions(&self, Parameters(params): Parameters<ActionsParams>) -> CallToolResult {
        result(self.client.request("/agent/actions",Some(json!({"steps":params.steps,"observe":params.observe.unwrap_or(true),"settle_ms":params.settle_ms.unwrap_or(1500)}))).await)
    }
    #[tool(
        description = "Run known Android steps in one call, using wait_for after screen changes. Alias of phone_actions; never replay a partial or unknown result."
    )]
    async fn phone_run_steps(
        &self,
        Parameters(params): Parameters<ActionsParams>,
    ) -> CallToolResult {
        self.phone_actions(Parameters(params)).await
    }
    #[tool(
        description = "Tap normalized Android screen coordinates, then observe. Prefer snapshot-bound element taps through phone_actions."
    )]
    async fn phone_tap(&self, Parameters(params): Parameters<TapParams>) -> CallToolResult {
        result(
            self.client
                .request(
                    "/agent/input",
                    Some(json!({"type":"tap","x":params.x,"y":params.y})),
                )
                .await,
        )
    }
    #[tool(
        description = "Type Unicode into exactly one focused native Android text field. Default inserts at the current selection; clear:true replaces the whole field. Password replacement uses clear:true and never returns the secret. WebView fields need editable accessibility support. Never replay after an unknown outcome."
    )]
    async fn phone_type(&self, Parameters(params): Parameters<TypeParams>) -> CallToolResult {
        result(self.client.request("/agent/input",Some(json!({"type":"type","text":params.text,"clear":params.clear.unwrap_or(false)}))).await)
    }
    #[tool(
        description = "Capture Android screenshot on demand. Protected surfaces may be blank; screenshot does not bypass device restrictions."
    )]
    async fn phone_screenshot(&self) -> CallToolResult {
        match self.client.screenshot().await {
            Ok(bytes) => {
                CallToolResult::success(vec![Content::image(STANDARD.encode(bytes), "image/png")])
            }
            Err(error) => CallToolResult::error(vec![Content::text(error)]),
        }
    }
    #[tool(
        description = "Release this MCP session's Android device lease so another agent or CLI caller can control it."
    )]
    async fn phone_release(&self) -> CallToolResult {
        result(self.client.request("/agent/release", Some(json!({}))).await)
    }
}
#[tool_handler]
impl ServerHandler for AndroidHandler {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("android-use",env!("CARGO_PKG_VERSION")))
            .with_instructions("Control one Android device through a shared phone-control core. Start with phone_capabilities and phone_elements. Prefer batching visible actions. Never replay unknown or partially completed actions. Release the lease when finished.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_preserves_indexes_delta_and_focus_without_repeating_geometry() {
        let value = json!({"ok":true,"snapshot":"s","delta":{"removed":[2]},"elements":[{"index":7,"kind":"TextField","label":"日本語 🦊\nline","identifier":"field","value":"hello","focused":true,"visible":true,"rect":[1,2,3,4]}]});
        let text = compact(&value);
        assert!(text.contains("[7] TextField"));
        assert!(text.contains("🦊\\nline"));
        assert!(text.contains("focused"));
        assert!(text.contains("removed"));
        assert!(!text.contains("rect"));
        assert!(text.len() < value.to_string().len());
    }
}
