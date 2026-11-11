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
pub mod interrupt;
pub(crate) mod logging;
pub(crate) mod mime;
pub mod models;
pub mod retry;
pub mod sandbox;
pub mod state_store;
pub mod tools;
pub mod types;

#[cfg(test)]
mod test_support;

pub use cancel::{CancelHandle, CancelOptions, CancelSignal};
pub use error::{Error, Result};
pub use state_store::StateStore;
pub use types::{ContentBlock, InterruptResponse, JsonValue, Message, StopReason};
