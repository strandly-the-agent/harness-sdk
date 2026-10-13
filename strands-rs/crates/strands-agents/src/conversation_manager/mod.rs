//! Conversation history management (`strands-ts/src/conversation-manager/`).
//!
//! A conversation manager is a [`Plugin`] whose primary job is reactive overflow recovery;
//! proactive compression is opt-in and layered on by [`ConversationManager::register_base_hooks`].

mod null_manager;
mod sliding_window;

use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent};
use crate::models::Model;
use crate::plugins::Plugin;

pub use null_manager::NullConversationManager;
pub use sliding_window::{SlidingWindowConfig, SlidingWindowConversationManager};

/// Default proactive-compression threshold ratio.
pub const DEFAULT_COMPRESSION_THRESHOLD: f64 = 0.7;

/// Proactive compression setting (TS `proactiveCompression?: boolean | { compressionThreshold }`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ProactiveCompression {
    /// Only reactive overflow recovery (TS `false` or omitted).
    #[default]
    Disabled,
    /// Compress at [`DEFAULT_COMPRESSION_THRESHOLD`] (TS `true`).
    Enabled,
    /// Compress at this ratio of the context window, in `(0, 1]` (TS `{ compressionThreshold }`).
    Threshold(f64),
}

impl ProactiveCompression {
    /// The validated threshold, or `None` when proactive compression is disabled.
    pub fn threshold(self) -> Result<Option<f64>> {
        let threshold = match self {
            ProactiveCompression::Disabled => return Ok(None),
            ProactiveCompression::Enabled => DEFAULT_COMPRESSION_THRESHOLD,
            ProactiveCompression::Threshold(t) => t,
        };
        if threshold <= 0.0 || threshold > 1.0 {
            return Err(Error::other(format!(
                "compressionThreshold must be between 0 (exclusive) and 1 (inclusive), got {threshold}"
            )));
        }
        Ok(Some(threshold))
    }
}

/// Why [`ConversationManager::reduce`] was called.
#[derive(Clone, Debug)]
pub enum ReduceTrigger {
    /// Reactive recovery from a context-window overflow: the manager MUST remove enough history
    /// for the next model call to succeed, or the error propagates out of the agent loop.
    Overflow(Arc<Error>),
    /// Proactive compression or window enforcement: best effort, errors are swallowed.
    Proactive,
}

impl ReduceTrigger {
    /// The overflow error, when this is a reactive call.
    pub fn error(&self) -> Option<&Arc<Error>> {
        match self {
            ReduceTrigger::Overflow(error) => Some(error),
            ReduceTrigger::Proactive => None,
        }
    }

    /// `true` for [`ReduceTrigger::Overflow`].
    pub fn is_overflow(&self) -> bool {
        matches!(self, ReduceTrigger::Overflow(_))
    }
}

/// Arguments to [`ConversationManager::reduce`].
pub struct ReduceOptions<'a> {
    /// The agent whose `messages` are mutated in place.
    pub agent: &'a Agent,
    /// Model of the call that triggered the reduction.
    pub model: Arc<dyn Model>,
    /// Reactive overflow or proactive compression.
    pub trigger: ReduceTrigger,
}

