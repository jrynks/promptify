//! MCP for Promptify.
//!
//! - [`server`]: `promptify-cli mcp` exposes `transform_prompt`, `clean_dictation` and `list_profiles`
//!   to MCP clients (Claude Desktop, VS Code, Cursor...) by forwarding to the running app's local API.
//! - [`client`]: fetches reference text from user-configured MCP servers before a prompt is written.

pub mod client;
pub mod server;
