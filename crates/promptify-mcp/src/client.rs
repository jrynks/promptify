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
//! transcript unless `"allow_transcript": true` is set for them.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use promptify_core::context::ActiveContext;
use promptify_core::pipeline::CancelToken;
use promptify_core::prompt::ToolContext;
use promptify_core::transform::ContextEnricher;
use rmcp::model::{CallToolRequestParams, JsonObject};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde::Deserialize;

pub const MAX_SERVERS: usize = 8;
pub const MAX_HOOKS_PER_SERVER: usize = 4;
const DEFAULT_TIMEOUT_MS: u64 = 3000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
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
    #[serde(default)]
    pub hooks: Vec<Hook>,
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
    #[serde(default)]
    pub servers: BTreeMap<String, ServerConfig>,
}

impl McpConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config: McpConfig = serde_json::from_str(&text).map_err(|e| format!("mcp.json is invalid: {e}"))?;
                for (name, server) in &config.servers {
                    if server.command.is_some() == server.url.is_some() {
                        return Err(format!("mcp.json: server {name:?} needs exactly one of command or url"));
                    }
                }
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("cannot read mcp.json: {e}")),
        }
    }
}

impl ServerConfig {
    fn transcript_allowed(&self) -> bool {
        self.allow_transcript.unwrap_or(self.url.is_none())
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

/// Holds connections to the configured servers for the life of the app.
pub struct McpEnricher {
    config: McpConfig,
    runtime: tokio::runtime::Runtime,
    clients: tokio::sync::Mutex<HashMap<String, Client>>,
}

impl McpEnricher {
    /// None when no server has hooks; the transform service then skips enrichment entirely.
    pub fn from_config(config: McpConfig) -> Result<Option<Self>, String> {
        if !config.servers.values().any(|s| s.enabled && !s.hooks.is_empty()) {
            return Ok(None);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("promptify-mcp")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Some(Self { config, runtime, clients: Default::default() }))
    }

    async fn connect(&self, name: &str) -> Result<Client, String> {
        let server = self.config.servers.get(name).ok_or("unknown server")?;
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

    async fn call(&self, call: &PlannedCall, timeout: Duration) -> Option<ToolContext> {
        let mut clients = self.clients.lock().await;
        if !clients.contains_key(&call.server) {
            match tokio::time::timeout(timeout, self.connect(&call.server)).await {
                Ok(Ok(client)) => {
                    clients.insert(call.server.clone(), client);
                }
                _ => {
                    log::warn!("mcp: could not start server {}", call.server);
                    return None;
                }
            }
        }
        let client = clients.get(&call.server)?;
        let request = CallToolRequestParams::new(call.tool.clone()).with_arguments(call.arguments.clone());
        let result = match tokio::time::timeout(timeout, client.call_tool(request)).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // A broken connection is dropped and reopened next time.
                clients.remove(&call.server);
                return None;
            }
            Err(_) => return None,
        };
        if result.is_error == Some(true) {
            return None;
        }
        let text: Vec<String> = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
        let text = text.join("\n");
        (!text.trim().is_empty()).then(|| ToolContext { source: format!("{}/{}", call.server, call.tool), text, truncated: false })
    }

    async fn collect(&self, calls: Vec<PlannedCall>, deadline: Instant, cancel: &CancelToken) -> Vec<ToolContext> {
        let mut out = Vec::new();
        for call in calls {
            let now = Instant::now();
            if cancel.is_cancelled() || now >= deadline {
                break;
            }
            let per_call = Duration::from_millis(self.config.servers.get(&call.server).and_then(|s| s.timeout_ms).unwrap_or(DEFAULT_TIMEOUT_MS));
            if let Some(context) = self.call(&call, per_call.min(deadline - now)).await {
                out.push(context);
            }
        }
        out
    }
}

impl ContextEnricher for McpEnricher {
    fn enrich(&self, profile_id: &str, transcript: &str, target: &ActiveContext, deadline: Instant, cancel: &CancelToken) -> Vec<ToolContext> {
        let calls = plan_calls(&self.config, profile_id, transcript, target);
        if calls.is_empty() {
            return Vec::new();
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        // A scoped thread keeps this callable from engine threads that belong to another async runtime.
        std::thread::scope(|scope| {
            scope
                .spawn(|| self.runtime.block_on(async { tokio::time::timeout(remaining, self.collect(calls, deadline, cancel)).await.unwrap_or_default() }))
                .join()
                .unwrap_or_default()
        })
    }
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
}
