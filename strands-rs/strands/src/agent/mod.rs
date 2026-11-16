//! The agent (TS `src/agent/`).

#[expect(clippy::module_inception, reason = "mirrors TS `agent/agent.ts`")]
mod agent;
pub(crate) mod agent_as_tool;
mod agent_delegation;
pub mod agent_metadata;
mod config;
pub(crate) mod continuation;
pub mod invocation_context;
pub(crate) mod printer;
mod run;
mod snapshot;
mod stream;
mod tool_caller;

pub use agent::Agent;
pub use agent_as_tool::{
    AgentAsTool, AgentAsToolOptions, AgentAsToolOptionsBuilder, agent_as_tool_options_builder,
};
pub use config::{AgentBuilder, AgentConfig, MemoryManagerSource, ModelSource, SharedSource, agent_builder};
pub use invocation_context::{InvocationContext, InvocationContextUsage};
pub(crate) use run::CancelledError;
pub use snapshot::{
    SnapshotField, SnapshotPreset, TakeSnapshotOptions, TakeSnapshotOptionsBuilder,
    take_snapshot_options_builder,
};
pub use stream::AgentStream;
pub use tool_caller::{
    DirectToolCallOptions, DirectToolCallOptionsBuilder, ToolCallStream, ToolHandle,
    direct_tool_call_options_builder,
};
