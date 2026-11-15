//! Ported from strands-ts/src/plugins/__tests__/plugin.test.ts and
//! strands-ts/src/plugins/__tests__/registry.test.ts.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::agent::Agent;
use crate::error::Error;
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent, BeforeModelCallEvent, InitializedEvent,
    MessageAddedEvent,
};
use crate::models::model::{BaseModelConfig, Model};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::plugins::PluginRegistry;
use crate::test_support::{mock_tool, text_result};
use crate::testing::MockMessageModel;
use crate::types::messages::TextBlock;

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(mock_tool(name, |context| {
        text_result(context.tool_use().tool_use_id(), "ok")
    }))
}

/// Registers a `BeforeInvocationEvent` hook that sets `hook_ran`.
struct TestPlugin {
    name: String,
    hook_ran: Arc<AtomicBool>,
}

impl TestPlugin {
    fn named(name: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            hook_ran: Arc::default(),
        })
    }
}

#[async_trait]
impl Plugin for TestPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let plugin = Arc::clone(&self);
        agent
            .hooks()
            .add_hook(move |_event: &mut BeforeInvocationEvent<'_>| {
                plugin.hook_ran.store(true, Ordering::SeqCst);
            });
        Ok(())
    }
}

/// Only the default `init_agent` and `get_tools`.
struct CustomNamePlugin(&'static str);

impl Plugin for CustomNamePlugin {
    fn name(&self) -> &str {
        self.0
    }
}

#[derive(Default)]
struct InitializablePlugin {
    initialized: AtomicBool,
    calls: AtomicUsize,
}

#[async_trait]
impl Plugin for InitializablePlugin {
    fn name(&self) -> &str {
        "initializable-plugin"
    }

    async fn init_agent(self: Arc<Self>, _agent: &mut LocalAgent) -> Result<()> {
        tokio::task::yield_now().await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }
}

struct ToolProviderPlugin {
    tools: Vec<Arc<dyn Tool>>,
    /// Tools are offered only once `init_agent` has run.
    initialized: AtomicBool,
}

#[async_trait]
impl Plugin for ToolProviderPlugin {
    fn name(&self) -> &str {
        "tool-provider"
    }

    async fn init_agent(self: Arc<Self>, _agent: &mut LocalAgent) -> Result<()> {
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        if self.initialized.load(Ordering::SeqCst) {
            self.tools.clone()
        } else {
            Vec::new()
        }
    }
}

struct FailingPlugin {
    calls: AtomicUsize,
}

#[async_trait]
impl Plugin for FailingPlugin {
    fn name(&self) -> &str {
        "failing-plugin"
    }

    async fn init_agent(self: Arc<Self>, _agent: &mut LocalAgent) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::other("plugin initialization failed"))
    }
}

async fn initialize(plugins: Vec<Arc<dyn Plugin>>, agent: &mut LocalAgent) -> Result<()> {
    PluginRegistry::default().initialize(plugins, agent).await
}

mod plugin {
    use super::*;

    #[test]
    fn returns_the_plugin_name() {
        assert_eq!(TestPlugin::named("test-plugin").name(), "test-plugin");
    }

    #[test]
    fn supports_custom_names_via_constructor() {
        assert_eq!(CustomNamePlugin("my-custom-plugin").name(), "my-custom-plugin");
    }

