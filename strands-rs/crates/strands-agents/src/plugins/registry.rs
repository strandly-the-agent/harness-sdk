//! Plugin registry (`strands-ts/src/plugins/registry.ts`).

use std::collections::VecDeque;
use std::sync::Arc;

use indexmap::IndexMap;
use parking_lot::Mutex;

use super::Plugin;
use crate::agent::Agent;
use crate::error::{Error, Result};

enum InitState {
    Pending,
    Done,
    Failed(String),
}

struct Inner {
    plugins: Mutex<IndexMap<String, Arc<dyn Plugin>>>,
    pending: Mutex<VecDeque<Arc<dyn Plugin>>>,
    init: tokio::sync::Mutex<InitState>,
}

/// Registry of the plugins attached to an agent.
///
/// Holds pending plugins and initializes them on first use, rejecting duplicate names and
/// registering each plugin's tools. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct PluginRegistry {
    inner: Arc<Inner>,
}

impl PluginRegistry {
    /// Registry with `plugins` pending initialization.
    pub fn new(plugins: Vec<Arc<dyn Plugin>>) -> Self {
        Self {
            inner: Arc::new(Inner {
                plugins: Mutex::new(IndexMap::new()),
                pending: Mutex::new(plugins.into()),
                init: tokio::sync::Mutex::new(InitState::Pending),
            }),
        }
    }

    /// Queue `plugin` for initialization. Plugins added during
    /// [`initialize`](Self::initialize) are picked up by the same pass.
    pub fn add(&self, plugin: Arc<dyn Plugin>) {
        self.inner.pending.lock().push_back(plugin);
    }

