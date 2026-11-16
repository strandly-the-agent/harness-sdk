//! Strands Agents SDK for Rust.
//!
//! Strands is a model-driven SDK for building AI agents: an agent is a model, a system prompt and a
//! list of tools, and the model's own reasoning decides which tools to call. This crate mirrors the
//! TypeScript SDK (`@strands-agents/sdk`) concept for concept, re-cased for Rust, and reads and
//! writes the same JSON for messages, sessions and snapshots.
//!
//! # Quick start
//!
//! An agent with one tool, invoked and then streamed. Without a `model`, an agent uses Amazon
//! Bedrock with the default AWS credential chain, so this example is compiled but not run:
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
//!         .printer(false) // this example prints the output itself
//!         .build()?;
//!
//!     let result = agent.invoke("What's the weather in Seattle?").await?;
//!     println!("{result}");
//!
//!     {
//!         let mut stream = agent.stream("And in Lisbon?");
//!         while let Some(event) = stream.next().await {
//!             if let AgentStreamEvent::ModelStreamUpdate(update) = event?
//!                 && let Some(text) = update.text_delta()
//!             {
//!                 print!("{text}");
//!             }
//!         }
//!         println!();
//!     }
//!     agent.shutdown().await
//! }
//! ```
//!
//! [`Agent::stream`] borrows the agent, so the stream is scoped (or dropped) before the agent is
//! used again. [`Agent::shutdown`] stands in for TS `await using`: it finishes the agent's pending
//! work, such as the hooks a dropped stream skipped.
//!
//! An array holds tools of one type. For a second tool, chain
//! [`.tool(GetWeather).tool(GetTime)`](agent::AgentBuilder::tool), or pass one list with
//! [`tool_list!`]: `.tools(tool_list![GetWeather, GetTime])`.
//!
//! An agent can also be built from an [`AgentConfig`] literal, which has a field for every
//! option:
//!
//! ```
//! use strands::testing::MockMessageModel;
//! use strands::types::messages::TextBlock;
//! use strands::{Agent, AgentConfig, StopReason};
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! // A scripted model, so this runs offline; any provider goes here.
//! let model = MockMessageModel::new();
//! model.add_turn(vec![TextBlock::new("Hello!").into()]);
//!
//! let mut agent = Agent::new(AgentConfig {
//!     model: Some(model.into()),
//!     system_prompt: Some("You are a concise assistant.".into()),
//!     printer: Some(false),
//!     ..Default::default()
//! })?;
//! let result = agent.invoke("Hi").await?;
//! assert_eq!(result.stop_reason(), StopReason::EndTurn);
//! assert_eq!(result.to_string(), "Hello!");
//! # Ok::<(), strands::Error>(())
//! # }).unwrap();
//! ```
//!
//! # Modules
//!
//! The modules follow the TypeScript SDK's source folders.
//!
//! | Module | Contents |
//! |---|---|
//! | [`agent`] | [`Agent`], its builder and [`AgentConfig`], [`AgentStream`], direct tool calls, snapshots, agents as tools |
//! | [`models`] | The [`Model`](models::Model) trait, stream events, model info, the providers `BedrockModel`, `AnthropicModel`, `OpenAIModel` (each behind its feature), and the model router in [`models::routing`] |
//! | [`tools`](mod@tools) | The [`Tool`] trait, [`ToolContext`], [`FunctionTool`](tools::FunctionTool), the [`ToolRegistry`](tools::ToolRegistry), executors and structured output; [`tool`](macro@tool) and [`tools`](macro@tools) are the macros |
//! | [`hooks`] | Typed callbacks on the agent's lifecycle events, with [`HookOrder`](hooks::HookOrder) and removal |
//! | [`plugins`] | The [`Plugin`](plugins::Plugin) trait: named extensions that register hooks and tools |
//! | [`middleware`] | Handlers around a stage of the agent loop, such as the model call or a tool call |
//! | [`interrupt`] | Pausing an agent for a human answer and resuming it |
//! | [`cancel`] | [`CancelSignal`], [`CancelHandle`] and [`CancelOptions`] |
//! | [`conversation_manager`] | Keeping the history inside the context window: sliding window, summarizing, or none |
//! | [`session`] | The [`SessionManager`](session::SessionManager): saving and restoring agents as snapshots |
//! | [`storage`] | The [`Storage`](storage::Storage) trait with in-memory and local file backends |
//! | [`memory`] | Long-term memory: the [`MemoryManager`](memory::MemoryManager), its tools and the [`MemoryStore`](memory::MemoryStore) trait |
//! | [`vended_memory_stores`] | Memory stores that ship with the SDK, such as the file-backed store |
//! | [`injection`] | Content injected into the model request on a user turn |
//! | [`sandbox`] | Where an agent's commands, code and file operations run |
//! | [`vended_tools`] | Tools that ship with the SDK: shell, file editing, HTTP, notebooks, subagents and more |
//! | [`vended_plugins`] | Plugins that ship with the SDK: context injection and agent skills |
//! | [`interventions`] | The [`InterventionHandler`](interventions::InterventionHandler) trait: handlers that proceed, deny, guide, confirm or transform at the agent's gates |
//! | [`vended_interventions`] | Interventions that ship with the SDK: human-in-the-loop approval and Cedar authorization |
//! | `mcp` | The Model Context Protocol client, `McpClient` (behind the `mcp` feature) |
//! | [`retry`] | Model retry strategies and backoffs |
//! | [`background_tasks`] | Tools that run as tasks while the agent continues |
//! | [`state_store`] | [`StateStore`], the JSON key-value state of an agent |
//! | [`telemetry`] | The metrics and traces every [`AgentResult`] carries, the `tracing` spans, and OpenTelemetry setup (behind the `otel` feature) |
//! | [`types`] | Messages, content blocks, media, citations, tool types, snapshots, and JSON helpers |
//! | [`error`] | [`Error`], [`Result`] and the error types behind each variant |
//! | [`experimental`] | APIs that may change in any release, such as the context manager |
//! | [`testing`] | Test doubles for code built on Strands, such as a scripted model |
//!
//! The most used types are re-exported from the crate root.
//!
//! Third-party types in public signatures come with the crate: [`JsonValue`] is
//! `serde_json::Value`, and [`indexmap`] is re-exported for the ordered maps
//! (`strands::indexmap::IndexMap`) that MCP server configs, subagent presets and trace attributes take.
//! Tool parameter types and structured output types implement `serde` traits and
//! `schemars::JsonSchema` from [`schemars`] 1, which is re-exported: depend on
//! `serde = { version = "1", features = ["derive"] }` and `schemars = "1"` to derive them, or
//! derive `strands::schemars::JsonSchema` with `#[schemars(crate = "strands::schemars")]`.
//!
//! # Cargo features
//!
//! | Feature | Default | Enables |
//! |---|---|---|
//! | `bedrock` | yes | The Amazon Bedrock provider (`models::BedrockModel`), which is also the default model of an agent built without one |
//! | `anthropic` | no | The Anthropic provider (`models::AnthropicModel`) |
//! | `openai` | no | The OpenAI provider (`models::OpenAIModel`) for the Responses and Chat Completions APIs; with `bedrock`, also Amazon Bedrock Mantle |
//! | `mcp` | no | The Model Context Protocol client (`mcp::McpClient`) over stdio, Streamable HTTP and HTTP+SSE |
//! | `otel` | no | OpenTelemetry export of the SDK's spans and metrics |
//!
//! Without `bedrock`, an agent needs an explicit `model`.
//!
//! # Coming from the TypeScript SDK
//!
//! | TypeScript | Rust |
//! |---|---|
//! | `new Agent({...})` | [`Agent::builder`] or [`Agent::new`] with an [`AgentConfig`] |
//! | `agent.invoke(args, options?)` | [`Agent::invoke`], [`Agent::invoke_with`] |
//! | `agent.stream(args, options?)` | [`Agent::stream`], [`Agent::stream_with`]; the items are `Result<AgentStreamEvent>` |
//! | `agent.cancel(options?)` | [`Agent::cancel_handle`] then [`CancelHandle::cancel`], or [`InvokeOptions::cancel_signal`] |
//! | `agent.addHook(Event, callback, { order })` | [`Agent::add_hook`], [`Agent::add_async_hook`], [`Agent::add_hook_impl`], and their `_with` forms for an order; each returns a [`HookCleanup`](hooks::HookCleanup) |
//! | `event.agent` (`LocalAgent`) | `event.agent()` and `event.agent_mut()` on sequential events, a [`LocalAgent`](types::agent::LocalAgent); `event.agent()` on per-tool events, a [`ToolAgent`](types::agent::ToolAgent) |
//! | `LifecycleObserver.observeAgent(agent)` | [`InterventionHandler::observe_agent`](interventions::InterventionHandler::observe_agent), a provided method of the handler trait |
//! | `RoutingCandidateOptions` | [`RoutingCandidate::new`](models::routing::RoutingCandidate::new) with `with_name`, `with_description` and `with_metadata` |
//! | `MemoryStoreConfig` | The `name`, `description`, `max_search_results`, `writable` and `extraction` methods of [`MemoryStore`](memory::MemoryStore) |
//! | `OffloadStrategyBuilder` | The `when(conditions)` method of each strategy [`Offload`](experimental::context_manager::Offload) returns |
//! | `tool({ name, inputSchema, callback })` | [`tool`](macro@tool) on a function, [`tools`](macro@tools) on an impl block, or [`FunctionTool`](tools::FunctionTool); [`tool_list!`] collects tools of different types for `.tools(..)` |
//! | `Tool.stream()` async generator | [`Tool::stream`], a stream that ends in a [`ToolOutcome`] |
//! | `context.interrupt(params)` | [`ToolContext::interrupt`], which returns the response on resume |
//! | `agent.tool.<name>.invoke(input)` | [`Agent::tool`] then `invoke(input)` |
//! | `AbortSignal` | [`CancelSignal`] |
//! | Zod schemas | `serde` 1 and [`schemars`] 1 types |
//! | `Record<string, JSONValue>` state | [`StateStore`], a JSON map with typed `get` and `set` |
//! | `Record<string, unknown>` invocation state | [`InvocationState`], a typed extensions map |
//! | Optional peer dependencies | Cargo features |
//! | `@strands-agents/sdk/experimental` | [`experimental`] |
//! | `await using` | [`Agent::shutdown`] |
//!
//! `strands::tools` is both a module and the [`tools`](macro@tools) attribute, so
//! `use strands::tools;` imports both. In a crate with its own `tools` module, write
//! `#[strands::tools]` instead of importing it.
//!
//! # Interrupts
//!
//! A tool (or a hook) pauses the agent with an interrupt; the result then stops with
//! [`StopReason::Interrupt`] and the caller resumes by invoking the agent with the answers:
//!
//! ```
//! use serde_json::json;
//! use strands::testing::MockMessageModel;
//! use strands::types::messages::{TextBlock, ToolUseBlock};
//! use strands::{Agent, InterruptResponse, StopReason, ToolContext, ToolError, tool};
//!
//! /// Delete a file.
//! #[tool]
//! async fn delete_file(
//!     #[tool(context)] ctx: &ToolContext,
//!     /// The path to delete.
//!     path: String,
//! ) -> Result<String, ToolError> {
//!     // The first run ends the tool here; the run after the resume gets the answer.
//!     let answer = ctx.interrupt("confirm".into())?;
//!     Ok(if answer == "yes" { format!("deleted {path}") } else { "kept".to_owned() })
//! }
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let model = MockMessageModel::new();
//! model.add_turn(vec![ToolUseBlock::new("delete_file", "tool-1", json!({"path": "a.txt"})).into()]);
//! model.add_turn(vec![TextBlock::new("Done.").into()]);
//! let mut agent = Agent::builder().model(model).tools([DeleteFile]).printer(false).build()?;
//!
//! let mut result = agent.invoke("Delete a.txt").await?;
//! while result.stop_reason() == StopReason::Interrupt {
//!     let answers: Vec<InterruptResponse> = result
//!         .interrupts()
//!         .iter()
//!         .map(|interrupt| InterruptResponse::new(interrupt.id(), json!("yes")))
//!         .collect();
//!     result = agent.invoke(answers).await?;
//! }
//! assert_eq!(result.to_string(), "Done.");
//! # Ok::<(), strands::Error>(())
//! # }).unwrap();
//! ```
//!
//! # Wire compatibility
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
pub mod injection;
pub mod interrupt;
pub mod interventions;
mod logging;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;
pub mod memory;
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
pub mod vended_interventions;
pub mod vended_memory_stores;
pub mod vended_plugins;
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

#[doc(inline)]
pub use agent::{Agent, AgentConfig, AgentStream};
pub use async_trait::async_trait;
#[doc(inline)]
pub use cancel::{CancelHandle, CancelOptions, CancelSignal};
#[doc(inline)]
pub use error::{Error, Result};
pub use indexmap;
pub use schemars;
#[doc(inline)]
pub use interrupt::InterruptResponse;
#[cfg(feature = "bedrock")]
#[doc(inline)]
pub use models::bedrock::{
    BedrockCacheConfig, BedrockCacheTtl, BedrockGuardrailConfig, BedrockGuardrailRedactionConfig,
    BedrockModel, BedrockModelConfig, BedrockModelOptions,
};
#[doc(inline)]
pub use state_store::StateStore;
#[doc(inline)]
pub use strands_macros::{tool, tools};
#[doc(inline)]
pub use tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolSource};
#[doc(inline)]
pub use types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeArgs, InvokeOptions};
#[doc(inline)]
pub use types::json::JsonValue;
#[doc(inline)]
pub use types::messages::{ContentBlock, Message, StopReason};
