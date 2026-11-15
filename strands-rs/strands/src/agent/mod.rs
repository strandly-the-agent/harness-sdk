//! The agent (TS `src/agent/`).

#[expect(clippy::module_inception, reason = "mirrors TS `agent/agent.ts`")]
mod agent;
pub mod agent_metadata;
mod config;
mod run;
mod stream;

pub use agent::Agent;
pub use config::{AgentBuilder, AgentConfig, ModelSource};
pub use stream::AgentStream;
