//! MCP (Model Context Protocol) client over [`rmcp`] (`strands-ts/src/mcp`).
//!
//! ```no_run
//! use strands_agents::mcp::{McpClient, McpServerConfig};
//!
//! # async fn demo() -> strands_agents::Result<()> {
//! let client = McpClient::connect(McpServerConfig::stdio("uvx", ["mcp-server-fetch"])).await?;
//! let tools = client.list_tools().await?;
//! # Ok(())
//! # }
//! ```

mod client;
mod config;
mod tool;

pub use client::{
    ListOptions, LoadOptions, McpClient, McpConnectionState, McpServerInfo, ToolsChangedCallback,
};

pub use config::{
    ConfigSource, McpConfigFile, McpServerConfig, McpToolFilters, McpTransportKind, ResolvedTransport,
};
/// Re-exported from `indexmap`: a public dependency of this module's config types
/// ([`ConfigSource::Inline`], [`ConfigSource::load`]), which keep server order.
pub use indexmap::IndexMap;
pub use tool::McpTool;
