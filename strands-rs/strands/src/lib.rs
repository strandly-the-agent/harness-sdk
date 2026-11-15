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
