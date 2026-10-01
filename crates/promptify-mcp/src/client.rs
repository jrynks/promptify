//! Context hooks: before a prompt is written, call tools on the user's MCP servers and pass their
//! text to the model as delimited, untrusted reference material.
//!
//! Configured in `mcp.json` in the app data folder:
//! ```json
//! { "servers": { "docs": {
//!     "command": "uvx", "args": ["some-mcp-server"],
//!     "profiles": ["cursor", "claude_code"],
//!     "hooks": [ { "tool": "search", "arguments": { "query": "{transcript}" } } ] } } }
//! ```
//! `url` may replace `command` for a Streamable HTTP server. Remote (`url`) servers never receive the
//! transcript unless `"allow_transcript": true` is set for them. The usual `"mcpServers"` key is
//! accepted too.
//!
//! `"loop_tools": ["search"]` lets the model call those tools itself, at most twice per prompt.
//! Only servers allowed to see the transcript may offer loop tools.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use promptify_core::context::ActiveContext;
use promptify_core::pipeline::CancelToken;
use promptify_core::prompt::ToolContext;
use promptify_core::tool_loop::{ToolRequest, ToolSpec};
use promptify_core::transform::ContextEnricher;
use rmcp::model::{CallToolRequestParams, JsonObject};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde::Deserialize;

pub const MAX_SERVERS: usize = 8;
pub const MAX_HOOKS_PER_SERVER: usize = 4;
const DEFAULT_TIMEOUT_MS: u64 = 3000;
const DEFAULT_MAX_CHARS: usize = 2000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// "stdio" or "http" in the common format; inferred from `command` or `url` when missing.
    #[serde(default, rename = "type")]
    pub transport: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Profile IDs this server applies to; empty means every prompt.
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub allow_transcript: Option<bool>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Longest text kept from one tool result.
    #[serde(default)]
    pub max_chars: Option<usize>,
    #[serde(default)]
    pub hooks: Vec<Hook>,
    /// Tools the model may call on its own in the tool loop. Empty means none.
    #[serde(default)]
    pub loop_tools: Vec<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub tool: String,
    #[serde(default)]
    pub arguments: JsonObject,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    #[serde(default, alias = "mcpServers")]
    pub servers: BTreeMap<String, ServerConfig>,
}

impl McpConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config: McpConfig = serde_json::from_str(&text).map_err(|e| format!("mcp.json is invalid: {e}"))?;
                config.validate()?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("cannot read mcp.json: {e}")),
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (name, server) in &self.servers {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("mcp.json: server name {name:?} may only use letters, digits, - and _"));
            }
            if server.command.is_some() == server.url.is_some() {
                return Err(format!("mcp.json: server {name:?} needs exactly one of command or url"));
            }
            let expected = if server.url.is_some() { ["http", "streamable-http"].as_slice() } else { ["stdio"].as_slice() };
            if let Some(kind) = &server.transport
                && !expected.contains(&kind.as_str())
            {
                return Err(format!("mcp.json: server {name:?} has unsupported type {kind:?}"));
            }
            if !server.loop_tools.is_empty() && !server.transcript_allowed() {
                return Err(format!("mcp.json: server {name:?} has loop_tools but may not see the transcript; set allow_transcript to true"));
            }
        }
        Ok(())
    }
}

impl ServerConfig {
    pub fn transcript_allowed(&self) -> bool {
        self.allow_transcript.unwrap_or(self.url.is_none())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS))
    }

    pub fn max_chars(&self) -> usize {
        self.max_chars.unwrap_or(DEFAULT_MAX_CHARS).min(promptify_core::transform::MAX_TOOL_CONTEXT_CHARS)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedCall {
    pub server: String,
    pub tool: String,
    pub arguments: JsonObject,
}

fn fill(value: &serde_json::Value, vars: &[(&str, &str)], used_transcript: &mut bool) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            let mut out = s.clone();
            for (key, replacement) in vars {
                let token = format!("{{{key}}}");
                if out.contains(&token) {
                    *used_transcript |= *key == "transcript";
                    out = out.split(&token).collect::<Vec<_>>().join(replacement);
                }
            }
            serde_json::Value::String(out)
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(items.iter().map(|v| fill(v, vars, used_transcript)).collect()),
        serde_json::Value::Object(map) => serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), fill(v, vars, used_transcript))).collect()),
        other => other.clone(),
    }
}

