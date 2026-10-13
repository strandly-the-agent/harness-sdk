//! Plugins (`strands-ts/src/plugins/plugin.ts`): extensions that register hooks and tools on an
//! agent at initialization.

use async_trait::async_trait;

use crate::agent::Agent;
use crate::error::Result;
use crate::tools::ToolRef;

/// An agent extension. `init_agent` runs once during `Agent::initialize`; `tools` runs after it.
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Unique plugin name; duplicates are rejected.
    fn name(&self) -> &str;
    /// Register hooks / configure the agent.
    async fn init_agent(&self, agent: &Agent) -> Result<()>;
    /// Tools to add to the agent after `init_agent`.
    fn tools(&self) -> Vec<ToolRef> {
        Vec::new()
    }
}
