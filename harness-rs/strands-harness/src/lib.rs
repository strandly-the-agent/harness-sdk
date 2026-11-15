//! The Strands Agents harness for Rust.
//!
//! The harness builds a ready-to-run [`strands`] agent with built-in tools, plugins, sessions,
//! memory and context management, using the same defaults as the TypeScript harness.
//!
//! # Cargo features
//!
//! | Feature | Default | Enables |
//! |---|---|---|
//! | `bedrock` | yes | Bedrock models (bare and `bedrock/` model strings) |
//! | `anthropic` | no | `anthropic/` model strings |
//! | `openai` | no | `openai/` model strings (`bedrock-mantle/` also needs `bedrock`) |
//! | `otel` | no | OpenTelemetry export of traces and metrics |
//!
//! MCP support is always on.
//!
//! # Panics
//!
//! A tool that panics becomes an error tool result, which needs unwinding. Never build with
//! `panic = "abort"`.