/// A conversation history management strategy.
///
/// Implementors are also [`Plugin`]s: their `init_agent` must call
/// [`ConversationManager::register_base_hooks`] (the TS `super.initAgent(agent)` requirement) or
/// overflow recovery and proactive compression are lost.
#[async_trait]
pub trait ConversationManager: Plugin + 'static {
    /// Reduce the conversation history in place; `true` if anything was reduced.
    ///
    /// Reactive calls ([`ReduceTrigger::Overflow`]) must make meaningful progress; proactive calls
    /// are best effort.
    async fn reduce(&self, options: ReduceOptions<'_>) -> Result<bool>;

    /// Validated proactive-compression threshold; `None` disables the proactive path.
    fn compression_threshold(&self) -> Option<f64> {
        None
    }

    /// Register overflow recovery (`AfterModelCallEvent`) and proactive compression
    /// (`BeforeModelCallEvent`). Call this from `Plugin::init_agent`; do not override it.
    fn register_base_hooks(&self, agent: &Agent)
    where
        Self: Sized + Clone,
    {
        let manager = self.clone();
        agent.hooks().add_callback(move |event: &mut AfterModelCallEvent| {
            let manager = manager.clone();
            let overflow = event
                .error
                .clone()
                .filter(|error| matches!(**error, Error::ContextWindowOverflow(_)));
            let agent = event.agent.clone();
            let model = event.model.clone();
            Box::pin(async move {
                if let Some(error) = overflow {
                    let reduced = manager
                        .reduce(ReduceOptions {
                            agent: &agent,
                            model,
                            trigger: ReduceTrigger::Overflow(error),
                        })
                        .await?;
                    if reduced {
                        event.retry = true;
                    }
                }
                Ok(())
            })
        });

        let manager = self.clone();
        agent.hooks().add_callback(move |event: &mut BeforeModelCallEvent| {
            let manager = manager.clone();
            let projected = event.projected_input_tokens;
            let agent = event.agent.clone();
            let model = event.model.clone();
            Box::pin(async move {
                let (Some(threshold), Some(projected)) = (manager.compression_threshold(), projected) else {
                    return Ok(());
                };
                let ratio = model.estimate_utilization(projected);
                if ratio >= threshold {
                    tracing::debug!(
                        "projected_tokens=<{projected}>, ratio=<{ratio:.2}>, compression_threshold=<{threshold}> | compression threshold exceeded, reducing context"
                    );
                    let result = manager
                        .reduce(ReduceOptions {
                            agent: &agent,
                            model,
                            trigger: ReduceTrigger::Proactive,
                        })
                        .await;
                    if let Err(error) = result {
                        tracing::warn!(
                            "conversation_manager=<{}> | proactive compression failed, continuing | error=<{error}>",
                            manager.name()
                        );
                    }
                }
                Ok(())
            })
        });
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::hooks::AfterInvocationEvent;
    use crate::models::{ModelStreamEvent, StreamOptions};
    use crate::types::Message;
    use crate::InvocationState;
    use futures::stream::BoxStream;

    pub(crate) struct LimitedModel(pub(crate) Option<u64>);

    #[async_trait]
    impl Model for LimitedModel {
        fn model_id(&self) -> Option<String> {
            None
        }

        fn context_window_limit(&self) -> Option<u64> {
            self.0
        }

        fn stream<'a>(
            &'a self,
            _messages: &'a [Message],
            _options: &'a StreamOptions,
        ) -> BoxStream<'a, Result<ModelStreamEvent>> {
            Box::pin(futures::stream::empty())
        }
    }

    pub(crate) fn model(limit: Option<u64>) -> Arc<dyn Model> {
        Arc::new(LimitedModel(limit))
    }

    pub(crate) fn overflow() -> Arc<Error> {
        Arc::new(Error::ContextWindowOverflow("Context overflow".to_owned()))
    }

    pub(crate) fn after_model_call(agent: &Agent, error: Option<Arc<Error>>) -> AfterModelCallEvent {
        AfterModelCallEvent {
            agent: agent.clone(),
            model: model(Some(1000)),
            stop_data: None,
            error,
            invocation_state: InvocationState::new(),
            attempt_count: 1,
            retry: false,
        }
    }

    pub(crate) fn before_model_call(
        agent: &Agent,
        model: Arc<dyn Model>,
        projected_input_tokens: Option<u64>,
    ) -> BeforeModelCallEvent {
        BeforeModelCallEvent {
            agent: agent.clone(),
            model,
            invocation_state: InvocationState::new(),
            projected_input_tokens,
            cancel: Default::default(),
        }
    }

    pub(crate) fn after_invocation(agent: &Agent) -> AfterInvocationEvent {
        AfterInvocationEvent {
            agent: agent.clone(),
            invocation_state: InvocationState::new(),
            resume: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::agent::test_agent_with_messages;
    use crate::types::Message;
    use parking_lot::Mutex;

    struct Call {
        agent: Agent,
        model: Arc<dyn Model>,
        trigger: ReduceTrigger,
    }

    #[derive(Default)]
    struct TestInner {
        calls: Mutex<Vec<Call>>,
        should_reduce: bool,
        fail_proactive: bool,
        threshold: Option<f64>,
    }

    #[derive(Clone)]
    struct TestManager {
        inner: Arc<TestInner>,
    }

    impl TestManager {
        fn new() -> Self {
            Self {
                inner: Arc::new(TestInner {
                    should_reduce: true,
                    ..Default::default()
                }),
            }
        }

        fn with(should_reduce: bool, fail_proactive: bool, proactive: ProactiveCompression) -> Result<Self> {
            Ok(Self {
                inner: Arc::new(TestInner {
                    calls: Mutex::new(Vec::new()),
                    should_reduce,
                    fail_proactive,
                    threshold: proactive.threshold()?,
                }),
            })
        }

        fn call_count(&self) -> usize {
            self.inner.calls.lock().len()
        }
    }

    #[async_trait]
    impl Plugin for TestManager {
        fn name(&self) -> &str {
            "test:conversation-manager"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            self.register_base_hooks(agent);
            Ok(())
        }
    }

    #[async_trait]
    impl ConversationManager for TestManager {
        async fn reduce(&self, options: ReduceOptions<'_>) -> Result<bool> {
            let proactive = !options.trigger.is_overflow();
            self.inner.calls.lock().push(Call {
                agent: options.agent.clone(),
                model: options.model.clone(),
                trigger: options.trigger,
            });
            if self.inner.fail_proactive && proactive {
                return Err(Error::other("proactive compression exploded".to_owned()));
            }
            if !self.inner.should_reduce {
                return Ok(false);
            }
            options.agent.with_messages(|messages| {
                if !messages.is_empty() {
                    messages.remove(0);
                }
            });
            Ok(true)
        }

        fn compression_threshold(&self) -> Option<f64> {
            self.inner.threshold
        }
    }

    fn agent() -> Agent {
        test_agent_with_messages(vec![Message::user("Message 1"), Message::assistant("Response 1")])
    }

    #[tokio::test]
    async fn registers_both_after_model_call_event_and_before_model_call_event_hooks() {
        let agent = agent();
        TestManager::new().init_agent(&agent).await.unwrap();

        assert_eq!(agent.hooks().count::<AfterModelCallEvent>(), 1);
        assert_eq!(agent.hooks().count::<BeforeModelCallEvent>(), 1);
    }

    #[tokio::test]
    async fn calls_reduce_and_sets_retry_true_on_context_window_overflow_error_when_reduce_returns_true() {
        let agent = test_agent_with_messages(vec![Message::user("Message 1"), Message::assistant("Response 1")]);
        let manager = TestManager::new();
        manager.init_agent(&agent).await.unwrap();

        let mut event = after_model_call(&agent, Some(overflow()));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 1);
        assert!(event.retry);
        assert_eq!(agent.messages().len(), 1);
    }

    #[tokio::test]
    async fn does_not_set_retry_when_reduce_returns_false() {
        let agent = agent();
        let manager = TestManager::with(false, false, ProactiveCompression::Disabled).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut event = after_model_call(&agent, Some(overflow()));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 1);
        assert!(!event.retry);
    }

    #[tokio::test]
    async fn does_not_call_reduce_for_non_overflow_errors() {
        let agent = agent();
        let manager = TestManager::new();
        manager.init_agent(&agent).await.unwrap();

        let error = Arc::new(Error::other("some other error".to_owned()));
        let mut event = after_model_call(&agent, Some(error));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 0);
        assert!(!event.retry);
    }

    #[tokio::test]
    async fn passes_error_to_reduce_when_called_due_to_overflow() {
        let agent = agent();
        let manager = TestManager::new();
        manager.init_agent(&agent).await.unwrap();

        let error = overflow();
        let mut event = after_model_call(&agent, Some(error.clone()));
        let event_model = event.model.clone();
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let calls = manager.inner.calls.lock();
        assert_eq!(calls.len(), 1);
        assert!(Arc::ptr_eq(calls[0].trigger.error().unwrap(), &error));
        assert!(Arc::ptr_eq(&calls[0].agent.inner, &agent.inner));
        assert!(Arc::ptr_eq(&calls[0].model, &event_model));
    }

    #[tokio::test]
    async fn always_registers_a_before_model_call_hook_regardless_of_proactive_compression_setting() {
        let agent = agent();
        TestManager::new().init_agent(&agent).await.unwrap();

        assert_eq!(agent.hooks().count::<AfterModelCallEvent>(), 1);
        assert_eq!(agent.hooks().count::<BeforeModelCallEvent>(), 1);
    }

    #[tokio::test]
    async fn before_model_call_event_handler_is_a_no_op_when_proactive_compression_is_not_set() {
        let agent = agent();
        let manager = TestManager::new();
        manager.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(Some(1000)), Some(900));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 0);
    }

    #[tokio::test]
    async fn uses_default_threshold_of_0_7_when_proactive_compression_is_true() {
        let agent = agent();
        let manager = TestManager::with(true, false, ProactiveCompression::Enabled).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut below = before_model_call(&agent, model(Some(1000)), Some(650));
        agent.hooks().invoke_callbacks(&mut below).await.unwrap();
        assert_eq!(manager.call_count(), 0);

        let mut above = before_model_call(&agent, model(Some(1000)), Some(800));
        agent.hooks().invoke_callbacks(&mut above).await.unwrap();
        assert_eq!(manager.call_count(), 1);
    }

    #[tokio::test]
    async fn calls_reduce_without_error_when_projected_tokens_exceed_custom_threshold() {
        let agent = agent();
        let manager = TestManager::with(true, false, ProactiveCompression::Threshold(0.5)).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let model = model(Some(1000));
        let mut event = before_model_call(&agent, model.clone(), Some(600));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let calls = manager.inner.calls.lock();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].trigger.error().is_none());
        assert!(Arc::ptr_eq(&calls[0].model, &model));
        assert!(Arc::ptr_eq(&calls[0].agent.inner, &agent.inner));
    }

    #[tokio::test]
    async fn does_not_call_reduce_when_below_threshold() {
        let agent = agent();
        let manager = TestManager::with(true, false, ProactiveCompression::Threshold(0.7)).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(Some(1000)), Some(500));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 0);
    }

    #[tokio::test]
    async fn does_not_call_reduce_when_projected_input_tokens_is_undefined() {
        let agent = agent();
        let manager = TestManager::with(true, false, ProactiveCompression::Enabled).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(Some(1000)), None);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 0);
    }

    #[tokio::test]
    async fn uses_200k_default_when_context_window_limit_is_undefined_and_logs_warning() {
        let agent = agent();
        let manager = TestManager::with(true, false, ProactiveCompression::Threshold(0.7)).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(None), Some(150_000));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 1);
    }

    #[tokio::test]
    async fn does_not_trigger_with_200k_default_when_below_threshold() {
        let agent = agent();
        let manager = TestManager::with(true, false, ProactiveCompression::Threshold(0.7)).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(None), Some(100_000));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(manager.call_count(), 0);
    }

    #[tokio::test]
    async fn swallows_errors_from_proactive_reduce_and_continues() {
        let agent = agent();
        let manager = TestManager::with(true, true, ProactiveCompression::Enabled).unwrap();
        manager.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(Some(1000)), Some(800));
        assert!(agent.hooks().invoke_callbacks(&mut event).await.is_ok());
    }

    #[test]
    fn throws_on_compression_threshold_le_0() {
        assert!(ProactiveCompression::Threshold(0.0).threshold().is_err());
        assert!(ProactiveCompression::Threshold(-1.0).threshold().is_err());
        let message = ProactiveCompression::Threshold(0.0)
            .threshold()
            .unwrap_err()
            .to_string();
        assert!(message.contains("must be between 0 (exclusive) and 1 (inclusive)"));
    }

    #[test]
    fn throws_on_compression_threshold_gt_1() {
        assert!(ProactiveCompression::Threshold(1.5).threshold().is_err());
    }

    #[test]
    fn accepts_compression_threshold_of_exactly_1() {
        assert_eq!(ProactiveCompression::Threshold(1.0).threshold().unwrap(), Some(1.0));
    }

    #[test]
    fn disabled_and_enabled_thresholds() {
        assert_eq!(ProactiveCompression::Disabled.threshold().unwrap(), None);
        assert_eq!(
            ProactiveCompression::Enabled.threshold().unwrap(),
            Some(DEFAULT_COMPRESSION_THRESHOLD)
        );
    }
}
