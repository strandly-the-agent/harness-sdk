//! Public API shapes a downstream crate relies on.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use strands::agent::{AgentBuilder, agent_builder};
use strands::experimental::context_manager::{
    ContextManagerConfig, ContextManagerConfigBuilder, OffloadConditions, OffloadConditionsBuilder,
    StashConfig, StashConfigBuilder, SummarizeConfig, SummarizeConfigBuilder, TruncateConfig,
    TruncateConfigBuilder, context_manager_config_builder, offload_conditions_builder, stash_config_builder,
    summarize_config_builder, truncate_config_builder,
};
use strands::hooks::{AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent, Hook};
use strands::session::{
    ListSnapshotIdsOptions, ListSnapshotIdsOptionsBuilder, SessionManagerConfig, SessionManagerConfigBuilder,
    list_snapshot_ids_options_builder, session_manager_config_builder,
};
use strands::storage::{
    InMemoryStorageConfig, InMemoryStorageConfigBuilder, LocalFileStorageConfig,
    LocalFileStorageConfigBuilder, in_memory_storage_config_builder, local_file_storage_config_builder,
};
use strands::telemetry::AttributeValue;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolSource};
use strands::types::agent::InvokeLimits;
use strands::types::json::JsonValue;
use strands::types::messages::TextBlock;
use strands::{Agent, ToolError};

fn ping() -> FunctionTool {
    FunctionTool::new(
        "ping",
        "Replies pong.",
        |_input: JsonValue, _context| async move { Ok::<_, ToolError>("pong") },
    )
}

fn model(replies: &[&str]) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for reply in replies {
        model.add_turn(vec![TextBlock::new(*reply).into()]);
    }
    model
}

fn with_ping<S: agent_builder::State>(builder: AgentBuilder<S>) -> AgentBuilder<S> {
    builder.tools([Arc::new(ping())])
}

#[tokio::test]
async fn a_helper_can_take_and_return_an_agent_builder() {
    let agent = with_ping(Agent::builder().model(model(&[])).printer(false))
        .build()
        .unwrap();
    assert!(agent.tool_registry().get("ping").is_some());
    let _ = ToolSource::from(Arc::new(ping()));
}

#[tokio::test]
async fn set_model_takes_an_arc_of_a_concrete_model() {
    let mut agent = Agent::builder().model(model(&[])).printer(false).build().unwrap();
    agent.set_model(model(&["second"]));
    let prompt = String::from("hi");
    let result = agent.invoke(&prompt).await.unwrap();
    assert_eq!(result.to_string(), "second");
    let _ = Agent::builder().model(&prompt);
}

#[tokio::test]
async fn sequential_hooks_read_metrics_and_traces_use_system_time() {
    let cycles = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&cycles);
    let mut agent = Agent::builder()
        .model(model(&["ok"]))
        .printer(false)
        .build()
        .unwrap();
    agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
        seen.lock().unwrap().push(event.agent().metrics().cycle_count());
    });
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen_attempts = Arc::clone(&attempts);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        let attempt: NonZeroU32 = event.attempt_count();
        seen_attempts.lock().unwrap().push(attempt.get());
    });

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(*cycles.lock().unwrap(), [1]);
    assert_eq!(*attempts.lock().unwrap(), [1]);
    let trace = &result.traces()[0];
    assert!(trace.start_time().duration_since(UNIX_EPOCH).is_ok());
    assert!(trace.end_time().unwrap() >= trace.start_time());
}

#[test]
fn invoke_limits_have_a_builder() {
    let limits = InvokeLimits::builder().maybe_turns(NonZeroU32::new(3)).build();
    assert_eq!(limits.turns, NonZeroU32::new(3));
}

struct Audit;

#[strands::async_trait]
impl<'x> Hook<BeforeInvocationEvent<'x>> for Audit {
    async fn handle(&self, event: &mut BeforeInvocationEvent<'x>) {
        let _ = event.agent().id();
    }
}

#[test]
fn hook_impls_need_only_the_strands_async_trait_re_export() {
    let agent = Agent::builder().model(model(&[])).printer(false).build().unwrap();
    agent.hooks().add_hook_impl::<BeforeInvocationEvent<'_>>(Audit);
}