    #[tokio::test]
    async fn registers_callbacks_via_agent_add_hook() {
        let mut agent = local_agent();
        TestPlugin::named("test-plugin")
            .init_agent(&mut agent)
            .await
            .unwrap();
        assert!(agent.hooks().has_callbacks::<BeforeInvocationEvent<'static>>());
    }

    #[tokio::test]
    async fn has_a_no_op_default_when_not_overridden() {
        let mut agent = local_agent();
        Arc::new(CustomNamePlugin("test"))
            .init_agent(&mut agent)
            .await
            .unwrap();
        assert!(!agent.hooks().has_callbacks::<BeforeInvocationEvent<'static>>());
        assert_eq!(agent.tool_registry().list().count(), 0);
    }

    #[tokio::test]
    async fn can_be_implemented_for_custom_initialization() {
        let plugin = Arc::new(InitializablePlugin::default());
        assert!(!plugin.initialized.load(Ordering::SeqCst));
        Arc::clone(&plugin).init_agent(&mut local_agent()).await.unwrap();
        assert!(plugin.initialized.load(Ordering::SeqCst));
    }

    #[test]
    fn is_optional_plugins_without_get_tools_are_valid() {
        assert!(TestPlugin::named("test-plugin").get_tools().is_empty());
    }

    #[test]
    fn can_be_implemented_to_provide_tools() {
        let mock = tool("mock-tool");
        let plugin = ToolProviderPlugin {
            tools: vec![Arc::clone(&mock)],
            initialized: AtomicBool::new(true),
        };
        let tools = plugin.get_tools();
        assert_eq!(tools.len(), 1);
        assert!(Arc::ptr_eq(&tools[0], &mock));
    }
}

mod registry {
    use super::*;

    #[tokio::test]
    async fn initializes_a_plugin_and_calls_init_agent() {
        let plugin = Arc::new(InitializablePlugin::default());
        initialize(vec![plugin.clone()], &mut local_agent())
            .await
            .unwrap();
        assert!(plugin.initialized.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn registers_hooks_via_agent_add_hook() {
        let mut agent = local_agent();
        initialize(vec![TestPlugin::named("test-plugin")], &mut agent)
            .await
            .unwrap();
        assert!(agent.hooks().has_callbacks::<BeforeInvocationEvent<'static>>());
    }

    #[tokio::test]
    async fn throws_error_when_plugins_have_duplicate_names() {
        let plugins: Vec<Arc<dyn Plugin>> = vec![
            TestPlugin::named("duplicate-name"),
            TestPlugin::named("duplicate-name"),
        ];
        let error = initialize(plugins, &mut local_agent()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin_name=<duplicate-name> | plugin already registered"
        );
    }

    #[tokio::test]
    async fn initializes_multiple_plugins_with_different_names() {
        let (first, second) = (TestPlugin::named("plugin-1"), TestPlugin::named("plugin-2"));
        let mut agent = local_agent();
        initialize(vec![first.clone(), second.clone()], &mut agent)
            .await
            .unwrap();
        let mut event = BeforeInvocationEvent::new(
            &mut agent,
            crate::types::agent::InvocationState::new(),
            Vec::new(),
        );
        agent_hooks(&event)
            .invoke::<BeforeInvocationEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(first.hook_ran.load(Ordering::SeqCst));
        assert!(second.hook_ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn auto_registers_tools_from_plugin_get_tools() {
        let mock = tool("mock-tool");
        let plugin = Arc::new(ToolProviderPlugin {
            tools: vec![Arc::clone(&mock)],
            initialized: AtomicBool::new(true),
        });
        let mut agent = local_agent();
        initialize(vec![plugin], &mut agent).await.unwrap();
        assert!(Arc::ptr_eq(
            agent.tool_registry().get("mock-tool").unwrap(),
            &mock
        ));
    }

    #[tokio::test]
    async fn calls_get_tools_after_init_agent_so_tools_can_depend_on_initialization_state() {
        let mock = tool("deferred-tool");
        let plugin = Arc::new(ToolProviderPlugin {
            tools: vec![Arc::clone(&mock)],
            initialized: AtomicBool::new(false),
        });
        let mut agent = local_agent();
        initialize(vec![plugin], &mut agent).await.unwrap();
        assert!(Arc::ptr_eq(
            agent.tool_registry().get("deferred-tool").unwrap(),
            &mock
        ));
    }

    #[tokio::test]
    async fn handles_async_init_agent() {
        let plugin = Arc::new(InitializablePlugin::default());
        initialize(vec![plugin.clone()], &mut local_agent())
            .await
            .unwrap();
        assert!(plugin.initialized.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn preserves_init_agent_failures_across_initialization_attempts() {
        let plugin = Arc::new(FailingPlugin {
            calls: AtomicUsize::new(0),
        });
        let mut registry = PluginRegistry::default();
        let mut agent = local_agent();
        for _ in 0..2 {
            let error = registry
                .initialize(vec![plugin.clone()], &mut agent)
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "plugin initialization failed");
        }
        assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn is_idempotent_calling_initialize_twice_only_runs_plugins_once() {
        let plugin = Arc::new(InitializablePlugin::default());
        let mut registry = PluginRegistry::default();
        let mut agent = local_agent();
        registry
            .initialize(vec![plugin.clone()], &mut agent)
            .await
            .unwrap();
        registry
            .initialize(vec![plugin.clone()], &mut agent)
            .await
            .unwrap();
        assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hooks_are_invoked_when_callbacks_are_called() {
        let plugin = TestPlugin::named("test-plugin");
        let mut agent = local_agent();
        initialize(vec![plugin.clone()], &mut agent).await.unwrap();
        let mut event = BeforeInvocationEvent::new(
            &mut agent,
            crate::types::agent::InvocationState::new(),
            Vec::new(),
        );
        agent_hooks(&event)
            .invoke::<BeforeInvocationEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(plugin.hook_ran.load(Ordering::SeqCst));
    }

    fn agent_hooks(event: &BeforeInvocationEvent<'_>) -> crate::hooks::HookRegistry {
        event.agent().hooks().clone()
    }
}

/// The plugin cases of strands-ts/src/agent/__tests__/agent.hook.test.ts and the agent's
/// registration order.
mod agent {
    use super::*;

    struct LifecycleBridge {
        queue: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl Plugin for LifecycleBridge {
        fn name(&self) -> &str {
            "strands:lifecycle-bridge"
        }

        async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
            let hooks = agent.hooks();
            let push = |label: &'static str| {
                let queue = Arc::clone(&self.queue);
                move || queue.lock().unwrap().push(label)
            };
            let initialized = push("initialized");
            hooks.add_hook(move |_: &mut InitializedEvent<'_>| initialized());
            let before_invocation = push("before-invocation");
            hooks.add_hook(move |_: &mut BeforeInvocationEvent<'_>| before_invocation());
            let after_invocation = push("after-invocation");
            hooks.add_hook(move |_: &mut AfterInvocationEvent<'_>| after_invocation());
            let before_model = push("before-model-call");
            hooks.add_hook(move |_: &mut BeforeModelCallEvent<'_>| before_model());
            let after_model = push("after-model-call");
            hooks.add_hook(move |_: &mut AfterModelCallEvent<'_>| after_model());
            let message_added = push("message-added");
            hooks.add_hook(move |_: &mut MessageAddedEvent<'_>| message_added());
            Ok(())
        }
    }

    fn hello_model() -> MockMessageModel {
        let model = MockMessageModel::new();
        model.add_turn(vec![TextBlock::new("Hello").into()]);
        model
    }

    #[tokio::test]
    async fn receives_lifecycle_events_when_registered_via_plugins_config() {
        let queue = Arc::new(Mutex::new(Vec::new()));
        let mut agent = Agent::builder()
            .model(hello_model())
            .plugin(LifecycleBridge {
                queue: Arc::clone(&queue),
            })
            .build()
            .unwrap();
        agent.invoke("Hi").await.unwrap();
        assert_eq!(
            *queue.lock().unwrap(),
            [
                "initialized",
                "before-invocation",
                "message-added",
                "before-model-call",
                "after-model-call",
                "message-added",
                "after-invocation",
            ]
        );
    }

    #[tokio::test]
    async fn keeps_a_plugin_initialization_failure_fail_closed_across_retries() {
        let plugin = Arc::new(FailingPlugin {
            calls: AtomicUsize::new(0),
        });
        let mut agent = Agent::builder()
            .model(MockMessageModel::new())
            .plugins([plugin.clone() as Arc<dyn Plugin>])
            .build()
            .unwrap();
        for _ in 0..2 {
            let error = agent.initialize().await.unwrap_err();
            assert_eq!(error.to_string(), "plugin initialization failed");
        }
        assert!(agent.invoke("Hi").await.is_err());
        assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejects_a_user_plugin_named_like_a_retry_strategy() {
        let mut agent = Agent::builder()
            .model(MockMessageModel::new())
            .plugin(CustomNamePlugin("strands:default-model-retry-strategy"))
            .build()
            .unwrap();
        let error = agent.initialize().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin_name=<strands:default-model-retry-strategy> | plugin already registered"
        );
    }

    #[tokio::test]
    async fn a_user_plugin_named_strands_agent_delegation_replaces_the_built_in() {
        let mut agent = Agent::builder()
            .model(MockMessageModel::new())
            .plugin(CustomNamePlugin("strands:agent-delegation"))
            .build()
            .unwrap();
        agent.initialize().await.unwrap();
        assert!(
            !agent
                .hooks()
                .has_callbacks::<crate::hooks::BeforeToolsEvent<'static>>()
        );

        let mut default = Agent::builder().model(MockMessageModel::new()).build().unwrap();
        default.initialize().await.unwrap();
        assert!(
            default
                .hooks()
                .has_callbacks::<crate::hooks::BeforeToolsEvent<'static>>()
        );
    }

    #[tokio::test]
    async fn rejects_a_user_plugin_named_strands_model() {
        let mut agent = Agent::builder()
            .model(MockMessageModel::new())
            .plugin(CustomNamePlugin("strands:model"))
            .build()
            .unwrap();
        let error = agent.initialize().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin_name=<strands:model> | plugin already registered"
        );
    }

    /// Records the order its `init_agent` ran in.
    struct Recorder {
        name: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl Plugin for Recorder {
        fn name(&self) -> &str {
            self.name
        }

        async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
            let retry_installed = agent.hooks().has_callbacks::<AfterModelCallEvent<'static>>();
            let delegation_installed = agent
                .hooks()
                .has_callbacks::<crate::hooks::BeforeToolsEvent<'static>>();
            assert!(retry_installed, "retry strategies initialize before user plugins");
            assert!(!delegation_installed, "delegation initializes after user plugins");
            self.order.lock().unwrap().push(self.name);
            Ok(())
        }
    }

    #[tokio::test]
    async fn initializes_retry_strategies_then_user_plugins_in_order_then_the_built_ins() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut agent = Agent::builder()
            .model(MockMessageModel::new())
            .plugin(Recorder {
                name: "first",
                order: Arc::clone(&order),
            })
            .plugin(Recorder {
                name: "second",
                order: Arc::clone(&order),
            })
            .build()
            .unwrap();
        agent.initialize().await.unwrap();
        assert_eq!(*order.lock().unwrap(), ["first", "second"]);
        assert!(
            agent
                .hooks()
                .has_callbacks::<crate::hooks::BeforeToolsEvent<'static>>()
        );
        assert!(agent.hooks().has_callbacks::<AfterInvocationEvent<'static>>());
    }

    fn stateful(model: MockMessageModel) -> MockMessageModel {
        model.update_config(BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(ModelCapabilities {
                    stateful: Some(true),
                    ..ModelCapabilities::default()
                }),
                ..ModelInfo::default()
            }),
            ..BaseModelConfig::default()
        });
        model
    }

    #[tokio::test]
    async fn the_model_plugin_clears_the_history_of_a_stateful_model_before_other_after_invocation_hooks() {
        let seen = Arc::new(Mutex::new(None));
        let record = Arc::clone(&seen);
        let mut agent = Agent::builder().model(stateful(hello_model())).build().unwrap();
        agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
            *record.lock().unwrap() = Some(event.agent().messages().len());
        });
        agent.invoke("Hi").await.unwrap();
        assert!(agent.messages().is_empty());
        assert_eq!(*seen.lock().unwrap(), Some(0));

        let mut stateless = Agent::builder().model(hello_model()).build().unwrap();
        stateless.invoke("Hi").await.unwrap();
        assert_eq!(stateless.messages().len(), 2);
        assert_eq!(
            json!(stateless.messages()[1].content()),
            json!([{"text": "Hello"}])
        );
    }
}
