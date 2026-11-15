//! Strands Agents SDK for Rust.
//!
//! Strands is a model-driven SDK for building AI agents: an agent is a model, a system prompt and a
//! list of tools, and the model's own reasoning decides which tools to call. This crate mirrors the
//! TypeScript SDK (`@strands-agents/sdk`) concept for concept, re-cased for Rust.
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
pub mod interrupt;
mod mime;
pub mod models;
pub mod state_store;
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
