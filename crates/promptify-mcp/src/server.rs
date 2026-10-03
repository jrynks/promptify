//! Stdio MCP server that forwards to the running Promptify app over its loopback API.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams, Tool};
use rmcp::{ErrorData as McpError, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use promptify_core::routing::{Rendering, RoutingOptions, Surface, TaskId};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransformArgs {
    /// The rough request to turn into a well-structured prompt.
    pub text: String,
    /// Where the prompt will be used, as an app or process name, e.g. "claude", "cursor" or "chatgpt".
    #[serde(default)]
    pub app: Option<String>,
    /// Web address of the target AI site, e.g. "https://chatgpt.com".
    #[serde(default)]
    pub url: Option<String>,
    /// Optional format policy: legacy or adaptive. Adaptive requires the v2 local API.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub rendering: Option<Rendering>,
    /// Optional enabled catalog ID, such as code.debug; requires adaptive rendering.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub task_type: Option<TaskId>,
    /// Optional confirmed AI input surface. Do not select an AI surface for a literal content field.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub surface: Option<Surface>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DictationArgs {
    /// Dictated text to tidy up without rewriting it.
    pub text: String,
}

#[derive(Clone)]
pub struct PromptifyTools {
    http: reqwest::Client,
    base: String,
    token_path: PathBuf,
    adaptive_available: Arc<AtomicBool>,
}

/// The API token must never leave this computer, so only loopback API addresses are accepted.
/// The address is rebuilt from its parsed parts, so text like `127.0.0.1:1@evil.example` cannot reach another host.
pub fn validate_api_base(base: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base).map_err(|_| "--api must be an http:// loopback address")?;
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return Err("--api must be an http:// loopback address such as http://127.0.0.1:47821".into());
    }
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else { return Err("--api needs a host and port".into()) };
    let ip = host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>();
    if !(host == "localhost" || ip.is_ok_and(|ip| ip.is_loopback())) {
        return Err("--api must point at this computer (127.0.0.1, localhost or [::1])".into());
    }
    Ok(format!("http://{host}:{port}"))
}

const NOT_RUNNING: &str = "Promptify is not reachable. Start Promptify and enable the API under Settings > Desktop API.";

impl PromptifyTools {
    pub fn new(base: String, token_path: PathBuf) -> Self {
        // A redirect could carry the bearer token to another address.
        let http = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
        Self { http, base, token_path, adaptive_available: Arc::new(AtomicBool::new(false)) }
    }

    async fn call(&self, body: serde_json::Value) -> Result<String, String> {
        let value = self.call_endpoint("v1/transform", body).await?;
        Self::response_text(&value["outcome"])
    }