#[cfg(feature = "anthropic")]
#[test]
fn the_anthropic_model_builder_feeds_an_agent_config() -> strands::Result<()> {
    use strands::AgentConfig;
    use strands::models::AnthropicModel;

    let agent = Agent::new(AgentConfig {
        model: Some(
            AnthropicModel::builder()
                .api_key("test-key")
                .model_id("claude-sonnet-5-5")
                .build()?
                .into(),
        ),
        system_prompt: Some("You are a concise assistant.".into()),
        ..Default::default()
    })?;
    assert_eq!(agent.model().model_id().as_deref(), Some("claude-sonnet-5-5"));
    Ok(())
}

#[cfg(feature = "openai")]
#[test]
fn openai_model_options_take_a_full_struct_literal_under_any_feature_set() {
    use strands::models::openai::{OpenAIApi, OpenAIClientConfig, OpenAIModelConfig, OpenAIModelOptions};

    let options = OpenAIModelOptions {
        api: OpenAIApi::Chat,
        api_key: None,
        client_config: OpenAIClientConfig::default(),
        config: OpenAIModelConfig::default(),
    };
    assert_eq!(options.api, OpenAIApi::Chat);
}

#[cfg(feature = "bedrock")]
#[test]
fn bedrock_types_are_exported_from_the_crate_root() {
    let model = strands::BedrockModel::new(strands::BedrockModelConfig {
        model_id: Some("us.anthropic.claude-sonnet-4-6".into()),
        ..Default::default()
    });
    assert_eq!(
        strands::models::model::Model::model_id(&model).as_deref(),
        Some("us.anthropic.claude-sonnet-4-6")
    );
}

#[tokio::test]
async fn a_downstream_crate_configures_and_implements_conversation_managers() {
    use strands::conversation_manager::{
        ConversationManager, ConversationManagerReduceOptions, ProactiveCompression,
        SlidingWindowConversationManager, SlidingWindowConversationManagerConfig,
        SummarizingConversationManager, SummarizingConversationManagerConfig,
    };

    struct DropOldest;

    #[strands::async_trait]
    impl ConversationManager for DropOldest {
        fn name(&self) -> &str {
            "app:drop-oldest"
        }

        async fn reduce(&self, mut options: ConversationManagerReduceOptions<'_>) -> strands::Result<bool> {
            let messages = options.agent_mut().messages_mut();
            Ok(!messages.is_empty() && messages.drain(..1).count() == 1)
        }
    }

    let config = SlidingWindowConversationManagerConfig::builder()
        .window_size(10)
        .proactive_compression(true)
        .build();
    let sliding = SlidingWindowConversationManager::new(config).unwrap();
    let summarizing = SummarizingConversationManager::new(
        SummarizingConversationManagerConfig::builder()
            .proactive_compression(ProactiveCompression::Threshold(0.5))
            .build(),
    )
    .unwrap();
    assert_eq!(sliding.compression_threshold(), Some(0.7));
    assert_eq!(summarizing.compression_threshold(), Some(0.5));
    let literal = SlidingWindowConversationManagerConfig {
        window_size: 10,
        ..Default::default()
    };
    assert!(SlidingWindowConversationManager::new(literal).is_ok());
    let literal = SummarizingConversationManagerConfig {
        preserve_recent_messages: 4,
        ..Default::default()
    };
    assert!(SummarizingConversationManager::new(literal).is_ok());
    let mut agent = Agent::builder()
        .model(model(&["ok"]))
        .conversation_manager(DropOldest)
        .printer(false)
        .build()
        .unwrap();
    assert_eq!(agent.invoke("hi").await.unwrap().to_string(), "ok");
}

fn in_session<S: session_manager_config_builder::State>(
    builder: SessionManagerConfigBuilder<S>,
) -> SessionManagerConfigBuilder<S> {
    builder
}