    /// The registered plugin with this name, if it has been initialized.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Plugin>> {
        self.inner.plugins.lock().get(name).cloned()
    }

    /// Initialize all pending plugins with `agent`, sequentially.
    ///
    /// Safe to call multiple times: later calls share the first call's outcome, including a
    /// failure — plugin initialization is fail-closed.
    pub async fn initialize(&self, agent: &Agent) -> Result<()> {
        let mut init = self.inner.init.lock().await;
        match &*init {
            InitState::Done => return Ok(()),
            InitState::Failed(message) => return Err(Error::Plugin(message.clone())),
            InitState::Pending => {}
        }

        match self.drain_pending(agent).await {
            Ok(()) => {
                *init = InitState::Done;
                Ok(())
            }
            Err(error) => {
                *init = InitState::Failed(error.to_string());
                Err(error)
            }
        }
    }

    async fn drain_pending(&self, agent: &Agent) -> Result<()> {
        while let Some(plugin) = self.inner.pending.lock().pop_front() {
            self.add_and_init(plugin, agent).await?;
        }
        Ok(())
    }

    async fn add_and_init(&self, plugin: Arc<dyn Plugin>, agent: &Agent) -> Result<()> {
        let name = plugin.name().to_owned();
        {
            let mut plugins = self.inner.plugins.lock();
            if plugins.contains_key(&name) {
                return Err(Error::Plugin(format!(
                    "plugin_name=<{name}> | plugin already registered"
                )));
            }
            plugins.insert(name, Arc::clone(&plugin));
        }

        plugin.init_agent(agent).await?;

        let tools = plugin.tools();
        if !tools.is_empty() {
            agent.with_tool_registry(|registry| -> Result<()> {
                for tool in tools {
                    registry.add(tool)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::super::test_plugins::{test_tool, TestPlugin};
    use super::*;
    use crate::agent::test_agent_for_plugins;
    use crate::hooks::BeforeInvocationEvent;

    fn registry(plugins: Vec<Arc<dyn Plugin>>) -> PluginRegistry {
        PluginRegistry::new(plugins)
    }

    #[tokio::test]
    async fn initializes_a_plugin_and_calls_init_agent() {
        let plugin = Arc::new(TestPlugin::new("initializable-plugin"));
        let registry = registry(vec![Arc::clone(&plugin) as Arc<dyn Plugin>]);

        registry.initialize(&test_agent_for_plugins()).await.unwrap();

        assert!(plugin.initialized());
    }

    #[tokio::test]
    async fn registers_hooks_via_agent_hooks() {
        let agent = test_agent_for_plugins();
        registry(vec![Arc::new(TestPlugin::new("test-plugin"))])
            .initialize(&agent)
            .await
            .unwrap();

        assert_eq!(agent.hooks().count::<BeforeInvocationEvent>(), 1);
    }

    #[tokio::test]
    async fn throws_error_when_plugins_have_duplicate_names() {
        let registry = registry(vec![
            Arc::new(TestPlugin::new("duplicate-name")),
            Arc::new(TestPlugin::new("duplicate-name")),
        ]);

        let err = registry.initialize(&test_agent_for_plugins()).await.unwrap_err();

        assert!(matches!(err, Error::Plugin(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "plugin_name=<duplicate-name> | plugin already registered"
        );
    }

    #[tokio::test]
    async fn initializes_multiple_plugins_with_different_names() {
        let agent = test_agent_for_plugins();
        registry(vec![
            Arc::new(TestPlugin::new("plugin-1")),
            Arc::new(TestPlugin::new("plugin-2")),
        ])
        .initialize(&agent)
        .await
        .unwrap();

        assert_eq!(agent.hooks().count::<BeforeInvocationEvent>(), 2);
    }

    #[tokio::test]
    async fn auto_registers_tools_from_plugin_tools() {
        let agent = test_agent_for_plugins();
        let plugin = TestPlugin::new("tool-provider").with_tools(vec![test_tool("mock-tool")]);
        registry(vec![Arc::new(plugin)]).initialize(&agent).await.unwrap();

        assert!(agent.with_tool_registry(|r| r.get("mock-tool")).is_some());
    }

    #[tokio::test]
    async fn calls_tools_after_init_agent_so_tools_can_depend_on_initialization_state() {
        struct DeferredToolPlugin {
            initialized: Mutex<bool>,
        }

        #[async_trait]
        impl Plugin for DeferredToolPlugin {
            fn name(&self) -> &str {
                "deferred-tool-plugin"
            }
            async fn init_agent(&self, _agent: &Agent) -> Result<()> {
                *self.initialized.lock() = true;
                Ok(())
            }
            fn tools(&self) -> Vec<crate::tools::ToolRef> {
                if *self.initialized.lock() {
                    vec![test_tool("deferred-tool")]
                } else {
                    Vec::new()
                }
            }
        }

        let agent = test_agent_for_plugins();
        registry(vec![Arc::new(DeferredToolPlugin {
            initialized: Mutex::new(false),
        })])
        .initialize(&agent)
        .await
        .unwrap();

        assert!(agent.with_tool_registry(|r| r.get("deferred-tool")).is_some());
    }

    #[tokio::test]
    async fn handles_async_init_agent() {
        struct AsyncPlugin {
            initialized: Mutex<bool>,
        }

        #[async_trait]
        impl Plugin for AsyncPlugin {
            fn name(&self) -> &str {
                "async-plugin"
            }
            async fn init_agent(&self, _agent: &Agent) -> Result<()> {
                tokio::task::yield_now().await;
                *self.initialized.lock() = true;
                Ok(())
            }
        }

        let plugin = Arc::new(AsyncPlugin {
            initialized: Mutex::new(false),
        });
        registry(vec![Arc::clone(&plugin) as Arc<dyn Plugin>])
            .initialize(&test_agent_for_plugins())
            .await
            .unwrap();

        assert!(*plugin.initialized.lock());
    }

    #[tokio::test]
    async fn preserves_init_agent_failures_across_initialization_attempts() {
        struct FailingPlugin {
            calls: Mutex<u32>,
        }

        #[async_trait]
        impl Plugin for FailingPlugin {
            fn name(&self) -> &str {
                "failing-plugin"
            }
            async fn init_agent(&self, _agent: &Agent) -> Result<()> {
                *self.calls.lock() += 1;
                Err(Error::other("plugin initialization failed"))
            }
        }

        let plugin = Arc::new(FailingPlugin { calls: Mutex::new(0) });
        let registry = registry(vec![Arc::clone(&plugin) as Arc<dyn Plugin>]);
        let agent = test_agent_for_plugins();

        let first = registry.initialize(&agent).await.unwrap_err();
        let second = registry.initialize(&agent).await.unwrap_err();

        assert_eq!(first.to_string(), "plugin initialization failed");
        assert_eq!(second.to_string(), "plugin initialization failed");
        assert_eq!(*plugin.calls.lock(), 1);
    }

    #[tokio::test]
    async fn is_idempotent_calling_initialize_twice_only_runs_plugins_once() {
        let plugin = Arc::new(TestPlugin::new("initializable-plugin"));
        let registry = registry(vec![Arc::clone(&plugin) as Arc<dyn Plugin>]);
        let agent = test_agent_for_plugins();

        registry.initialize(&agent).await.unwrap();
        plugin.reset_initialized();
        registry.initialize(&agent).await.unwrap();

        assert!(!plugin.initialized());
    }

    #[tokio::test]
    async fn hooks_are_invoked_when_callbacks_are_called() {
        let agent = test_agent_for_plugins();
        let plugin = Arc::new(TestPlugin::new("test-plugin"));
        registry(vec![Arc::clone(&plugin) as Arc<dyn Plugin>])
            .initialize(&agent)
            .await
            .unwrap();

        let mut event = BeforeInvocationEvent {
            agent: agent.clone(),
            invocation_state: crate::agent::InvocationState::new(),
            cancel: crate::hooks::Cancel::No,
        };
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert!(plugin.hook_fired());
    }

    #[tokio::test]
    async fn drains_plugins_added_during_initialization() {
        struct AddingPlugin {
            registry: PluginRegistry,
            added: Arc<TestPlugin>,
        }

        #[async_trait]
        impl Plugin for AddingPlugin {
            fn name(&self) -> &str {
                "adding-plugin"
            }
            async fn init_agent(&self, _agent: &Agent) -> Result<()> {
                self.registry.add(Arc::clone(&self.added) as Arc<dyn Plugin>);
                Ok(())
            }
        }

        let registry = PluginRegistry::new(Vec::new());
        let added = Arc::new(TestPlugin::new("added-during-init"));
        registry.add(Arc::new(AddingPlugin {
            registry: registry.clone(),
            added: Arc::clone(&added),
        }));

        registry.initialize(&test_agent_for_plugins()).await.unwrap();

        assert!(added.initialized());
        assert!(registry.get("added-during-init").is_some());
    }
}