/// Decides which tool calls to make for this request. Pure, so the gating rules are testable.
pub fn plan_calls(config: &McpConfig, profile_id: &str, transcript: &str, target: &ActiveContext) -> Vec<PlannedCall> {
    let app = target.normalized_process();
    let url = target.url_host().unwrap_or_default();
    let vars = [("transcript", transcript), ("app", app.as_str()), ("url", url.as_str())];
    let mut calls = Vec::new();
    for (name, server) in config.servers.iter().filter(|(_, s)| s.enabled).take(MAX_SERVERS) {
        if !server.profiles.is_empty() && !server.profiles.iter().any(|p| p == profile_id) {
            continue;
        }
        for hook in server.hooks.iter().take(MAX_HOOKS_PER_SERVER) {
            let mut used_transcript = false;
            let filled = fill(&serde_json::Value::Object(hook.arguments.clone()), &vars, &mut used_transcript);
            if used_transcript && !server.transcript_allowed() {
                log::info!("mcp: skipped {name}/{} (transcript not allowed for this server)", hook.tool);
                continue;
            }
            let serde_json::Value::Object(arguments) = filled else { continue };
            calls.push(PlannedCall { server: name.clone(), tool: hook.tool.clone(), arguments });
        }
    }
    calls
}

type Client = RunningService<RoleClient, ()>;

/// Holds one connection per configured server for the life of the app. Servers are queried in
/// parallel; calls to the same server run one after another.
pub struct McpEnricher {
    config: McpConfig,
    runtime: tokio::runtime::Runtime,
    clients: HashMap<String, tokio::sync::Mutex<Option<Client>>>,
    /// Tool descriptions for the tool loop, fetched once per server.
    loop_specs: std::sync::Mutex<HashMap<String, Vec<ToolSpec>>>,
}

/// Connects to one configured server.
async fn connect(server: &ServerConfig) -> Result<Client, String> {
    if let Some(url) = &server.url {
        let transport = rmcp::transport::StreamableHttpClientTransport::from_uri(url.as_str());
        return ().serve(transport).await.map_err(|e| e.to_string());
    }
    let mut command = tokio::process::Command::new(server.command.as_deref().unwrap_or_default());
    command.args(&server.args).envs(&server.env).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let transport = rmcp::transport::TokioChildProcess::new(command).map_err(|e| e.to_string())?;
    ().serve(transport).await.map_err(|e| e.to_string())
}

/// Starts a server, lists its tools and stops it again. Used by the settings page's "Test" button.
pub fn probe_server(server: &ServerConfig, timeout: Duration) -> Result<Vec<String>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        tokio::time::timeout(timeout, async {
            let client = connect(server).await?;
            let tools = client.list_all_tools().await.map_err(|e| e.to_string())?;
            let _ = client.cancel().await;
            Ok(tools.into_iter().map(|t| t.name.into_owned()).collect())
        })
        .await
        .map_err(|_| "the server did not answer in time".to_string())?
    })
}

fn text_of(result: &rmcp::model::CallToolResult, max_chars: usize) -> Option<(String, bool)> {
    if result.is_error == Some(true) {
        return None;
    }
    let text: Vec<String> = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
    let text = text.join("\n");
    if text.trim().is_empty() {
        return None;
    }
    let truncated = text.chars().count() > max_chars;
    Some((if truncated { text.chars().take(max_chars).collect() } else { text }, truncated))
}

