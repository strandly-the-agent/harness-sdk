//! Strands Agents SDK for Rust.
//!
//! Strands is a model-driven SDK for building AI agents: an agent is a model, a system prompt and a
//! set of tools, and the model's own reasoning decides which tools to call. This crate tracks the
//! Strands Agents TypeScript SDK and shares its concepts, names and wire formats.
//!
//! # Cargo features
//!
//! | Feature | Default | Enables |
//! |---|---|---|
//! | `bedrock` | yes | `BedrockModel` and the default model |
//! | `anthropic` | no | `AnthropicModel` |
//! | `openai` | no | `OpenAIModel` (Bedrock Mantle also needs `bedrock`) |
//! | `mcp` | no | `McpClient` |
//! | `otel` | no | OpenTelemetry export of traces and metrics |
//!
//! # Panics
//!
//! A tool that panics becomes an error tool result, which needs unwinding. Never build with
//! `panic = "abort"`.

#[allow(
    unused_extern_crates,
    reason = "lets `#[tool]` expansions inside this crate name `::strands`"
)]
extern crate self as strands;

#[macro_use]
mod macros;

pub mod agent;
pub mod error;
pub mod interrupt;
pub mod mime;
pub mod models;
pub mod retry;
pub mod state_store;
pub mod types;

pub use error::{Error, Result};
pub use interrupt::InterruptResponse;
pub use state_store::StateStore;
pub use types::json::JsonValue;
pub use types::messages::{ContentBlock, Message, StopReason};
