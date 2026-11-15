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

#[cfg_attr(
    not(test),
    expect(
        unused_extern_crates,
        reason = "`#[tool]` code in this crate names `::strands`; only tests use it yet"
    )
)]
extern crate self as strands;

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
pub mod retry;
pub mod state_store;
pub mod testing;
pub mod tools;
pub mod types;

#[doc(hidden)]
pub mod __private {
    //! Re-exports for macro-generated code; not a public API.
    pub use async_trait;
    pub use jsonschema;
    pub use schemars;
    pub use serde;
    pub use serde_json;

    pub use crate::tools::function_tool::InputValidator;
    pub use crate::tools::macro_support::{LazyToolDef, ToolDef, blocking, result_output, value_output};
    pub use crate::tools::{Tool, ToolContext, ToolSource, ToolStream};
    pub use crate::types::tools::ToolSpec;

    /// The normalized input schema for `T`, as typed tools use it.
    pub fn input_schema_for<T: schemars::JsonSchema>() -> crate::JsonValue {
        crate::tools::schema::input_schema_for::<T>()
    }

    /// A handler value as a tool result (the `FunctionTool` wrapping rules).
    pub fn wrap_in_tool_result(
        value: crate::JsonValue,
        tool_use_id: &str,
    ) -> crate::types::messages::ToolResultBlock {
        crate::tools::function_tool::wrap_in_tool_result(value, tool_use_id)
    }
}

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod test_support;

pub use async_trait::async_trait;
pub use cancel::{CancelHandle, CancelOptions, CancelSignal};
pub use error::{Error, Result};
pub use interrupt::InterruptResponse;
pub use state_store::StateStore;
pub use strands_macros::{tool, tools};
pub use tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolSource};
pub use types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeArgs};
pub use types::json::JsonValue;
pub use types::messages::{ContentBlock, Message, StopReason};
