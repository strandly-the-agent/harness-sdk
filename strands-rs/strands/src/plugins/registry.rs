//! The plugin registry (TS `plugins/registry.ts`).

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::plugins::Plugin;
use crate::types::agent::LocalAgent;

/// Initializes an agent's plugins once, in order, and keeps a failure (TS `PluginRegistry`).
#[derive(Debug, Default)]
pub(crate) struct PluginRegistry {
    names: Vec<String>,
    /// How many plugins finished; a dropped `initialize` resumes from here.
    next: usize,
    /// The first failure, returned again by every later `initialize` (TS keeps the rejected
    /// promise).
    failure: Option<String>,
}

impl PluginRegistry {
    /// Runs each plugin's `init_agent` and adds its tools; a later call does nothing, returns
    /// the first call's failure, or resumes a dropped call at the plugin it was awaiting.
    pub(crate) async fn initialize(
        &mut self,
        plugins: Vec<Arc<dyn Plugin>>,
        agent: &mut LocalAgent,
    ) -> Result<()> {
        if let Some(failure) = &self.failure {
            return Err(Error::other(failure.clone()));
        }
        for plugin in plugins.into_iter().skip(self.next) {
            if let Err(error) = self.add_and_init(plugin, agent).await {
                self.failure = Some(error.to_string());
                return Err(error);
            }
            self.next += 1;
        }
        Ok(())
    }

    async fn add_and_init(&mut self, plugin: Arc<dyn Plugin>, agent: &mut LocalAgent) -> Result<()> {
        let name = plugin.name().to_owned();
        if self.names.contains(&name) {
            return Err(Error::other(format!(
                "plugin_name=<{name}> | plugin already registered"
            )));
        }
        Arc::clone(&plugin).init_agent(agent).await?;
        self.names.push(name);
        let tools = plugin.get_tools();
        if !tools.is_empty() {
            agent.tool_registry_mut().add_all(tools)?;
        }
        Ok(())
    }
}
