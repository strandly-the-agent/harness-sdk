//! Ported from strands-ts/src/context-manager/__tests__/context-manager.test.ts,
//! strands-ts/src/agent/__tests__/agent.context-manager.test.ts and the plugin cases of
//! strands-ts/src/context-manager/__tests__/stash-integration.test.ts

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::test_support::{agent_with, agent_with_model, assistant, tool_result, user};
use super::*;
use crate::conversation_manager::{ConversationManager, SlidingWindowConversationManager};
use crate::error::ContextWindowOverflowError;
use crate::models::model::{BaseModelConfig, Model};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::storage::InMemoryStorage;
use crate::testing::MockMessageModel;
use crate::types::agent::InvocationState;
use crate::types::messages::ContentBlock;
use crate::{Agent, AgentConfig};

#[derive(Default)]
struct Recorder {
    calls: AtomicUsize,
    init: AtomicBool,
    acted: bool,
    fails: bool,
    seen: Mutex<Vec<(f64, bool)>>,
}

#[async_trait]
impl ContextStrategy for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    fn init(self: Arc<Self>, _agent: &mut LocalAgent, _stash: Option<Stash>) {
        self.init.store(true, Ordering::Relaxed);
    }

    async fn apply(&self, context: &mut ContextState<'_>) -> Result<bool> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.seen
            .lock()
            .unwrap()
            .push((context.utilization, context.overflow));
        if self.fails {
            return Err(Error::other("strategy failed"));
        }
        Ok(self.acted)
    }
}

fn manager_with(strategy: Arc<Recorder>) -> Arc<ContextManager> {
    let config = ContextManagerConfig {
        strategies: Some(vec![ContextStrategyEntry::Strategy(strategy)]),
        stash: None,
    };
    Arc::new(ContextManager::new(config).unwrap())
}

fn conversation(pairs: usize) -> Vec<Message> {
    let mut messages = vec![user("system")];
    for index in 1..=pairs {
        messages.push(assistant(&format!("response {index}")));
        messages.push(user(&format!("msg {}", index + 1)));
    }
    messages
}

/// A model reporting a context window of `window` tokens.
fn model_with_window(window: u64) -> Arc<dyn Model> {
    let model = MockMessageModel::new();
    model.update_config(BaseModelConfig {
        context_window_limit: Some(window),
        ..Default::default()
    });
    Arc::new(model)
}

async fn overflow(agent: &mut LocalAgent) -> bool {
    let error = Error::from(ContextWindowOverflowError::new("overflow"));
    after_model_call(agent, Some(&error)).await
}

async fn after_model_call(agent: &mut LocalAgent, error: Option<&Error>) -> bool {
    let hooks = agent.hooks().clone();
    let model = Arc::clone(agent.model());
    let mut event = AfterModelCallEvent::new(agent, model, InvocationState::new(), NonZeroU32::MIN);
    if let Some(error) = error {
        event = event.with_error(error);
    }
    hooks
        .invoke::<AfterModelCallEvent<'static>>(&mut event)
        .await
        .unwrap();
    event.retry()
}

async fn before_model_call(agent: &mut LocalAgent, projected: Option<u64>) {
    let hooks = agent.hooks().clone();
    let model = Arc::clone(agent.model());
    let mut event = BeforeModelCallEvent::new(agent, model, InvocationState::new(), projected);
    hooks
        .invoke::<BeforeModelCallEvent<'static>>(&mut event)
        .await
        .unwrap();
}

#[test]
fn has_correct_plugin_name() {
    let manager = ContextManager::new(ContextManagerConfig::default()).unwrap();
    assert_eq!(Plugin::name(&manager), "strands:context-manager");
    assert_eq!(
        manager.strategy_names(),
        [
            "offload:truncate",
            "offload:summarize",
            "offload:emergency-truncate"
        ]
    );
}