    async fn call_endpoint(&self, endpoint: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
        // Read per call: the app creates the token when the desktop API is first turned on.
        let token = std::fs::read_to_string(&self.token_path).map_err(|_| NOT_RUNNING.to_string())?;
        let response = self
            .http
            .post(format!("{}/{endpoint}", self.base))
            .bearer_auth(token.trim())
            .header("content-type", "application/json")
            .body(body.to_string())
            .timeout(Duration::from_secs(150))
            .send()
            .await
            .map_err(|_| NOT_RUNNING.to_string())?;
        let status = response.status();
        if status.as_u16() == 401 {
            return Err("Promptify rejected the API token; restart the MCP server after reinstalling Promptify.".into());
        }
        let text = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("Promptify returned {status}: {}", text.chars().take(200).collect::<String>()));
        }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    async fn supports_adaptive(&self) -> Result<bool, String> {
        let token = std::fs::read_to_string(&self.token_path).map_err(|_| NOT_RUNNING.to_owned())?;
        let response = self.http.get(format!("{}/v2/catalog", self.base)).bearer_auth(token.trim())
            .timeout(Duration::from_secs(3)).send().await.map_err(|error| format!("{NOT_RUNNING} {error}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(format!("Promptify capability discovery failed: {}", response.status()));
        }
        let value: serde_json::Value = serde_json::from_str(&response.text().await.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if value["version"].as_u64() != Some(u64::from(promptify_core::routing::CATALOG_VERSION)) {
            return Err("Promptify advertised an unsupported catalog version.".into());
        }
        Ok(value["rendering"].as_array().is_some_and(|values| values.iter().any(|value| value == "adaptive")))
    }

    fn advertised_tool(mut tool: Tool, adaptive: bool) -> Tool {
        if tool.name == "transform_prompt" && !adaptive {
            let schema = Arc::make_mut(&mut tool.input_schema);
            if let Some(properties) = schema.get_mut("properties").and_then(serde_json::Value::as_object_mut) {
                for name in ["rendering", "task_type", "surface"] {
                    properties.remove(name);
                }
            }
        }
        tool
    }

    fn response_text(outcome: &serde_json::Value) -> Result<String, String> {
        match outcome["kind"].as_str() {
            Some("ready") => outcome["text"].as_str().filter(|text| !text.trim().is_empty())
                .map(str::to_owned).ok_or_else(|| "Promptify returned no prompt text.".into()),
            Some("truncated") => Err("Promptify returned incomplete output; it was not returned as a usable prompt.".into()),
            Some(kind) => Err(format!("Promptify could not write a prompt ({kind}: {})", outcome["detail"].as_str().or_else(|| outcome["reason"].as_str()).unwrap_or("no detail"))),
            None => Err("unexpected reply from Promptify".into()),
        }
    }

    fn require_graph(text: String) -> Result<String, String> {
        promptify_core::structure::validate_graph(&text).map_err(|_| {
            "The backend returned a prompt without the mandatory steps, bounded loop, and Done when criteria. It was not returned as a usable prompt.".to_owned()
        })?;
        Ok(text)
    }

    fn result(outcome: Result<String, String>) -> CallToolResult {
        match outcome {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        }
    }
}

#[tool_router]
impl PromptifyTools {
    #[tool(description = "Rewrite a rough request into a prompt for another AI using the local model. Every final prompt requires numbered steps, a bounded check loop and Done when criteria. Adaptive routing changes task-specific content, never that mandate. Generator-only fields need review because they cannot be assumed to execute a graph. No destination actions are executed.")]
    async fn transform_prompt(&self, Parameters(args): Parameters<TransformArgs>) -> Result<CallToolResult, McpError> {
        let has_override = args.task_type.is_some() || args.surface.is_some();
        let routing = RoutingOptions {
            rendering: args.rendering.unwrap_or(if has_override { Rendering::Adaptive } else { Rendering::Legacy }),
            task_type: args.task_type, surface: args.surface,
        };
        if let Err(error) = routing.validate() {
            return Ok(Self::result(Err(error)));
        }
        if routing.rendering == Rendering::Adaptive {
            let available = match self.supports_adaptive().await {
                Ok(available) => available,
                Err(error) => return Ok(Self::result(Err(error))),
            };
            self.adaptive_available.store(available, Ordering::SeqCst);
            if !available {
                return Ok(Self::result(Err("This Promptify backend does not support adaptive prompts. Upgrade it or explicitly use legacy rendering without task/surface overrides.".into())));
            }
            let body = serde_json::json!({ "text": args.text, "mode": "prompt", "app": args.app.unwrap_or_default(), "url": args.url, "routing": routing });
            return Ok(match self.call_endpoint("v2/transform", body).await {
                Ok(value) => {
                    let outcome = if value["outcome"]["kind"] == "truncated" {
                        Err("The adaptive prompt is incomplete; it must not be inserted automatically.".into())
                    } else {
                        Self::response_text(&value["outcome"]).and_then(Self::require_graph)
                    };
                    match outcome {
                        Ok(text) => {
                            let mut result = Self::result(Ok(text));
                            result.structured_content = Some(value);
                            result
                        }
                        Err(error) => Self::result(Err(error)),
                    }
                }
                Err(error) => Self::result(Err(error)),
            });
        }
        let body = serde_json::json!({ "text": args.text, "mode": "prompt", "app": args.app.unwrap_or_default(), "url": args.url });
        Ok(Self::result(self.call(body).await.and_then(Self::require_graph)))
    }

    #[tool(description = "Tidy dictated text: remove filler words and fix casing without changing the wording.")]
    async fn clean_dictation(&self, Parameters(args): Parameters<DictationArgs>) -> Result<CallToolResult, McpError> {
        Ok(Self::result(self.call(serde_json::json!({ "text": args.text, "mode": "dictation" })).await))
    }
}

#[tool_handler]
impl rmcp::ServerHandler for PromptifyTools {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let adaptive = self.supports_adaptive().await.unwrap_or_else(|error| {
            log::warn!("adaptive tool schema is unavailable: {error}");
            false
        });
        self.adaptive_available.store(adaptive, Ordering::SeqCst);
        let supports_cache = context.protocol_version().is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        Ok(ListToolsResult {
            result_type: Some(rmcp::model::ResultType::COMPLETE),
            tools: Self::tool_router().list_all().into_iter().map(|tool| Self::advertised_tool(tool, adaptive)).collect(),
            meta: None, next_cursor: None, ttl_ms: supports_cache.then_some(0),
            cache_scope: supports_cache.then_some(rmcp::model::CacheScope::Public),
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::tool_router().get(name).cloned().map(|tool| Self::advertised_tool(tool, self.adaptive_available.load(Ordering::SeqCst)))
    }
}

pub async fn serve_stdio(base: String, token_path: PathBuf) -> Result<(), String> {
    let service = PromptifyTools::new(validate_api_base(&base)?, token_path).serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
    service.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PromptifyTools, TransformArgs, validate_api_base};
    use promptify_core::routing::Rendering;
    use rmcp::handler::server::wrapper::Parameters;

    fn discovery_backend(status: &str, body: &str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert!(first.starts_with("GET /v2/catalog "));
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() { break; }
            }
            socket.write_all(response.as_bytes()).unwrap();
        });
        (base, thread)
    }

    #[test]
    fn adaptive_requests_to_old_backends_are_explicit_errors() {
        let (base, server) = discovery_backend("404 Not Found", "");
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        std::fs::write(&token, "test-only-loopback-token").unwrap();
        let tools = PromptifyTools::new(base, token);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(tools.transform_prompt(Parameters(TransformArgs {
            text: "Debug the checkout crash".into(), app: Some("cursor".into()), url: None,
            rendering: Some(Rendering::Adaptive), task_type: None, surface: None,
        }))).unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].as_text().unwrap().text.contains("does not support adaptive"));
        assert!(result.structured_content.is_none());
        server.join().unwrap();
    }

    #[test]
    fn capability_discovery_checks_the_advertised_version() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        std::fs::write(&token, "test-only-loopback-token").unwrap();
        for (body, expected) in [
            (r#"{"version":1,"rendering":["legacy","adaptive"]}"#, true),
            (r#"{"version":1,"rendering":["legacy"]}"#, false),
        ] {
            let (base, server) = discovery_backend("200 OK", body);
            let tools = PromptifyTools::new(base, token.clone());
            assert_eq!(runtime.block_on(tools.supports_adaptive()).unwrap(), expected);
            server.join().unwrap();
        }
        let (base, server) = discovery_backend("200 OK", r#"{"version":99,"rendering":["adaptive"]}"#);
        let tools = PromptifyTools::new(base, token);
        assert!(runtime.block_on(tools.supports_adaptive()).unwrap_err().contains("unsupported catalog version"));
        server.join().unwrap();
    }

    #[test]
    fn adaptive_parameters_require_verified_backend_capabilities() {
        let tool = PromptifyTools::tool_router().get("transform_prompt").unwrap().clone();
        let legacy = PromptifyTools::advertised_tool(tool.clone(), false);
        let adaptive = PromptifyTools::advertised_tool(tool, true);
        for name in ["rendering", "task_type", "surface"] {
            assert!(legacy.input_schema["properties"].get(name).is_none());
            assert!(adaptive.input_schema["properties"].get(name).is_some());
        }
        assert_eq!(legacy.input_schema["required"], adaptive.input_schema["required"]);
        assert!(PromptifyTools::response_text(&serde_json::json!({ "kind": "ready" })).is_err());
        assert!(PromptifyTools::require_graph("A plain description.".into()).is_err());
        assert!(PromptifyTools::response_text(&serde_json::json!({ "kind": "truncated", "text": "Partial prompt" })).is_err());
    }

    #[test]
    fn rejected_prompt_details_reach_the_mcp_client() {
        let detail = "The role/persona-prefixed draft was not used.";
        let rejected = serde_json::json!({ "kind": "failed", "reason": "invalid_prompt", "detail": detail });
        let error = PromptifyTools::response_text(&rejected).unwrap_err();
        assert!(error.contains(detail));
        assert_eq!(PromptifyTools::result(Err(error)).is_error, Some(true));
        let fallback = serde_json::json!({ "kind": "failed", "reason": "generation_failed", "detail": null });
        assert!(PromptifyTools::response_text(&fallback).unwrap_err().contains("generation_failed"));
    }

    #[test]
    fn token_only_goes_to_loopback() {
        assert_eq!(validate_api_base("http://127.0.0.1:47821/").unwrap(), "http://127.0.0.1:47821");
        assert!(validate_api_base("http://localhost:1").is_ok());
        assert!(validate_api_base("http://[::1]:1").is_ok());
        for bad in [
            "https://127.0.0.1:1",
            "http://127.0.0.1.evil.com:1",
            "http://evil.com/127.0.0.1",
            "http://user@evil.com:1",
            "http://10.0.0.2:1",
            "http://127.0.0.1:47821@evil.example",
            "http://localhost:9@attacker.test/x",
            "http://[::1]:1@evil.example",
            "http://user:pass@127.0.0.1:1",
            "http://user@127.0.0.1:1",
            "http://127.0.0.1:1/other?x=1",
            "http://localhost.evil.com:1",
        ] {
            assert!(validate_api_base(bad).is_err(), "{bad}");
        }
        assert_eq!(validate_api_base("http://127.0.0.2:5").unwrap(), "http://127.0.0.2:5", "all of 127/8 is loopback");
        assert_eq!(validate_api_base("http://LOCALHOST:5").unwrap(), "http://localhost:5", "rebuilt from parsed parts");
        assert_eq!(validate_api_base("http://127.1:5").unwrap(), "http://127.0.0.1:5", "rebuilt from parsed parts");
    }
}
