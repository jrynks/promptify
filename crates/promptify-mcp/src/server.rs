//! Stdio MCP server that forwards to the running Promptify app over its loopback API.

use std::path::PathBuf;
use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, ServiceExt, schemars, tool, tool_router};
use serde::Deserialize;

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

const NOT_RUNNING: &str = "Promptify is not reachable. Start Promptify and turn on \"Allow paired devices to use Promptify\" in its settings.";

impl PromptifyTools {
    pub fn new(base: String, token_path: PathBuf) -> Self {
        // A redirect could carry the bearer token to another address.
        let http = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
        Self { http, base, token_path }
    }

    async fn call(&self, body: serde_json::Value) -> Result<String, String> {
        // Read per call: the app creates the token when remote access is first turned on.
        let token = std::fs::read_to_string(&self.token_path).map_err(|_| NOT_RUNNING.to_string())?;
        let response = self
            .http
            .post(format!("{}/v1/transform", self.base))
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
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let outcome = &value["outcome"];
        match outcome["kind"].as_str() {
            Some("ready") | Some("truncated") => Ok(outcome["text"].as_str().unwrap_or_default().to_owned()),
            Some(kind) => Err(format!("Promptify could not write a prompt ({kind}: {})", outcome["reason"].as_str().unwrap_or("no detail"))),
            None => Err("unexpected reply from Promptify".into()),
        }
    }

    fn result(outcome: Result<String, String>) -> CallToolResult {
        match outcome {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        }
    }
}

#[tool_router(server_handler)]
impl PromptifyTools {
    #[tool(description = "Rewrite a rough request into a clear, well-structured prompt for another AI, using Promptify's local model on this computer. AI prompts get numbered steps with dependencies, bounded check-and-revise loops and completion checks, sized to the request. Image and video creation prompts stay descriptive.")]
    async fn transform_prompt(&self, Parameters(args): Parameters<TransformArgs>) -> Result<CallToolResult, McpError> {
        let body = serde_json::json!({ "text": args.text, "mode": "prompt", "app": args.app.unwrap_or_default(), "url": args.url });
        Ok(Self::result(self.call(body).await))
    }

    #[tool(description = "Tidy dictated text: remove filler words and fix casing without changing the wording.")]
    async fn clean_dictation(&self, Parameters(args): Parameters<DictationArgs>) -> Result<CallToolResult, McpError> {
        Ok(Self::result(self.call(serde_json::json!({ "text": args.text, "mode": "dictation" })).await))
    }
}

pub async fn serve_stdio(base: String, token_path: PathBuf) -> Result<(), String> {
    let service = PromptifyTools::new(validate_api_base(&base)?, token_path).serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
    service.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_api_base;

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
