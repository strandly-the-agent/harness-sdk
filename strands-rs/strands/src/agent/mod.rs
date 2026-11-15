//! The agent (TS `src/agent/`).

#[expect(clippy::module_inception, reason = "mirrors TS `agent/agent.ts`")]
mod agent;
mod agent_as_tool;
mod agent_delegation;
pub mod agent_metadata;
mod config;
pub mod invocation_context;
pub(crate) mod printer;
mod run;
mod stream;
mod tool_caller;

pub use agent::Agent;
pub use agent_as_tool::{AgentAsTool, AgentAsToolOptions};
pub use config::{AgentBuilder, AgentConfig, ModelSource};
pub use invocation_context::{InvocationContext, InvocationContextUsage};
pub use stream::AgentStream;
pub use tool_caller::{DirectToolCallOptions, ToolCallStream, ToolHandle};
