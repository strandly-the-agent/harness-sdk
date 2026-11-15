//! The agent (TS `agent/`).

#[expect(clippy::module_inception, reason = "mirrors TS agent/agent.ts")]
pub mod agent;
mod agent_as_tool;
mod agent_delegation;
pub mod agent_metadata;
mod config;
mod event_loop;
pub mod invocation_context;
pub mod invocation_state;
mod invoke;
pub mod local_agent;
mod printer;
pub mod snapshot;
mod stream;
pub mod stream_event;
pub mod tool_agent;
mod tool_caller;

pub use agent::{Agent, DEFAULT_AGENT_ID, DEFAULT_AGENT_NAME};
pub use agent_as_tool::{AgentAsTool, AgentAsToolOptions, DELEGATION_DESCRIPTION_SUFFIX};
pub use config::{AgentBuilder, AgentConfig, agent_builder};
pub use invocation_context::{InvocationContext, InvocationContextUsage, InvokeLimits};
pub use invoke::{InvokeArgs, InvokeOptions};
pub use stream::AgentStream;
pub use tool_caller::{DirectToolCallOptions, ToolHandle};