#[test]
fn a_helper_can_name_the_storage_session_and_context_manager_config_builders() {
    let config = in_session(SessionManagerConfig::builder().session_id("s")).build();
    assert_eq!(config.session_id.as_deref(), Some("s"));
    let _: ListSnapshotIdsOptionsBuilder<list_snapshot_ids_options_builder::Empty> =
        ListSnapshotIdsOptions::builder();
    let _: InMemoryStorageConfigBuilder<in_memory_storage_config_builder::Empty> =
        InMemoryStorageConfig::builder();
    let _: LocalFileStorageConfigBuilder<local_file_storage_config_builder::Empty> =
        LocalFileStorageConfig::builder();
    let _: OffloadConditionsBuilder<offload_conditions_builder::Empty> = OffloadConditions::builder();
    let _: SummarizeConfigBuilder<summarize_config_builder::Empty> = SummarizeConfig::builder();
    let _: TruncateConfigBuilder<truncate_config_builder::Empty> = TruncateConfig::builder();
    let _: ContextManagerConfigBuilder<context_manager_config_builder::Empty> =
        ContextManagerConfig::builder();
    let _: StashConfigBuilder<stash_config_builder::Empty> = StashConfig::builder();
}

fn in_invoke<S: strands::types::agent::invoke_options_builder::State>(
    builder: strands::types::agent::InvokeOptionsBuilder<S>,
) -> strands::types::agent::InvokeOptionsBuilder<S> {
    builder
}

fn in_sleep<S: strands::vended_tools::sleep::make_sleep_options_builder::State>(
    builder: strands::vended_tools::sleep::MakeSleepOptionsBuilder<S>,
) -> strands::vended_tools::sleep::MakeSleepOptionsBuilder<S> {
    builder
}

fn in_request<S: strands::models::model_request_builder::State>(
    builder: strands::models::ModelRequestBuilder<S>,
) -> strands::models::ModelRequestBuilder<S> {
    builder
}

fn in_injector<S: strands::vended_plugins::context_injector::context_injector_config_builder::State>(
    builder: strands::vended_plugins::context_injector::ContextInjectorConfigBuilder<S>,
) -> strands::vended_plugins::context_injector::ContextInjectorConfigBuilder<S> {
    builder
}

#[test]
fn a_helper_can_name_any_config_builder() {
    let _ = in_invoke(strands::InvokeOptions::builder()).build();
    let _ = in_sleep(strands::vended_tools::MakeSleepOptions::builder()).build();
    let _ = in_request(strands::models::ModelRequest::builder()).build();
    let config = in_injector(strands::vended_plugins::ContextInjectorConfig::builder())
        .render_content(|_context| async { Ok(None) })
        .build();
    assert_eq!((config.name, config.trigger.is_none()), (None, true));
}

#[cfg(feature = "bedrock")]
#[test]
fn the_bedrock_model_has_a_builder_with_a_nameable_state() {
    use strands::models::bedrock::{
        BedrockModelBuilder, BedrockModelConfigBuilder, bedrock_model_builder, bedrock_model_config_builder,
    };

    fn in_model<S: bedrock_model_builder::State>(builder: BedrockModelBuilder<S>) -> BedrockModelBuilder<S> {
        builder
    }
    fn in_config<S: bedrock_model_config_builder::State>(
        builder: BedrockModelConfigBuilder<S>,
    ) -> BedrockModelConfigBuilder<S> {
        builder
    }

    let model = in_model(strands::BedrockModel::builder())
        .model_id("us.anthropic.claude-sonnet-4-6")
        .build();
    assert_eq!(
        strands::models::Model::model_id(&model).as_deref(),
        Some("us.anthropic.claude-sonnet-4-6")
    );
    let config = in_config(strands::BedrockModelConfig::builder()).build();
    assert_eq!(config.model_id, None);
}

#[cfg(feature = "openai")]
#[test]
fn the_openai_model_has_a_builder() -> strands::Result<()> {
    let model = strands::models::OpenAIModel::builder()
        .api_key("test-key")
        .model_id("gpt-5.4")
        .build()?;
    assert_eq!(
        strands::models::Model::model_id(&model).as_deref(),
        Some("gpt-5.4")
    );
    Ok(())
}

