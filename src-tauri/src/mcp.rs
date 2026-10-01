//! Settings page support for MCP context hooks: show the configured servers, test one, reload.

use promptify_core::pipeline::Orchestrator;
use promptify_mcp::client::{McpConfig, McpEnricher, STARTUP_TIMEOUT, probe_server};
use serde::Serialize;
use tauri::State;

use crate::AppState;

#[derive(Serialize)]
pub struct ServerInfo {
    name: String,
    remote: bool,
    enabled: bool,
    profiles: Vec<String>,
    hooks: usize,
    loop_tools: Vec<String>,
    transcript_allowed: bool,
}

#[derive(Serialize)]
pub struct McpInfo {
    path: String,
    error: Option<String>,
    active: bool,
    servers: Vec<ServerInfo>,
}

fn config_path(state: &AppState) -> std::path::PathBuf {
    state.data_dir.join("mcp.json")
}

/// Reads `mcp.json` and installs (or removes) the enricher. Returns the config that is now active.
pub fn reload(data_dir: &std::path::Path, orchestrator: &Orchestrator) -> Result<McpConfig, String> {
    let config = McpConfig::load(&data_dir.join("mcp.json"));
    let enricher = config.clone().and_then(McpEnricher::from_config);
    match enricher {
        Ok(Some(enricher)) => {
            orchestrator.service().set_enricher(Some(std::sync::Arc::new(enricher)));
            log::info!("mcp: context hooks enabled");
        }
        Ok(None) => orchestrator.service().set_enricher(None),
        Err(e) => {
            // A broken file turns tools off rather than keeping a stale configuration.
            orchestrator.service().set_enricher(None);
            log::warn!("mcp: {e}");
            return Err(e);
        }
    }
    config
}

fn info(state: &AppState, result: Result<McpConfig, String>) -> McpInfo {
    let (config, error) = match result {
        Ok(config) => (config, None),
        Err(e) => (McpConfig::default(), Some(e)),
    };
    let servers = config
        .servers
        .iter()
        .map(|(name, s)| ServerInfo {
            name: name.clone(),
            remote: s.url.is_some(),
            enabled: s.enabled,
            profiles: s.profiles.clone(),
            hooks: s.hooks.len(),
            loop_tools: s.loop_tools.clone(),
            transcript_allowed: s.transcript_allowed(),
        })
        .collect();
    let active = error.is_none() && config.servers.values().any(|s| s.enabled && (!s.hooks.is_empty() || !s.loop_tools.is_empty()));
    McpInfo { path: config_path(state).to_string_lossy().into_owned(), error, active, servers }
}

#[tauri::command]
pub fn mcp_info(state: State<'_, AppState>) -> McpInfo {
    info(&state, McpConfig::load(&config_path(&state)))
}

#[tauri::command]
pub fn mcp_reload(state: State<'_, AppState>) -> McpInfo {
    info(&state, reload(&state.data_dir, &state.orchestrator))
}

/// Starts the named server, lists its tools and stops it again.
#[tauri::command]
pub async fn mcp_test(state: State<'_, AppState>, name: String) -> Result<Vec<String>, String> {
    let config = McpConfig::load(&config_path(&state))?;
    let server = config.servers.get(&name).cloned().ok_or("no server with that name in mcp.json")?;
    tauri::async_runtime::spawn_blocking(move || probe_server(&server, STARTUP_TIMEOUT)).await.map_err(|e| e.to_string())?
}

const TEMPLATE: &str = r#"{
  "mcpServers": {
    "docs": {
      "type": "stdio",
      "command": "uvx",
      "args": ["some-mcp-server"],
      "profiles": ["cursor", "claude_code"],
      "timeout_ms": 3000,
      "hooks": [{ "tool": "search", "arguments": { "query": "{transcript}" } }]
    }
  }
}
"#;

#[derive(Serialize)]
pub struct McpFile {
    /// The file as it is on disk; empty when it does not exist yet.
    text: String,
    exists: bool,
    template: &'static str,
}

#[tauri::command]
pub fn mcp_read(state: State<'_, AppState>) -> Result<McpFile, String> {
    match std::fs::read_to_string(config_path(&state)) {
        Ok(text) => Ok(McpFile { text, exists: true, template: TEMPLATE }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(McpFile { text: String::new(), exists: false, template: TEMPLATE }),
        Err(e) => Err(format!("cannot read mcp.json: {e}")),
    }
}

/// Checks text exactly as loading would, without touching the file. Returns the number of servers.
#[tauri::command]
pub fn mcp_validate(text: String) -> Result<usize, String> {
    McpConfig::parse(&text).map(|c| c.servers.len())
}

/// Saves valid text over the version the editor opened, then applies it right away.
#[tauri::command]
pub fn mcp_save(state: State<'_, AppState>, text: String, original: String) -> Result<McpInfo, String> {
    McpConfig::save(&config_path(&state), &text, &original)?;
    Ok(info(&state, reload(&state.data_dir, &state.orchestrator)))
}
