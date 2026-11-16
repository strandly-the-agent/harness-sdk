//! Strands Agents SDK for Rust.
//!
//! Strands is a model-driven SDK for building AI agents: an agent is a model, a system prompt and a
//! list of tools, and the model's own reasoning decides which tools to call. This crate mirrors the
//! TypeScript SDK (`@strands-agents/sdk`) concept for concept, re-cased for Rust.
//!
//! An agent with one tool, invoked and then streamed (it needs a configured default model, so this
//! example is only compiled):
//!
//! ```no_run
//! use futures::StreamExt;
//! use strands::{Agent, AgentStreamEvent, tool};
//!
//! /// Get the current weather for a city.
//! #[tool]
//! async fn get_weather(
//!     /// City name, for example "Seattle".
//!     city: String,
//! ) -> Result<String, strands::ToolError> {
//!     Ok(format!("Sunny in {city}"))
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), strands::Error> {
//!     let mut agent = Agent::builder() // default model: Bedrock
//!         .system_prompt("You are a concise assistant.")
//!         .tools([GetWeather])
//!         .build()?;
//!
//!     let result = agent.invoke("What's the weather in Seattle?").await?;
//!     println!("{result}");
//!
//!     {
//!         let mut stream = agent.stream("And in Lisbon?");
//!         while let Some(event) = stream.next().await {
//!             if let AgentStreamEvent::ModelStreamUpdate(update) = event? {
//!                 if let Some(text) = update.text_delta() {
//!                     print!("{text}");
//!                 }
//!             }
//!         }
//!     }
//!     agent.shutdown().await
//! }
//! ```
//!
//! Messages serialize to the same wire JSON as TS:
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
pub mod background_tasks;
pub mod cancel;
pub mod conversation_manager;
pub mod error;
pub mod experimental;
pub mod hooks;
pub mod interrupt;
mod logging;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;
pub mod middleware;
mod mime;
pub mod models;
pub mod plugins;
pub mod retry;
pub mod sandbox;
pub mod session;
pub mod state_store;
pub mod storage;
pub mod telemetry;
pub mod testing;
pub mod tools;
pub mod types;
pub mod vended_tools;

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

pub use agent::{Agent, AgentConfig, AgentStream};
pub use async_trait::async_trait;
pub use cancel::{CancelHandle, CancelOptions, CancelSignal};
pub use error::{Error, Result};
pub use interrupt::InterruptResponse;
#[cfg(feature = "bedrock")]
pub use models::bedrock::{
    BedrockCacheConfig, BedrockCacheTtl, BedrockGuardrailConfig, BedrockGuardrailRedactionConfig,
    BedrockModel, BedrockModelConfig, BedrockModelOptions,
};
pub use state_store::StateStore;
pub use strands_macros::{tool, tools};
pub use tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolSource};
pub use types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeArgs, InvokeOptions};
pub use types::json::JsonValue;
pub use types::messages::{ContentBlock, Message, StopReason};
