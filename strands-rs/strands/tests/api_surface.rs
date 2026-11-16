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
