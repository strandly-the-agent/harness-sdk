//! Strands Agents SDK for Rust.
//!
//! Strands takes a model-driven approach to building AI agents: an agent is a model, a system
//! prompt and a set of tools, and the model's own reasoning decides which tools to call.
//!
//! This crate is a developer preview that tracks the TypeScript SDK (`@strands-agents/sdk`).
//! Names, concepts and wire formats match it, re-cased for Rust.
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

pub mod error;
pub mod interrupt;
pub(crate) mod mime;
pub mod models;
pub mod sandbox;
pub mod tools;
pub mod types;

#[cfg(test)]
mod test_support;

pub use error::{Error, Result};
pub use types::{ContentBlock, InterruptResponse, JsonValue, Message, StopReason};
