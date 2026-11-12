//! The agent (TS `agent/`).

#[expect(clippy::module_inception, reason = "mirrors the TS `agent/agent.ts` stem")]
mod agent;
mod agent_as_tool;
mod agent_metadata;
mod invocation_context;
mod invocation_state;
mod invoke;
pub(crate) mod printer;
mod result;
mod run;
pub(crate) mod stream;
mod tool_caller;
mod views;

pub use agent::{Agent, AgentBuilder, AgentConfig, ModelSource};
pub use agent_as_tool::{AgentAsTool, AgentAsToolOptions, DELEGATION_DESCRIPTION_SUFFIX};
pub use agent_metadata::AgentMetadata;
pub use invocation_context::{InvocationContext, InvocationContextUsage, InvokeLimits};
pub use invocation_state::InvocationState;
pub use invoke::{InvokeArgs, InvokeOptions};
pub use result::AgentResult;
pub use stream::{AgentStream, AgentStreamEvent};
pub use tool_caller::{DirectToolCallOptions, ToolHandle};
pub use views::{LocalAgent, ToolAgent};

#[cfg(test)]
mod tests;
