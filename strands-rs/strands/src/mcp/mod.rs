//! Model Context Protocol client (TS `src/mcp/`), on [`rmcp`].
//!
//! An [`McpClient`] connects to one server over stdio, Streamable HTTP or the legacy HTTP+SSE
//! transport, and an agent takes it as a tool source: `Agent::initialize` lists its tools and
//! keeps them current when the server reports a tools-list change.
//!
//! ```no_run
//! use strands::mcp::{McpClient, McpClientConfig, McpTransport, StdioServerParameters};
//!
//! # async fn run() -> strands::Result<()> {
//! let client = McpClient::new(McpClientConfig {
//!     transport: Some(McpTransport::stdio(StdioServerParameters {
//!         command: "uvx".into(),
//!         args: vec!["awslabs.aws-documentation-mcp-server@latest".into()],
//!         ..Default::default()
//!     })),
//!     ..Default::default()
//! })?;
//! let mut agent = strands::Agent::builder().tools([client.clone()]).build()?;
//! agent.invoke("What is Amazon Bedrock?").await?;
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
//!
//! The MCP wire types (results, capabilities, elicitation and logging payloads) are `rmcp`'s,
//! re-exported as [`rmcp`], as the TS client exposes the MCP SDK types. [`regex`] (for
//! [`McpToolMatcher::Pattern`]) and [`reqwest`] (for [`McpAuthProvider`]) are re-exported too.

pub(crate) mod agent_tools;
mod auth;
mod client;
pub(crate) mod config;
mod elicitation;
mod error;
mod filters;
pub(crate) mod handler;
mod mcp_tool;
mod sse;
pub(crate) mod trace;
pub(crate) mod transport;

pub use regex;
pub use reqwest;
pub use rmcp;

pub use client::{
    DEFAULT_CLIENT_NAME, McpAuthProvider, McpCallToolOptions, McpClient, McpClientConfig,
    McpClientCredentials, McpClientOptions, McpConnectionState, McpListOptions, McpListToolsOptions,
    McpLogHandler, McpProgress, McpProgressCallback, McpToolsChangedCallback,
};
pub use config::{McpLoadServersOptions, McpServerConfig, McpServers, SerializableMcpToolFilters};
pub use elicitation::{ElicitationCallback, ElicitationContext};
pub use error::{McpError, McpErrorKind};
pub use filters::{McpToolFilterCallback, McpToolFilters, McpToolMatcher};
pub use mcp_tool::McpTool;
pub use transport::{McpTransport, StdioServerParameters};

#[cfg(test)]
pub(crate) mod tests;
