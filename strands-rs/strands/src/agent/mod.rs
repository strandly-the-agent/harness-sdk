//! The agent (TS `src/agent/`).

#[expect(clippy::module_inception, reason = "mirrors TS `agent/agent.ts`")]
mod agent;
pub mod agent_metadata;
mod config;
pub mod invocation_context;
mod run;
mod stream;
mod tool_caller;

pub use agent::Agent;
pub use config::{AgentBuilder, AgentConfig, ModelSource};
pub use invocation_context::{InvocationContext, InvocationContextUsage};
pub use stream::AgentStream;
pub use tool_caller::{DirectToolCallOptions, ToolCallStream, ToolHandle};