impl McpEnricher {
    /// None when no server has hooks or loop tools; the transform service then skips tools entirely.
    pub fn from_config(config: McpConfig) -> Result<Option<Self>, String> {
        if !config.servers.values().any(|s| s.enabled && (!s.hooks.is_empty() || !s.loop_tools.is_empty())) {
            return Ok(None);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("promptify-mcp")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let clients = config.servers.keys().map(|name| (name.clone(), tokio::sync::Mutex::new(None))).collect();
        Ok(Some(Self { config, runtime, clients, loop_specs: Default::default() }))
    }

    pub fn config(&self) -> &McpConfig {
        &self.config
    }

    /// Runs `call` on the named server, connecting first if needed. A failed call drops the connection.
    async fn with_client<T>(&self, name: &str, timeout: Duration, call: impl AsyncFnOnce(&Client) -> Result<T, String>) -> Option<T> {
        let server = self.config.servers.get(name)?;
        let mut slot = self.clients.get(name)?.lock().await;
        if slot.is_none() {
            match tokio::time::timeout(timeout, connect(server)).await {
                Ok(Ok(client)) => *slot = Some(client),
                _ => {
                    log::warn!("mcp: could not start server {name}");
                    return None;
                }
            }
        }
        let client = slot.as_ref()?;
        match tokio::time::timeout(timeout, call(client)).await {
            Ok(Ok(value)) => Some(value),
            Ok(Err(_)) => {
                *slot = None;
                None
            }
            Err(_) => None,
        }
    }

    async fn run_call(&self, call: &PlannedCall, deadline: Instant) -> Option<ToolContext> {
        let server = self.config.servers.get(&call.server)?;
        let timeout = server.timeout().min(deadline.saturating_duration_since(Instant::now()));
        let request = CallToolRequestParams::new(call.tool.clone()).with_arguments(call.arguments.clone());
        let result = self.with_client(&call.server, timeout, async |client| client.call_tool(request).await.map_err(|e| e.to_string())).await?;
        let (text, truncated) = text_of(&result, server.max_chars())?;
        Some(ToolContext { source: format!("{}/{}", call.server, call.tool), text, truncated })
    }

    /// Each server's hooks run in order; different servers run at the same time.
    async fn collect(&self, calls: Vec<PlannedCall>, deadline: Instant, cancel: &CancelToken) -> Vec<ToolContext> {
        let mut by_server: Vec<(String, Vec<PlannedCall>)> = Vec::new();
        for call in calls {
            match by_server.iter_mut().find(|(name, _)| *name == call.server) {
                Some((_, list)) => list.push(call),
                None => by_server.push((call.server.clone(), vec![call])),
            }
        }
        let per_server = by_server.iter().map(|(_, list)| async move {
            let mut out = Vec::new();
            for call in list {
                if cancel.is_cancelled() || Instant::now() >= deadline {
                    break;
                }
                if let Some(context) = self.run_call(call, deadline).await {
                    out.push(context);
                }
            }
            out
        });
        futures_util::future::join_all(per_server).await.into_iter().flatten().collect()
    }

    /// Runs `future` on this enricher's runtime from any thread, bounded by `deadline`.
    fn block_on<T: Send + Default>(&self, deadline: Instant, future: impl Future<Output = T> + Send) -> T {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // A scoped thread keeps this callable from engine threads that belong to another async runtime.
        std::thread::scope(|scope| scope.spawn(|| self.runtime.block_on(async { tokio::time::timeout(remaining, future).await.unwrap_or_default() })).join().unwrap_or_default())
    }

    async fn fetch_specs(&self, name: &str, allowed: &[String], timeout: Duration) -> Vec<ToolSpec> {
        let tools = self.with_client(name, timeout, async |client| client.list_all_tools().await.map_err(|e| e.to_string())).await.unwrap_or_default();
        tools
            .into_iter()
            .filter(|t| allowed.iter().any(|a| a == t.name.as_ref()))
            .map(|t| ToolSpec {
                name: format!("{name}.{}", t.name),
                description: t.description.map(|d| d.into_owned()).unwrap_or_default(),
                parameters: t.input_schema.get("properties").and_then(|p| p.as_object()).map(|p| p.keys().cloned().collect()).unwrap_or_default(),
            })
            .collect()
    }
}

impl ContextEnricher for McpEnricher {
    fn enrich(&self, profile_id: &str, transcript: &str, target: &ActiveContext, deadline: Instant, cancel: &CancelToken) -> Vec<ToolContext> {
        let calls = plan_calls(&self.config, profile_id, transcript, target);
        if calls.is_empty() {
            return Vec::new();
        }
        self.block_on(deadline, self.collect(calls, deadline, cancel))
    }