#[test]
fn accepts_custom_strategies() {
    let manager = manager_with(Arc::new(Recorder::default()));
    assert_eq!(
        manager.strategy_names(),
        ["recorder", "offload:emergency-truncate"]
    );
}

#[test]
fn returns_a_context_manager_instance_as_is() {
    let manager = manager_with(Arc::new(Recorder::default()));
    let resolved = ContextManager::from_strategy(Arc::clone(&manager).into())
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&resolved, &manager));
}

#[tokio::test]
async fn initializes_strategies_with_init_context() {
    let strategy = Arc::new(Recorder::default());
    let mut agent = agent_with(Vec::new());
    manager_with(Arc::clone(&strategy))
        .init_agent(&mut agent)
        .await
        .unwrap();
    assert!(strategy.init.load(Ordering::Relaxed));
    assert!(agent.context_stash().is_some());
}

#[tokio::test]
async fn uses_an_explicitly_configured_scoped_stash_storage_as_the_exact_root() {
    let storage = InMemoryStorage::new();
    let view: Arc<dyn Storage> = Arc::new(storage.namespace("team").unwrap());
    let config = ContextManagerConfig {
        strategies: None,
        stash: Some(StashOption::Enabled(StashConfig {
            storage: Some(view),
            retrieval_tool: true,
        })),
    };
    let manager = Arc::new(ContextManager::new(config).unwrap());
    let mut agent = agent_with(vec![user("hello")]);
    let tracking_id = agent.messages()[0].tracking_id().to_owned();
    Arc::clone(&manager).init_agent(&mut agent).await.unwrap();
    assert_eq!(
        Storage::list(&storage, "").await.unwrap(),
        [format!("team/{tracking_id}_0")]
    );
}

#[tokio::test]
async fn keeps_the_per_agent_root_under_a_scoped_agent_storage() {
    let storage = InMemoryStorage::new();
    let view: Arc<dyn Storage> = Arc::new(storage.namespace("team").unwrap());
    let mut agent = agent_with(vec![user("hello")]);
    agent.set_storage(Some(view));
    agent.set_session_id("sess-1".to_owned());
    let tracking_id = agent.messages()[0].tracking_id().to_owned();
    Arc::new(ContextManager::new(ContextManagerConfig::default()).unwrap())
        .init_agent(&mut agent)
        .await
        .unwrap();
    assert_eq!(
        Storage::list(&storage, "").await.unwrap(),
        [format!("team/context/sess-1/scopes/agent/agent/{tracking_id}_0")]
    );
}

#[tokio::test]
async fn does_not_overwrite_a_stashed_original_with_a_seeded_preview() {
    let preview = "[Truncated: 1 block, ~10,973 tokens]\n\nrow-1\n\n[Stashed] [ref: tu-1_0]";
    let mut agent = agent_with(vec![user("task"), tool_result("tu-1", preview)]);
    let manager = Arc::new(ContextManager::new(ContextManagerConfig::default()).unwrap());
    Arc::clone(&manager).init_agent(&mut agent).await.unwrap();
    assert_eq!(manager.stash().unwrap().retrieve("tu-1_0").await.unwrap(), None);
}

#[tokio::test]
async fn runs_strategies_on_context_window_overflow_error_and_sets_retry() {
    let strategy = Arc::new(Recorder {
        acted: true,
        ..Default::default()
    });
    let mut agent = agent_with(conversation(2));
    manager_with(Arc::clone(&strategy))
        .init_agent(&mut agent)
        .await
        .unwrap();
    assert!(overflow(&mut agent).await);
    assert_eq!(strategy.calls.load(Ordering::Relaxed), 1);
    assert!(strategy.seen.lock().unwrap()[0].1);
}

