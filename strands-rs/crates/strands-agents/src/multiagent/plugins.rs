//! Multi-agent plugins (`strands-ts/src/multiagent/plugins.ts`): extensions that register hooks
//! on an orchestrator at initialization, mirroring [`Plugin`](crate::plugins::Plugin).

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use parking_lot::Mutex;

use super::events::OrchestratorRef;
use crate::error::{Error, Result};

/// An orchestrator extension. `init_multi_agent` runs once, when the orchestrator initializes.
#[async_trait]
pub trait MultiAgentPlugin: Send + Sync {
    /// Unique plugin name; duplicates are rejected.
    fn name(&self) -> &str;
    /// Register hooks / configure the orchestrator. Takes the shared [`OrchestratorRef`] so a
    /// plugin can retain a handle (hook callbacks outlive this call).
    async fn init_multi_agent(&self, orchestrator: &OrchestratorRef) -> Result<()>;
}

/// Shared plugins are plugins too, so `Arc<P>` works wherever `impl MultiAgentPlugin` is expected.
#[async_trait]
impl<P: MultiAgentPlugin + ?Sized> MultiAgentPlugin for Arc<P> {
    fn name(&self) -> &str {
        (**self).name()
    }

    async fn init_multi_agent(&self, orchestrator: &OrchestratorRef) -> Result<()> {
        (**self).init_multi_agent(orchestrator).await
    }
}

enum InitState {
    Pending,
    Done,
    Failed(String),
}

struct Inner {
    plugins: Mutex<IndexMap<String, Arc<dyn MultiAgentPlugin>>>,
    pending: Mutex<VecDeque<Arc<dyn MultiAgentPlugin>>>,
    init: tokio::sync::Mutex<InitState>,
}

/// Registry of the plugins attached to an orchestrator.
///
/// Holds pending plugins and initializes them on first use, rejecting duplicate names. Cheap to
/// clone; clones share state.
#[derive(Clone)]
pub struct MultiAgentPluginRegistry {
    inner: Arc<Inner>,
}

impl MultiAgentPluginRegistry {
    /// Registry with `plugins` pending initialization.
    pub fn new(plugins: Vec<Arc<dyn MultiAgentPlugin>>) -> Self {
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
    pub fn add(&self, plugin: Arc<dyn MultiAgentPlugin>) {
        self.inner.pending.lock().push_back(plugin);
    }

    /// The registered plugin with this name, if it has been initialized.
    pub fn get(&self, name: &str) -> Option<Arc<dyn MultiAgentPlugin>> {
        self.inner.plugins.lock().get(name).cloned()
    }

    /// Initialize all pending plugins with `orchestrator`, sequentially.
    ///
    /// Safe to call multiple times: later calls share the first call's outcome, including a
    /// failure — plugin initialization is fail-closed.
    pub async fn initialize(&self, orchestrator: &OrchestratorRef) -> Result<()> {
        let mut init = self.inner.init.lock().await;
        match &*init {
            InitState::Done => return Ok(()),
            InitState::Failed(message) => return Err(Error::Plugin(message.clone())),
            InitState::Pending => {}
        }

        match self.drain_pending(orchestrator).await {
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

    async fn drain_pending(&self, orchestrator: &OrchestratorRef) -> Result<()> {
        loop {
            // Pop in its own statement so the guard is dropped before awaiting.
            let next = self.inner.pending.lock().pop_front();
            let Some(plugin) = next else { break };
            self.add_and_init(plugin, orchestrator).await?;
        }
        Ok(())
    }

    async fn add_and_init(
        &self,
        plugin: Arc<dyn MultiAgentPlugin>,
        orchestrator: &OrchestratorRef,
    ) -> Result<()> {
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

        plugin.init_multi_agent(orchestrator).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::multiagent::state::{MultiAgentResult, ResultStatus};
    use crate::multiagent::test_orchestrator::TestOrchestrator;

    /// Plugin whose initialization always fails, counting its calls.
    struct FailingPlugin {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl MultiAgentPlugin for FailingPlugin {
        fn name(&self) -> &str {
            "failing-plugin"
        }

        async fn init_multi_agent(&self, _orchestrator: &OrchestratorRef) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(Error::Plugin("plugin initialization failed".to_owned()))
        }
    }

    #[tokio::test]
    async fn preserves_init_multi_agent_failures_across_initialization_attempts() {
        let plugin = Arc::new(FailingPlugin {
            calls: AtomicUsize::new(0),
        });
        let registry = MultiAgentPluginRegistry::new(vec![plugin.clone()]);
        let orchestrator = TestOrchestrator::shared(
            "inner",
            vec![],
            MultiAgentResult::new(vec![], 1).with_status(ResultStatus::Completed),
        );

        for _ in 0..2 {
            let error = registry
                .initialize(&orchestrator)
                .await
                .expect_err("initialization stays failed");
            assert!(
                error.to_string().contains("plugin initialization failed"),
                "{error}"
            );
        }

        assert_eq!(
            plugin.calls.load(Ordering::SeqCst),
            1,
            "the failure is not retried"
        );
    }

    #[tokio::test]
    async fn rejects_a_duplicate_plugin_name() {
        let plugin = Arc::new(NoopPlugin);
        let registry = MultiAgentPluginRegistry::new(vec![plugin.clone(), plugin.clone()]);
        let orchestrator = TestOrchestrator::shared("inner", vec![], MultiAgentResult::new(vec![], 1));

        let error = registry
            .initialize(&orchestrator)
            .await
            .expect_err("duplicates are rejected");

        assert!(error.to_string().contains("plugin already registered"), "{error}");
        assert!(registry.get("noop-plugin").is_some());
    }

    struct NoopPlugin;

    #[async_trait]
    impl MultiAgentPlugin for NoopPlugin {
        fn name(&self) -> &str {
            "noop-plugin"
        }

        async fn init_multi_agent(&self, _orchestrator: &OrchestratorRef) -> Result<()> {
            Ok(())
        }
    }
}