    fn loop_tools(&self, profile_id: &str) -> Vec<ToolSpec> {
        let servers = loop_servers(&self.config, profile_id);
        let missing: Vec<&str> = servers.iter().filter(|name| !self.loop_specs.lock().unwrap().contains_key(**name)).copied().collect();
        if !missing.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(3);
            let fetched: Vec<(String, Vec<ToolSpec>)> = self.block_on(deadline, async {
                let lookups = missing.iter().map(|name| async move {
                    let allowed = &self.config.servers[*name].loop_tools;
                    (name.to_string(), self.fetch_specs(name, allowed, Duration::from_secs(3)).await)
                });
                futures_util::future::join_all(lookups).await
            });
            let mut cache = self.loop_specs.lock().unwrap();
            for (name, specs) in fetched {
                // An unreachable server is asked again next time instead of being cached as empty.
                if !specs.is_empty() {
                    cache.insert(name, specs);
                }
            }
        }
        let cache = self.loop_specs.lock().unwrap();
        servers.iter().filter_map(|name| cache.get(*name)).flatten().cloned().collect()
    }

    fn call_tool(&self, request: &ToolRequest, deadline: Instant, cancel: &CancelToken) -> Option<ToolContext> {
        let (server, tool) = request.tool.split_once('.')?;
        // Checked again here: only allowlisted tools on servers trusted with the transcript.
        let config = self.config.servers.get(server)?;
        if !config.enabled || !config.transcript_allowed() || !config.loop_tools.iter().any(|t| t == tool) || cancel.is_cancelled() {
            return None;
        }
        let call = PlannedCall { server: server.to_owned(), tool: tool.to_owned(), arguments: request.arguments.clone() };
        self.block_on(deadline, async { self.run_call(&call, deadline).await })
    }
}