#[tokio::test]
async fn does_not_run_strategies_on_non_overflow_errors() {
    let strategy = Arc::new(Recorder::default());
    let mut agent = agent_with(conversation(2));
    manager_with(Arc::clone(&strategy))
        .init_agent(&mut agent)
        .await
        .unwrap();
    let error = Error::other("some other error");
    assert!(!after_model_call(&mut agent, Some(&error)).await);
    assert_eq!(strategy.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn truncates_when_utilization_remains_at_or_above_1_after_strategies() {
    let strategy = Arc::new(Recorder::default());
    let mut agent = agent_with_model(conversation(5), model_with_window(1));
    manager_with(strategy).init_agent(&mut agent).await.unwrap();
    assert!(overflow(&mut agent).await);
    assert!(agent.messages().len() < 11);
    assert_eq!(first_text(&agent.messages()[0]), "system");
}

#[tokio::test]
async fn truncates_when_a_strategy_throws() {
    let strategy = Arc::new(Recorder {
        fails: true,
        ..Default::default()
    });
    let mut agent = agent_with(conversation(5));
    manager_with(strategy).init_agent(&mut agent).await.unwrap();
    assert!(overflow(&mut agent).await);
    assert!(agent.messages().len() < 11);
}

#[tokio::test]
async fn does_not_truncate_on_non_overflow_when_utilization_is_below_1_0() {
    let strategy = Arc::new(Recorder::default());
    let mut agent = agent_with(conversation(5));
    manager_with(strategy).init_agent(&mut agent).await.unwrap();
    before_model_call(&mut agent, Some(10)).await;
    assert_eq!(agent.messages().len(), 11);
}

#[tokio::test]
async fn emergency_truncate_fires_when_estimate_undercounts_on_overflow() {
    let strategy = Arc::new(Recorder::default());
    let mut agent = agent_with_model(conversation(5), model_with_window(1_000_000));
    manager_with(strategy).init_agent(&mut agent).await.unwrap();
    assert!(overflow(&mut agent).await);
    assert!(agent.messages().len() < 11);
}

#[tokio::test]
async fn caps_retries_at_3_and_stops_setting_retry() {
    let strategy = Arc::new(Recorder {
        acted: true,
        ..Default::default()
    });
    let mut agent = agent_with(conversation(2));
    manager_with(strategy).init_agent(&mut agent).await.unwrap();
    for _ in 0..3 {
        assert!(overflow(&mut agent).await);
    }
    assert!(!overflow(&mut agent).await);
}

#[tokio::test]
async fn resets_retry_counter_on_successful_non_overflow_model_call() {
    let strategy = Arc::new(Recorder {
        acted: true,
        ..Default::default()
    });
    let mut agent = agent_with(conversation(2));
    manager_with(strategy).init_agent(&mut agent).await.unwrap();
    for _ in 0..3 {
        assert!(overflow(&mut agent).await);
    }
    after_model_call(&mut agent, None).await;
    assert!(overflow(&mut agent).await);
}

#[tokio::test]
async fn preserves_the_first_message_content() {
    let mut agent = agent_with(conversation(5));
    manager_with(Arc::new(Recorder::default()))
        .init_agent(&mut agent)
        .await
        .unwrap();
    overflow(&mut agent).await;
    assert_eq!(first_text(&agent.messages()[0]), "system");
}

#[tokio::test]
async fn does_not_orphan_tool_use_tool_result_pairs_in_preserved_region() {
    let mut messages = vec![user("task")];
    for index in 0..5 {
        let id = format!("tu-{index}");
        messages.push(test_support::tool_use(&id, "read"));
        messages.push(tool_result(&id, "x"));
    }
    messages.push(assistant("done"));
    let mut agent = agent_with(messages);
    manager_with(Arc::new(Recorder::default()))
        .init_agent(&mut agent)
        .await
        .unwrap();
    overflow(&mut agent).await;
    let mut uses = HashSet::new();
    for message in agent.messages() {
        for block in message.content() {
            match block {
                ContentBlock::ToolUse(tool_use) => {
                    uses.insert(tool_use.tool_use_id().to_owned());
                }
                ContentBlock::ToolResult(result) => assert!(uses.contains(result.tool_use_id())),
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn does_not_truncate_when_messages_at_most_3() {
    let mut agent = agent_with(conversation(1));
    manager_with(Arc::new(Recorder::default()))
        .init_agent(&mut agent)
        .await
        .unwrap();
    assert!(!overflow(&mut agent).await);
    assert_eq!(agent.messages().len(), 3);
}

#[tokio::test]
async fn runs_strategies_with_projected_input_tokens() {
    let strategy = Arc::new(Recorder::default());
    let mut agent = agent_with_model(conversation(1), model_with_window(1000));
    manager_with(Arc::clone(&strategy))
        .init_agent(&mut agent)
        .await
        .unwrap();
    before_model_call(&mut agent, Some(500)).await;
    assert_eq!(*strategy.seen.lock().unwrap(), [(0.5, false)]);
}

#[tokio::test]
async fn registers_retrieve_context_after_plugin_initialization() {
    let manager = Arc::new(ContextManager::new(ContextManagerConfig::default()).unwrap());
    assert!(manager.get_tools().is_empty());
    let mut agent = agent_with(Vec::new());
    Arc::clone(&manager).init_agent(&mut agent).await.unwrap();
    let names: Vec<String> = manager
        .get_tools()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
    assert_eq!(names, [RETRIEVAL_TOOL_NAME]);
}

#[tokio::test]
async fn does_not_register_retrieve_context_when_stash_is_disabled() {
    let config = ContextManagerConfig {
        strategies: None,
        stash: Some(false.into()),
    };
    let manager = Arc::new(ContextManager::new(config).unwrap());
    let mut agent = agent_with(Vec::new());
    Arc::clone(&manager).init_agent(&mut agent).await.unwrap();
    assert!(manager.get_tools().is_empty());
    assert!(manager.stash().is_none());
}

#[tokio::test]
async fn keeps_the_stash_without_the_tool_when_retrieval_tool_is_false() {
    let config = ContextManagerConfig {
        strategies: None,
        stash: Some(StashConfig::builder().retrieval_tool(false).build().into()),
    };
    let manager = Arc::new(ContextManager::new(config).unwrap());
    let mut agent = agent_with(Vec::new());
    Arc::clone(&manager).init_agent(&mut agent).await.unwrap();
    assert!(manager.get_tools().is_empty());
    assert!(manager.stash().is_some());
}

#[tokio::test]
async fn backfills_stash_from_pre_existing_messages_at_init() {
    let mut agent = agent_with(vec![
        user("q"),
        assistant("a"),
        tool_result("tu-1", "original output"),
    ]);
    let manager = Arc::new(ContextManager::new(ContextManagerConfig::default()).unwrap());
    Arc::clone(&manager).init_agent(&mut agent).await.unwrap();
    assert_eq!(
        manager.stash().unwrap().retrieve("tu-1_0").await.unwrap(),
        Some(serde_json::json!({"text": "original output"}))
    );
}

fn first_text(message: &Message) -> &str {
    test_support::first_text(message)
}

fn conversation_manager_name(agent: &Agent) -> String {
    agent.conversation_manager().name().to_owned()
}

fn model() -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn(vec![crate::types::messages::TextBlock::new("hi").into()]);
    model
}

fn agent_config(context_manager: Option<ContextManagerStrategy>) -> AgentConfig {
    AgentConfig {
        model: Some(model().into()),
        context_manager,
        printer: Some(false),
        ..Default::default()
    }
}

fn plugin_names(agent: &Agent) -> Vec<String> {
    agent
        .registration_order()
        .iter()
        .map(|plugin| plugin.name().to_owned())
        .collect()
}

#[test]
fn uses_sliding_window_conversation_manager() {
    let agent = Agent::new(agent_config(None)).unwrap();
    assert_eq!(
        conversation_manager_name(&agent),
        SlidingWindowConversationManager::default().name()
    );
    assert!(agent.context_manager().is_none());
}

#[test]
fn uses_null_conversation_manager_with_auto() {
    let agent = Agent::new(agent_config(Some(ContextManagerPreset::Auto.into()))).unwrap();
    assert_eq!(
        conversation_manager_name(&agent),
        "strands:null-conversation-manager"
    );
    assert!(agent.context_manager().is_some());
    assert!(plugin_names(&agent).contains(&ContextManager::NAME.to_owned()));
}

#[test]
fn registers_context_manager_before_the_session_manager() {
    let mut config = agent_config(Some(ContextManagerPreset::Auto.into()));
    config.session_manager = Some(crate::session::SessionManager::new(Default::default()).unwrap());
    let agent = Agent::new(config).unwrap();
    let names = plugin_names(&agent);
    let context = names
        .iter()
        .position(|name| name == ContextManager::NAME)
        .unwrap();
    let session = names
        .iter()
        .position(|name| name == "strands:session-manager")
        .unwrap();
    assert!(context < session);
}

#[test]
fn ignores_user_provided_conversation_manager() {
    let mut config = agent_config(Some(ContextManagerPreset::Agentic.into()));
    config.conversation_manager = Some(Arc::new(SlidingWindowConversationManager::default()));
    let agent = Agent::new(config).unwrap();
    assert_eq!(
        conversation_manager_name(&agent),
        "strands:null-conversation-manager"
    );
}

#[test]
fn agentic_adds_the_context_tools() {
    let agent = Agent::new(agent_config(Some(ContextManagerPreset::Agentic.into()))).unwrap();
    for name in ["summarize_context", "truncate_context", "pin_context"] {
        assert!(agent.tool_registry().get(name).is_some(), "{name}");
    }
}

#[test]
fn throws_when_used_with_a_stateful_model() {
    let model = model();
    model.update_config(BaseModelConfig {
        model_info: Some(ModelInfo {
            capabilities: Some(ModelCapabilities {
                stateful: Some(true),
                ..Default::default()
            }),
            limits: None,
        }),
        ..Default::default()
    });
    let mut config = agent_config(Some(ContextManagerPreset::Agentic.into()));
    config.model = Some(model.into());
    let error = Agent::new(config).unwrap_err();
    assert_eq!(
        error.to_string(),
        "contextManager and conversationManager cannot be used with a stateful model. The model manages conversation state server-side."
    );
}

#[test]
fn uses_null_conversation_manager_with_a_config_object() {
    let agent = Agent::new(agent_config(Some(ContextManagerConfig::default().into()))).unwrap();
    assert_eq!(
        conversation_manager_name(&agent),
        "strands:null-conversation-manager"
    );
    assert!(plugin_names(&agent).contains(&ContextManager::NAME.to_owned()));
}

#[test]
fn uses_null_conversation_manager_when_no_conversation_manager_provided() {
    let agent = Agent::new(agent_config(Some(ContextManagerStrategy::Off))).unwrap();
    assert_eq!(
        conversation_manager_name(&agent),
        "strands:null-conversation-manager"
    );
    assert!(agent.context_manager().is_none());
    assert!(!plugin_names(&agent).contains(&ContextManager::NAME.to_owned()));
}

#[test]
fn uses_user_provided_conversation_manager() {
    let mut config = agent_config(Some(ContextManagerStrategy::Off));
    config.conversation_manager = Some(Arc::new(SlidingWindowConversationManager::default()));
    let agent = Agent::new(config).unwrap();
    assert_eq!(
        conversation_manager_name(&agent),
        SlidingWindowConversationManager::default().name()
    );
}

#[test]
fn uses_the_instance_as_is() {
    let manager = manager_with(Arc::new(Recorder::default()));
    let agent = Agent::new(agent_config(Some(Arc::clone(&manager).into()))).unwrap();
    assert!(Arc::ptr_eq(agent.context_manager().unwrap(), &manager));
    assert_eq!(
        conversation_manager_name(&agent),
        "strands:null-conversation-manager"
    );
}
