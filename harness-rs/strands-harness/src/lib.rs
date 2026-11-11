//! The Strands Agents harness for Rust.
//!
//! The harness builds a ready-to-run [Strands](https://strandsagents.com) agent with built-in
//! tools, plugins, sessions and memory, using the same defaults as the TypeScript harness
//! (`@strands-agents/harness`).
//!
//! # Cargo features
//!
//! | Feature | Enables |
//! |---|---|
//! | `bedrock` (default) | `bedrock/` model strings (a bare model id means Bedrock) |
//! | `anthropic` | `anthropic/` model strings |
//! | `openai` | `openai/` model strings, and `bedrock-mantle/` together with `bedrock` |
//! | `otel` | OpenTelemetry export |