/// Servers whose loop tools may be offered to the model for this profile.
pub fn loop_servers<'a>(config: &'a McpConfig, profile_id: &str) -> Vec<&'a str> {
    config
        .servers
        .iter()
        .filter(|(_, s)| s.enabled && !s.loop_tools.is_empty() && s.transcript_allowed())
        .filter(|(_, s)| s.profiles.is_empty() || s.profiles.iter().any(|p| p == profile_id))
        .take(MAX_SERVERS)
        .map(|(name, _)| name.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> McpConfig {
        serde_json::from_str(json).unwrap()
    }

    fn target() -> ActiveContext {
        ActiveContext { process_name: "Cursor.exe".into(), url: Some("https://example.com/x".into()), ..Default::default() }
    }

    #[test]
    fn plans_templated_calls_for_matching_profiles() {
        let c = config(r#"{"servers":{"docs":{"command":"x","profiles":["cursor"],"hooks":[{"tool":"search","arguments":{"query":"{transcript} in {app}","site":["{url}"],"n":3}}]}}}"#);
        let calls = plan_calls(&c, "cursor", "fix login", &target());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["query"], "fix login in cursor");
        assert_eq!(calls[0].arguments["site"][0], "example.com");
        assert_eq!(calls[0].arguments["n"], 3);
        assert!(plan_calls(&c, "chatgpt", "fix login", &target()).is_empty());
    }

    #[test]
    fn remote_servers_never_get_the_transcript_unless_allowed() {
        let remote = config(r#"{"servers":{"r":{"url":"https://mcp.example.com/mcp","hooks":[{"tool":"s","arguments":{"q":"{transcript}"}},{"tool":"t","arguments":{"q":"{app}"}}]}}}"#);
        let calls = plan_calls(&remote, "generic", "SECRET words", &target());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "t");
        assert!(!format!("{calls:?}").contains("SECRET"));
        let allowed = config(r#"{"servers":{"r":{"url":"https://mcp.example.com/mcp","allow_transcript":true,"hooks":[{"tool":"s","arguments":{"q":"{transcript}"}}]}}}"#);
        assert_eq!(plan_calls(&allowed, "generic", "SECRET", &target()).len(), 1);
        let local_denied = config(r#"{"servers":{"l":{"command":"x","allow_transcript":false,"hooks":[{"tool":"s","arguments":{"q":"{transcript}"}}]}}}"#);
        assert!(plan_calls(&local_denied, "generic", "SECRET", &target()).is_empty());
    }

    #[test]
    fn disabled_servers_and_excess_hooks_are_ignored() {
        let hooks: Vec<String> = (0..10).map(|i| format!(r#"{{"tool":"t{i}"}}"#)).collect();
        let c = config(&format!(r#"{{"servers":{{"a":{{"command":"x","hooks":[{}]}},"b":{{"command":"x","enabled":false,"hooks":[{{"tool":"z"}}]}}}}}}"#, hooks.join(",")));
        let calls = plan_calls(&c, "generic", "hi", &target());
        assert_eq!(calls.len(), MAX_HOOKS_PER_SERVER);
        assert!(calls.iter().all(|c| c.server == "a"));
    }

    #[test]
    fn config_validation_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(McpConfig::load(&dir.path().join("mcp.json")).unwrap().servers.is_empty());
        let path = dir.path().join("bad.json");
        std::fs::write(&path, r#"{"servers":{"x":{"hooks":[]}}}"#).unwrap();
        assert!(McpConfig::load(&path).is_err());
        std::fs::write(&path, r#"{"servers":{"x":{"command":"a","url":"http://b","hooks":[]}}}"#).unwrap();
        assert!(McpConfig::load(&path).is_err());
        std::fs::write(&path, r#"{"servers":{"x":{"command":"a","bogus":1}}}"#).unwrap();
        assert!(McpConfig::load(&path).is_err());
        assert!(McpEnricher::from_config(McpConfig::default()).unwrap().is_none());
    }

    fn validate(json: &str) -> Result<(), String> {
        config(json).validate()
    }

    #[test]
    fn accepts_the_common_format_and_checks_new_fields() {
        let c = config(r#"{"mcpServers":{"fs":{"type":"stdio","command":"npx","args":["-y","server"]},"web":{"type":"http","url":"https://x.example/mcp"}}}"#);
        assert!(c.validate().is_ok());
        assert_eq!(c.servers.len(), 2);
        assert!(validate(r#"{"mcpServers":{"web":{"type":"sse","url":"https://x.example/sse"}}}"#).is_err(), "sse is not supported");
        assert!(validate(r#"{"mcpServers":{"fs":{"type":"http","command":"npx"}}}"#).is_err(), "type must match command/url");
        assert!(validate(r#"{"servers":{"a.b":{"command":"x"}}}"#).is_err(), "dots would make loop tool names ambiguous");
        assert!(validate(r#"{"servers":{"r":{"url":"https://x.example/mcp","loop_tools":["search"]}}}"#).is_err(), "loop tools see the transcript");
        assert!(validate(r#"{"servers":{"r":{"url":"https://x.example/mcp","allow_transcript":true,"loop_tools":["search"]}}}"#).is_ok());
        let caps = config(r#"{"servers":{"a":{"command":"x"},"b":{"command":"x","max_chars":500},"c":{"command":"x","max_chars":99999}}}"#);
        assert_eq!(caps.servers["a"].max_chars(), DEFAULT_MAX_CHARS);
        assert_eq!(caps.servers["b"].max_chars(), 500);
        assert_eq!(caps.servers["c"].max_chars(), promptify_core::transform::MAX_TOOL_CONTEXT_CHARS);
    }

    #[test]
    fn loop_tools_are_offered_only_where_allowed() {
        let c = config(
            r#"{"servers":{
                "docs":{"command":"x","profiles":["cursor"],"loop_tools":["search"]},
                "off":{"command":"x","enabled":false,"loop_tools":["search"]},
                "remote":{"url":"https://x.example/mcp","loop_tools":["search"]},
                "plain":{"command":"x","hooks":[{"tool":"t"}]}}}"#,
        );
        assert_eq!(loop_servers(&c, "cursor"), vec!["docs"]);
        assert!(loop_servers(&c, "chatgpt").is_empty());
    }

    #[test]
    fn tool_results_are_capped_and_errors_dropped() {
        use rmcp::model::{CallToolResult, ContentBlock};
        let ok = CallToolResult::success(vec![ContentBlock::text("abcdef")]);
        assert_eq!(text_of(&ok, 4), Some(("abcd".into(), true)));
        assert_eq!(text_of(&ok, 10), Some(("abcdef".into(), false)));
        assert_eq!(text_of(&CallToolResult::error(vec![ContentBlock::text("boom")]), 10), None);
        assert_eq!(text_of(&CallToolResult::success(vec![ContentBlock::text("  ")]), 10), None);
    }
}
