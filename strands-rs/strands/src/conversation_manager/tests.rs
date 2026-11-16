//! Ported from strands-ts/src/conversation-manager/__tests__/conversation-manager.test.ts and
//! null-conversation-manager.test.ts.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::agent::{Agent, InvocationContext};
use crate::conversation_manager::{
    ConversationManager, ConversationManagerPlugin, ConversationManagerReduceOptions,
    NullConversationManager, ProactiveCompression,
};
use crate::error::{ContextWindowOverflowError, Error, ModelError, Result};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent};
use crate::models::model::{BaseModelConfig, Model};
use crate::models::streaming::Usage;
use crate::plugins::Plugin;
use crate::testing::{MockMessageModel, TestModelProvider};
use crate::types::agent::{InvocationState, InvokeLimits, LocalAgent};
use crate::types::messages::{Message, Role, TextBlock};

#[derive(Debug, Clone, PartialEq)]
struct Call {
    had_error: bool,
    error_message: Option<String>,
    model_id: Option<String>,
    has_context: bool,
}

/// TS `TestConversationManager`/`ThresholdTestManager`/the capturing managers in one: records
/// each call, then drops the first message and returns `true` unless told otherwise.
#[derive(Default)]
struct TestManager {
    threshold: Option<f64>,
    decline: bool,
    fail_proactive: bool,
    calls: Mutex<Vec<Call>>,
    contexts: Mutex<Vec<InvocationContext>>,
}

