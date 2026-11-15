//! The plugin trait (TS `plugins/plugin.ts`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::tools::Tool;
use crate::types::agent::LocalAgent;

/// A named extension that sets an agent up when it initializes (TS `Plugin`).
///
/// [`init_agent`](Plugin::init_agent) runs once, from the agent's first `initialize`, and
/// registers hooks through [`LocalAgent::hooks`]; it receives the plugin as an `Arc`, so hooks
/// can hold the plugin itself. The tools from [`get_tools`](Plugin::get_tools), read after
/// `init_agent`, are added to the agent's registry. Names are unique per agent.
///
/// ```
/// use std::sync::Arc;
///
/// use strands::hooks::BeforeInvocationEvent;
/// use strands::plugins::Plugin;
/// use strands::testing::MockMessageModel;
/// use strands::types::agent::LocalAgent;
/// use strands::Agent;
///
/// struct Greeter;
///
/// #[strands::async_trait]
/// impl Plugin for Greeter {
///     fn name(&self) -> &str {
///         "greeter"
///     }
///
///     async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> strands::Result<()> {
///         agent.hooks().add_hook(|_event: &mut BeforeInvocationEvent<'_>| {});
///         Ok(())
///     }
/// }
///
/// let agent = Agent::builder().model(MockMessageModel::new()).plugin(Greeter).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// The plugin's name, unique per agent.
    fn name(&self) -> &str;

    /// Registers the plugin's hooks on `agent` (TS `initAgent`); does nothing by default.
    ///
    /// # Errors
    ///
    /// Whatever the plugin reports; the agent's initialization fails with it, and so does
    /// every later attempt.
    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let _ = agent;
        Ok(())
    }

    /// Tools to add to the agent, read once after [`init_agent`](Plugin::init_agent) (TS
    /// `getTools`); none by default.
    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }
}

impl fmt::Debug for dyn Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugin").field("name", &self.name()).finish()
    }
}

#[cfg(test)]
mod tests;
