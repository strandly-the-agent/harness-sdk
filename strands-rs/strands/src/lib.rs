//! Strands Agents SDK for Rust.
//!
//! Strands takes a model-driven approach to building AI agents: an agent is a model, a system
//! prompt and a set of tools, and the model's own reasoning decides which tools to call.
//!
//! This crate is a developer preview that tracks the TypeScript SDK (`@strands-agents/sdk`).
//! Names, concepts and wire formats match it, re-cased for Rust.
//!
//! ```
//! use strands::{ContentBlock, Message};
//! use strands::types::Role;
//!
//! let message = Message::new(Role::User, vec![ContentBlock::from("What's the weather?")]);
//! let json = serde_json::to_value(&message)?;
//! assert_eq!(json["role"], "user");
//! assert_eq!(json["content"], serde_json::json!([{ "text": "What's the weather?" }]));
//! # Ok::<(), serde_json::Error>(())
//! ```
//!
//! # Cargo features
//!
//! | Feature | Enables |
//! |---|---|
//! | `bedrock` (default) | Amazon Bedrock model provider |
//! | `anthropic` | Anthropic model provider |
//! | `openai` | OpenAI model provider (Bedrock Mantle also needs `bedrock`) |
//! | `mcp` | Model Context Protocol client |
//! | `otel` | OpenTelemetry export for traces and metrics |
//!
//! # Panics
//!
//! A tool that panics becomes an error tool result and the agent loop continues. That relies on
//! unwinding, so don't build binaries that use this crate with `panic = "abort"`.

pub mod agent;
pub mod cancel;
pub mod error;
pub mod hooks;
pub mod interrupt;
pub(crate) mod logging;
pub(crate) mod mime;
pub mod models;
pub mod retry;
pub mod sandbox;
pub mod state_store;
pub mod telemetry;
pub mod testing;
pub mod tools;
pub mod types;

#[cfg(test)]
mod test_support;

#[allow(unused_extern_crates, reason = "lets `#[tool]` expand inside this crate")]
extern crate self as strands;

pub use agent::{
    Agent, AgentConfig, AgentResult, AgentStream, AgentStreamEvent, InvocationState, InvokeArgs,
    InvokeOptions,
};
pub use cancel::{CancelHandle, CancelOptions, CancelSignal};
pub use error::{Error, Result};
pub use state_store::StateStore;
pub use strands_macros::{tool, tools};
pub use tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolSource};
pub use types::{ContentBlock, InterruptResponse, JsonValue, Message, StopReason};

#[doc(hidden)]
pub mod __private {
    pub use async_trait;
    pub use futures;
    pub use jsonschema;
    pub use schemars;
    pub use serde;
    pub use serde_json;
    pub use tokio;
}