impl TestManager {
    fn with_threshold(compression: ProactiveCompression) -> Self {
        Self {
            threshold: compression.threshold().unwrap(),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ConversationManager for TestManager {
    fn name(&self) -> &str {
        "test:conversation-manager"
    }

    fn compression_threshold(&self) -> Option<f64> {
        self.threshold
    }

    async fn reduce(&self, mut options: ConversationManagerReduceOptions<'_>) -> Result<bool> {
        self.calls.lock().unwrap().push(Call {
            had_error: options.error().is_some(),
            error_message: options.error().map(|error| error.message().to_owned()),
            model_id: options.model().model_id(),
            has_context: options.invocation_context().is_some(),
        });
        if let Some(context) = options.invocation_context() {
            self.contexts.lock().unwrap().push(context.clone());
        }
        if self.fail_proactive && options.error().is_none() {
            return Err(Error::other("proactive compression exploded"));
        }
        if self.decline {
            return Ok(false);
        }
        options.agent_mut().messages_mut().remove(0);
        Ok(true)
    }
}

fn two_messages() -> Vec<Message> {
    vec![
        Message::new(Role::User, vec![TextBlock::new("Message 1").into()]),
        Message::new(Role::Assistant, vec![TextBlock::new("Response 1").into()]),
    ]
}

fn limited_model(limit: Option<u64>) -> Arc<dyn Model> {
    let model = TestModelProvider::new();
    model.update_config(BaseModelConfig {
        model_id: Some("limited".to_owned()),
        context_window_limit: limit,
        ..BaseModelConfig::default()
    });
    Arc::new(model)
}

async fn agent_with(manager: &Arc<TestManager>, messages: Vec<Message>) -> LocalAgent {
    let mut agent = LocalAgent::new(limited_model(Some(1000)));
    agent.set_messages(messages);
    let plugin = Arc::new(ConversationManagerPlugin(
        Arc::clone(manager) as Arc<dyn ConversationManager>
    ));
    plugin.init_agent(&mut agent).await.unwrap();
    agent
}

/// Fires `AfterModelCallEvent` with `error`; returns the event's `retry`.
async fn after_model_call(agent: &mut LocalAgent, error: &Error) -> bool {
    let hooks = agent.hooks().clone();
    let model = limited_model(Some(1000));
    let mut event =
        AfterModelCallEvent::new(agent, model, InvocationState::new(), NonZeroU32::MIN).with_error(error);
    hooks
        .invoke::<AfterModelCallEvent<'static>>(&mut event)
        .await
        .unwrap();
    event.retry()
}

async fn before_model_call(agent: &mut LocalAgent, model: Arc<dyn Model>, projected: Option<u64>) {
    let hooks = agent.hooks().clone();
    let mut event = BeforeModelCallEvent::new(agent, model, InvocationState::new(), projected);
    hooks
        .invoke::<BeforeModelCallEvent<'static>>(&mut event)
        .await
        .unwrap();
}

fn overflow() -> Error {
    ContextWindowOverflowError::new("overflow").into()
}

mod init_agent {
    use super::*;

    #[tokio::test]
    async fn registers_both_after_model_call_event_and_before_model_call_event_hooks() {
        let manager = Arc::new(TestManager::default());
        let agent = agent_with(&manager, Vec::new()).await;
        assert_eq!(agent.hooks().callback_count::<AfterModelCallEvent<'static>>(), 1);
        assert_eq!(agent.hooks().callback_count::<BeforeModelCallEvent<'static>>(), 1);
    }

    #[tokio::test]
    async fn calls_reduce_and_sets_retry_true_on_context_window_overflow_error_when_reduce_returns_true() {
        let manager = Arc::new(TestManager::default());
        let mut agent = agent_with(&manager, two_messages()).await;
        assert!(after_model_call(&mut agent, &overflow()).await);
        assert_eq!(manager.calls().len(), 1);
        assert_eq!(agent.messages().len(), 1);
    }

    #[tokio::test]
    async fn does_not_set_retry_when_reduce_returns_false() {
        let manager = Arc::new(TestManager {
            decline: true,
            ..TestManager::default()
        });
        let mut agent = agent_with(&manager, Vec::new()).await;
        assert!(!after_model_call(&mut agent, &overflow()).await);
        assert_eq!(manager.calls().len(), 1);
    }

    #[tokio::test]
    async fn does_not_call_reduce_for_non_overflow_errors() {
        let manager = Arc::new(TestManager::default());
        let mut agent = agent_with(&manager, Vec::new()).await;
        let error = ModelError::new("some other error").into();
        assert!(!after_model_call(&mut agent, &error).await);
        assert!(manager.calls().is_empty());
    }

    #[tokio::test]
    async fn passes_error_to_reduce_when_called_due_to_overflow() {
        let manager = Arc::new(TestManager {
            decline: true,
            ..TestManager::default()
        });
        let mut agent = agent_with(&manager, Vec::new()).await;
        after_model_call(&mut agent, &overflow()).await;
        assert_eq!(
            manager.calls(),
            [Call {
                had_error: true,
                error_message: Some("overflow".to_owned()),
                model_id: Some("limited".to_owned()),
                has_context: false,
            }]
        );
    }

    #[tokio::test]
    async fn passes_reduce_a_handle_that_counts_toward_the_request_usage_without_its_limits() {
        let manager = Arc::new(TestManager {
            decline: true,
            ..TestManager::default()
        });
        let mut agent = agent_with(&manager, Vec::new()).await;
        let limits = InvokeLimits {
            turns: NonZeroU32::new(1),
            ..InvokeLimits::default()
        };
        let request = InvocationContext::new(Some(limits), None, false);
        request.set_turns_taken(1);
        agent.set_invocation_context(Some(request.clone()));
        after_model_call(&mut agent, &overflow()).await;
        let handed = manager.contexts.lock().unwrap()[0].clone();
        handed.record_usage(&Usage::new(1, 2, 3));
        assert!(handed.limits().is_none());
        let usage = request.usage();
        assert_eq!((usage.turns_taken(), *usage.tokens()), (1, Usage::new(1, 2, 3)));
    }
}

mod proactive_compression {
    use super::*;

    fn model() -> Arc<dyn Model> {
        limited_model(Some(1000))
    }

    #[tokio::test]
    async fn always_registers_a_before_model_call_event_hook_regardless_of_proactive_compression_setting() {
        let manager = Arc::new(TestManager::default());
        let agent = agent_with(&manager, Vec::new()).await;
        assert_eq!(agent.hooks().callback_count::<AfterModelCallEvent<'static>>(), 1);
        assert_eq!(agent.hooks().callback_count::<BeforeModelCallEvent<'static>>(), 1);
    }

    #[tokio::test]
    async fn before_model_call_event_handler_is_a_no_op_when_proactive_compression_is_not_set() {
        let manager = Arc::new(TestManager::default());
        let mut agent = agent_with(&manager, Vec::new()).await;
        before_model_call(&mut agent, model(), Some(900)).await;
        assert!(manager.calls().is_empty());
    }

    #[tokio::test]
    async fn uses_default_threshold_of_0_7_when_proactive_compression_is_true() {
        let manager = Arc::new(TestManager::with_threshold(true.into()));
        let mut agent = agent_with(&manager, two_messages()).await;
        before_model_call(&mut agent, model(), Some(650)).await;
        assert!(manager.calls().is_empty());
        before_model_call(&mut agent, model(), Some(800)).await;
        assert_eq!(manager.calls().len(), 1);
    }

    #[tokio::test]
    async fn calls_reduce_without_error_when_projected_tokens_exceed_custom_threshold() {
        let manager = Arc::new(TestManager::with_threshold(ProactiveCompression::Threshold(0.5)));
        let mut agent = agent_with(&manager, two_messages()).await;
        before_model_call(&mut agent, model(), Some(600)).await;
        assert_eq!(
            manager.calls(),
            [Call {
                had_error: false,
                error_message: None,
                model_id: Some("limited".to_owned()),
                has_context: false,
            }]
        );
        assert_eq!(agent.messages().len(), 1);
    }

    #[tokio::test]
    async fn does_not_call_reduce_when_below_threshold() {
        let manager = Arc::new(TestManager::with_threshold(ProactiveCompression::Threshold(0.7)));
        let mut agent = agent_with(&manager, Vec::new()).await;
        before_model_call(&mut agent, model(), Some(500)).await;
        assert!(manager.calls().is_empty());
    }

    #[tokio::test]
    async fn does_not_call_reduce_when_projected_input_tokens_is_undefined() {
        let manager = Arc::new(TestManager::with_threshold(ProactiveCompression::Threshold(0.7)));
        let mut agent = agent_with(&manager, Vec::new()).await;
        before_model_call(&mut agent, model(), None).await;
        assert!(manager.calls().is_empty());
    }

    // The TS case also checks the warn-once message; that warning is process-wide once, so
    // another test may have logged it first (it is covered in `models::model::tests`).
    #[tokio::test]
    async fn uses_200k_default_when_context_window_limit_is_undefined_and_logs_warning() {
        let manager = Arc::new(TestManager::with_threshold(ProactiveCompression::Threshold(0.7)));
        let mut agent = agent_with(&manager, two_messages()).await;
        before_model_call(&mut agent, limited_model(None), Some(150_000)).await;
        assert_eq!(manager.calls().len(), 1);
    }

    #[tokio::test]
    async fn does_not_trigger_with_200k_default_when_below_threshold() {
        let manager = Arc::new(TestManager::with_threshold(ProactiveCompression::Threshold(0.7)));
        let mut agent = agent_with(&manager, Vec::new()).await;
        before_model_call(&mut agent, limited_model(None), Some(100_000)).await;
        assert!(manager.calls().is_empty());
    }

    #[tokio::test]
    async fn swallows_errors_from_proactive_reduce_and_continues() {
        let manager = Arc::new(TestManager {
            threshold: Some(0.7),
            fail_proactive: true,
            ..TestManager::default()
        });
        let mut agent = agent_with(&manager, Vec::new()).await;
        before_model_call(&mut agent, model(), Some(800)).await;
        assert_eq!(manager.calls().len(), 1);
    }

    fn threshold_error(threshold: f64) -> String {
        ProactiveCompression::Threshold(threshold)
            .threshold()
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn throws_on_compression_threshold_0() {
        assert!(threshold_error(0.0).contains("must be between 0 (exclusive) and 1 (inclusive)"));
        assert_eq!(
            threshold_error(-1.0),
            "compressionThreshold must be between 0 (exclusive) and 1 (inclusive), got -1"
        );
    }

    #[test]
    fn throws_on_compression_threshold_1() {
        assert!(threshold_error(1.5).contains("must be between 0 (exclusive) and 1 (inclusive)"));
    }

    #[test]
    fn accepts_compression_threshold_of_exactly_1() {
        assert_eq!(
            ProactiveCompression::Threshold(1.0).threshold().unwrap(),
            Some(1.0)
        );
    }
}

mod overflow_propagation {
    use super::*;

    #[tokio::test]
    async fn propagates_context_window_overflow_error_out_of_the_agent_loop_when_reduce_returns_false() {
        let model = MockMessageModel::new();
        model.add_error(|| ContextWindowOverflowError::new("context window exceeded").into());
        let mut agent = Agent::builder()
            .model(model)
            .conversation_manager(NullConversationManager::new())
            .printer(false)
            .build()
            .unwrap();
        let error = agent.invoke("hello").await.unwrap_err();
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
    }
}

mod agent_loop {
    use super::*;
    use crate::conversation_manager::{
        SlidingWindowConversationManager, SlidingWindowConversationManagerConfig,
    };

    #[tokio::test]
    async fn the_default_manager_trims_the_history_and_retries_an_overflowing_call() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ContextWindowOverflowError::new("context window exceeded").into());
        model.add_turn(vec![TextBlock::new("recovered").into()]);
        let config = SlidingWindowConversationManagerConfig::builder()
            .window_size(2)
            .should_truncate_results(false)
            .build();
        let mut history = two_messages();
        history.extend(two_messages());
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .messages(history)
            .conversation_manager(SlidingWindowConversationManager::new(config).unwrap())
            .printer(false)
            .build()
            .unwrap();
        let result = agent.invoke("hello").await.unwrap();
        assert_eq!(result.to_string(), "recovered");
        assert_eq!(model.call_count(), 2);
        assert_eq!(model.requests()[1].messages.len(), 1, "trimmed to the new prompt");
    }
}

mod null_conversation_manager {
    use super::*;

    async fn null_agent(messages: Vec<Message>) -> LocalAgent {
        let mut agent = LocalAgent::new(limited_model(Some(1000)));
        agent.set_messages(messages);
        let plugin = Arc::new(ConversationManagerPlugin(
            Arc::new(NullConversationManager::new()),
        ));
        plugin.init_agent(&mut agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn does_not_modify_conversation_history_on_overflow() {
        let messages = two_messages();
        let mut agent = null_agent(messages.clone()).await;
        after_model_call(&mut agent, &overflow()).await;
        assert_eq!(agent.messages(), messages);
    }

    #[tokio::test]
    async fn does_not_set_retry_on_context_overflow() {
        let mut agent = null_agent(two_messages()).await;
        assert!(!after_model_call(&mut agent, &overflow()).await);
    }

    #[tokio::test]
    async fn registers_both_the_overflow_recovery_and_proactive_compression_hooks() {
        let agent = null_agent(Vec::new()).await;
        assert_eq!(agent.hooks().callback_count::<AfterModelCallEvent<'static>>(), 1);
        assert_eq!(agent.hooks().callback_count::<BeforeModelCallEvent<'static>>(), 1);
    }

    #[test]
    fn returns_the_plugin_name() {
        assert_eq!(
            NullConversationManager::new().name(),
            "strands:null-conversation-manager"
        );
    }
}