#[test]
fn single_value_builder_setters_take_a_value_an_arc_or_a_trait_object() -> strands::Result<()> {
    use strands::conversation_manager::{ConversationManager, NullConversationManager};
    use strands::plugins::Plugin;
    use strands::retry::DefaultModelRetryStrategy;
    use strands::vended_interventions::hitl::{HumanInTheLoop, HumanInTheLoopConfig};
    use strands::vended_plugins::{ContextInjector, ContextInjectorConfig};

    let injector = || ContextInjector::new(ContextInjectorConfig::new(|_context| async { Ok(None) }));
    let approval = Arc::new(HumanInTheLoop::new(HumanInTheLoopConfig::default()));
    let manager: Arc<dyn ConversationManager> = Arc::new(NullConversationManager::new());
    let shared_plugin: Arc<dyn Plugin> = Arc::new(injector());
    let agent = Agent::builder()
        .model(model(&[]))
        .printer(false)
        .plugin(injector())
        .plugin(shared_plugin)
        .intervention(Arc::clone(&approval))
        .conversation_manager(manager)
        .retry_strategy(Arc::new(DefaultModelRetryStrategy::default()))
        .build()?;
    drop(agent);
    let _ = Agent::builder()
        .model(model(&[]))
        .intervention(approval)
        .conversation_manager(Arc::new(NullConversationManager::new()))
        .build()?;
    Ok(())
}

#[test]
fn types_with_a_fixed_name_expose_it_as_a_constant() {
    use strands::conversation_manager::{
        ConversationManager, NullConversationManager, SlidingWindowConversationManager,
        SummarizingConversationManager,
    };
    use strands::experimental::context_manager::{DropStrategy, SummarizeStrategy, TruncateStrategy};

    assert_eq!(
        NullConversationManager::new().name(),
        NullConversationManager::NAME
    );
    assert_eq!(
        [
            SlidingWindowConversationManager::NAME,
            SummarizingConversationManager::NAME,
            DropStrategy::NAME,
            TruncateStrategy::NAME,
            SummarizeStrategy::NAME,
        ],
        [
            "strands:sliding-window-conversation-manager",
            "strands:summarizing-conversation-manager",
            "offload:drop",
            "offload:truncate",
            "offload:summarize",
        ]
    );
}

#[test]
fn a_model_impl_and_ordered_maps_need_only_strands_paths() {
    use strands::models::{BaseModelConfig, BoxStream, Model, ModelRequest, ModelStreamEvent};

    fn stream_of(model: &dyn Model) -> BoxStream<'static, strands::Result<ModelStreamEvent>> {
        model.stream(ModelRequest::default())
    }
    let _: fn(&dyn Model) -> BoxStream<'static, strands::Result<ModelStreamEvent>> = stream_of;
    let _ = BaseModelConfig::default();

    let mut baggage = strands::indexmap::IndexMap::new();
    baggage.insert("tenant".to_owned(), "a".to_owned());
    let config = strands::AgentConfig {
        baggage_attributes: baggage,
        ..Default::default()
    };
    assert_eq!(config.baggage_attributes.len(), 1);
}

#[test]
fn trace_and_baggage_attributes_take_plain_pairs() {
    let agent = Agent::builder()
        .model(model(&[]))
        .trace_attributes([("session.id".to_owned(), AttributeValue::from("abc"))])
        .baggage_attributes([("tenant".to_owned(), "a".to_owned())])
        .printer(false)
        .build();
    assert!(agent.is_ok());
}

#[cfg(feature = "otel")]
mod otel {
    use strands::telemetry::{
        Exporters, MeterConfig, MeterConfigBuilder, TracerConfig, TracerConfigBuilder, get_tracer,
        meter_config_builder, opentelemetry, opentelemetry_sdk, tracer_config_builder, tracing_layer,
        tracing_opentelemetry, tracing_subscriber,
    };

    fn traces_to_console<S: tracer_config_builder::State>(
        builder: TracerConfigBuilder<S>,
    ) -> TracerConfigBuilder<tracer_config_builder::SetExporters<S>>
    where
        S::Exporters: tracer_config_builder::IsUnset,
    {
        builder.exporters(Exporters::builder().console(true).build())
    }

