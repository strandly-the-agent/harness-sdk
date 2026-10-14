//! Plugins (`strands-ts/src/plugins/plugin.ts`): extensions that register hooks and tools on an
//! agent at initialization.

mod model_plugin;
mod registry;
pub mod vended;

use async_trait::async_trait;

use crate::agent::Agent;
use crate::error::Result;
use crate::tools::ToolRef;

pub use model_plugin::ModelPlugin;
pub use registry::PluginRegistry;
pub use vended::{AgentSkills, ContextInjector, Skill, SkillSource};

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

/// Shared plugins are plugins too, so `Arc<P>` can be passed wherever `impl Plugin` is expected.
#[async_trait]
impl<P: Plugin + ?Sized> Plugin for std::sync::Arc<P> {
    fn name(&self) -> &str {
        (**self).name()
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        (**self).init_agent(agent).await
    }

    fn tools(&self) -> Vec<ToolRef> {
        (**self).tools()
    }
}

#[cfg(test)]
pub(crate) mod test_plugins {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::Plugin;
    use crate::agent::Agent;
    use crate::error::Result;
    use crate::hooks::BeforeInvocationEvent;
    use crate::tools::{Tool, ToolContext, ToolRef};
    use crate::types::{ToolResultBlock, ToolSpec};

    /// Records that it was initialized and registers one `BeforeInvocationEvent` hook.
    pub(crate) struct TestPlugin {
        name: String,
        initialized: Arc<AtomicBool>,
        hook_fired: Arc<AtomicBool>,
        tools: Vec<ToolRef>,
    }

    impl TestPlugin {
        pub(crate) fn new(name: &str) -> Self {
            Self {
                name: name.to_owned(),
                initialized: Arc::new(AtomicBool::new(false)),
                hook_fired: Arc::new(AtomicBool::new(false)),
                tools: Vec::new(),
            }
        }

        pub(crate) fn with_tools(mut self, tools: Vec<ToolRef>) -> Self {
            self.tools = tools;
            self
        }

        pub(crate) fn initialized(&self) -> bool {
            self.initialized.load(Ordering::SeqCst)
        }

        pub(crate) fn hook_fired(&self) -> bool {
            self.hook_fired.load(Ordering::SeqCst)
        }

        pub(crate) fn reset_initialized(&self) {
            self.initialized.store(false, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Plugin for TestPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            self.initialized.store(true, Ordering::SeqCst);
            let fired = Arc::clone(&self.hook_fired);
            agent
                .hooks()
                .add_sync_callback::<BeforeInvocationEvent, _>(move |_| {
                    fired.store(true, Ordering::SeqCst);
                    Ok(())
                });
            Ok(())
        }

        fn tools(&self) -> Vec<ToolRef> {
            self.tools.clone()
        }
    }

    struct NoopTool(ToolSpec);

    #[async_trait]
    impl Tool for NoopTool {
        fn name(&self) -> &str {
            &self.0.name
        }
        fn description(&self) -> &str {
            &self.0.description
        }
        fn spec(&self) -> &ToolSpec {
            &self.0
        }
        async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
            Ok(ToolResultBlock::success(&ctx.tool_use.tool_use_id, vec![]))
        }
    }

    pub(crate) fn test_tool(name: &str) -> ToolRef {
        Arc::new(NoopTool(ToolSpec::new(name, "a test tool")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::test_plugins::{test_tool, TestPlugin};
    use super::Plugin;
    use crate::agent::test_agent_for_plugins;
    use crate::hooks::BeforeInvocationEvent;

    #[test]
    fn returns_the_plugin_name() {
        assert_eq!(TestPlugin::new("test-plugin").name(), "test-plugin");
    }

    #[test]
    fn supports_custom_names() {
        assert_eq!(TestPlugin::new("my-custom-plugin").name(), "my-custom-plugin");
    }

    #[tokio::test]
    async fn registers_callbacks_via_agent_hooks() {
        let agent = test_agent_for_plugins();
        TestPlugin::new("p").init_agent(&agent).await.unwrap();
        assert_eq!(agent.hooks().count::<BeforeInvocationEvent>(), 1);
    }

    #[tokio::test]
    async fn can_be_implemented_for_custom_initialization() {
        let agent = test_agent_for_plugins();
        let plugin = TestPlugin::new("p");
        assert!(!plugin.initialized());
        plugin.init_agent(&agent).await.unwrap();
        assert!(plugin.initialized());
    }

    #[test]
    fn tools_default_to_none() {
        assert!(TestPlugin::new("p").tools().is_empty());
    }

    #[test]
    fn tools_can_be_implemented_to_provide_tools() {
        let tool = test_tool("plugin-tool");
        let plugin = TestPlugin::new("p").with_tools(vec![Arc::clone(&tool)]);
        assert_eq!(plugin.tools().len(), 1);
        assert_eq!(plugin.tools()[0].name(), tool.name());
    }
}
