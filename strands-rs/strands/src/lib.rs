//! Strands Agents: a model-driven SDK for building AI agents.
//!
//! An agent combines a model, a system prompt and a set of tools, and lets the model decide which
//! tools to call and when to stop.
//!
//! # Cargo features
//!
//! - `bedrock` (default): the Amazon Bedrock model provider.
//! - `anthropic`: the Anthropic model provider.
//! - `openai`: the OpenAI model provider. Together with `bedrock` it also enables Bedrock Mantle.
//! - `mcp`: the Model Context Protocol client.
//! - `otel`: OpenTelemetry tracing and metrics export.
//!
//! # Panics
//!
//! A panic inside a tool or hook is turned into an error result, which needs unwinding. Don't build
//! with `panic = "abort"`.