    fn metrics_to_console<S: meter_config_builder::State>(
        builder: MeterConfigBuilder<S>,
    ) -> MeterConfigBuilder<meter_config_builder::SetExporters<S>>
    where
        S::Exporters: meter_config_builder::IsUnset,
    {
        builder.exporters(Exporters::builder().console(true).build())
    }

    #[test]
    fn a_helper_can_take_and_return_the_telemetry_config_builders() {
        let console = Exporters {
            otlp: false,
            console: true,
        };
        assert_eq!(
            traces_to_console(TracerConfig::builder()).build().exporters,
            console
        );
        assert_eq!(
            metrics_to_console(MeterConfig::builder()).build().exporters,
            console
        );
    }

    #[test]
    fn the_opentelemetry_crates_of_the_api_are_re_exported() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let layer: tracing_opentelemetry::OpenTelemetryLayer<_, opentelemetry_sdk::trace::SdkTracer> =
            tracing_layer(&provider);
        let _subscriber = tracing_subscriber::registry().with(layer);
        let _tracer: opentelemetry::global::BoxedTracer = get_tracer();
    }
}

#[test]
fn a_boxed_error_becomes_a_tool_error_and_reads_back_typed() {
    fn helper(text: &str) -> Result<u32, strands::error::BoxError> {
        Ok(text.parse::<u32>()?)
    }
    fn tool_body(text: &str) -> Result<u32, ToolError> {
        let number = helper(text).map_err(ToolError::from_boxed)?;
        Ok(number * 2)
    }

    assert_eq!(tool_body("7").ok(), Some(14));
    let error = tool_body("x").unwrap_err();
    assert!(error.downcast_ref::<std::num::ParseIntError>().is_some());
    assert!(error.downcast_ref::<std::fmt::Error>().is_none());
    assert_eq!(error.to_string(), "invalid digit found in string");
}

#[test]
fn a_type_built_from_one_config_takes_it_in_new_and_has_a_default() {
    use strands::interventions::InterventionHandler;
    use strands::storage::InMemoryStorage;
    use strands::vended_interventions::hitl::{HumanInTheLoop, HumanInTheLoopConfig};

    let _bare = InMemoryStorage::default();
    let _configured = InMemoryStorage::new(InMemoryStorageConfig::default());
    assert_eq!(HumanInTheLoop::default().name(), HumanInTheLoop::NAME);
    assert_eq!(
        HumanInTheLoop::new(HumanInTheLoopConfig::default()).name(),
        HumanInTheLoop::NAME
    );
}

#[tokio::test]
async fn a_tool_runs_on_its_own_with_a_testing_tool_context() {
    use futures::StreamExt;
    use strands::sandbox::NotASandboxLocalEnvironment;
    use strands::testing::{tool_context, tool_context_with_sandbox};
    use strands::tools::ToolStreamItem;
    use strands::types::tools::ToolUse;
    use strands::{Tool, ToolOutcome};

    let tool = ping();
    let tool_use = ToolUse::new("ping", "tool-1", serde_json::json!({}));
    let value = tool
        .invoke(serde_json::json!({}), tool_context(tool_use.clone()))
        .await
        .unwrap();
    assert_eq!(value, serde_json::json!("pong"));

    let context = tool_context_with_sandbox(tool_use, Arc::new(NotASandboxLocalEnvironment::new()));
    assert_eq!(context.tool_use().name(), "ping");
    let items: Vec<ToolStreamItem> = tool.stream(context).collect().await;
    assert!(matches!(
        items.last(),
        Some(ToolStreamItem::Outcome(ToolOutcome::Result(_)))
    ));
}

#[test]
fn the_builder_takes_tools_of_different_types_one_at_a_time() -> strands::Result<()> {
    /// Get the time.
    #[strands::tool]
    fn get_time() -> String {
        "noon".to_owned()
    }

    let agent = Agent::builder()
        .model(model(&[]))
        .tool(ping())
        .tool(GetTime)
        .build()?;
    assert_eq!(agent.tool_registry().len(), 2);
    Ok(())
}
