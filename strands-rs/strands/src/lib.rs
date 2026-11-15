//! Strands Agents SDK for Rust.
//!
//! Strands is a model-driven SDK for building AI agents: an agent is a model, a system prompt and a
//! list of tools, and the model's own reasoning decides which tools to call. This crate mirrors the
//! TypeScript SDK (`@strands-agents/sdk`) concept for concept, re-cased for Rust.
//!
//! ```
//! use serde_json::json;
//! use strands::types::messages::{Role, TextBlock, ToolResultBlock, ToolUseBlock};
//! use strands::types::tools::ToolResultStatus;
//! use strands::Message;
//!
//! let message = Message::new(
//!     Role::User,
//!     vec![
//!         TextBlock::new("What's the weather?").into(),
//!         ToolUseBlock::new("weather", "tooluse_1", json!({"city": "Seattle"})).into(),
//!         ToolResultBlock::new("tooluse_1", ToolResultStatus::Success, vec!["Sunny".into()]).into(),
//!     ],
//! );
//! let wire = serde_json::to_value(&message)?;
//! assert_eq!(wire["role"], "user");
//! assert_eq!(
//!     wire["content"],
//!     json!([
//!         {"text": "What's the weather?"},
//!         {"toolUse": {"name": "weather", "toolUseId": "tooluse_1", "input": {"city": "Seattle"}}},
//!         {"toolResult": {"toolUseId": "tooluse_1", "status": "success", "content": [{"text": "Sunny"}]}}
//!     ])
//! );
//! # Ok::<(), strands::Error>(())
//! ```
//!
//! # Cargo features
//!
//! - `bedrock` (default): the Amazon Bedrock model provider and the default model.
//! - `anthropic`: the Anthropic model provider.
//! - `openai`: the OpenAI model provider; with `bedrock`, also Bedrock Mantle.
//! - `mcp`: the Model Context Protocol client.
//! - `otel`: OpenTelemetry export.
//!
//! # Panics and unwinding
//!
//! A panicking tool becomes an error tool result, which relies on unwinding. Don't build with
//! `panic = "abort"`.

#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
mod open_enum;

pub mod agent;
pub mod cancel;
pub mod error;
pub mod hooks;
pub mod interrupt;
mod logging;
mod mime;
pub mod models;
pub mod state_store;
pub mod testing;
pub mod types;

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod test_support;

pub use cancel::{CancelHandle, CancelOptions, CancelSignal};
pub use error::{Error, Result};
pub use interrupt::InterruptResponse;
pub use state_store::StateStore;
pub use types::json::JsonValue;
pub use types::messages::{ContentBlock, Message, StopReason};
