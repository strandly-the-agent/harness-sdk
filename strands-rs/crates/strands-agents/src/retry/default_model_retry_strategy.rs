//! Default model-retry policy (`strands-ts/src/retry/default-model-retry-strategy.ts`).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::backoff::{BackoffContext, BackoffStrategy, ExponentialBackoff, ExponentialBackoffOptions};
use super::model_retry_strategy::{register_model_retry_hook, ModelRetryStrategy, Sleeper, TokioSleeper};
use super::retry_strategy::RetryDecision;
use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::AfterModelCallEvent;
use crate::plugins::Plugin;

/// Total model attempts before giving up: 1 initial call + 5 retries.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 6;
const DEFAULT_BACKOFF_BASE_MS: f64 = 4_000.0;
const DEFAULT_BACKOFF_MAX_MS: f64 = 240_000.0;

/// Classifies an error as retryable or not.
pub type RetryablePredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// Whether the SDK retries `error` by default: throttling only.
pub fn is_default_retryable(error: &Error) -> bool {
    matches!(error, Error::ModelThrottled { .. })
}

/// Options for [`DefaultModelRetryStrategy`].
#[derive(Clone)]
pub struct DefaultModelRetryStrategyOptions {
    /// Total model attempts before giving up and re-raising. Must be >= 1. Default
    /// [`DEFAULT_MAX_ATTEMPTS`].
    pub max_attempts: u32,
    /// Backoff used to compute the delay between retries. Default
    /// `ExponentialBackoff` with `base_ms: 4_000`, `max_ms: 240_000`.
    pub backoff: Arc<dyn BackoffStrategy>,
    /// Which errors to retry. Default [`is_default_retryable`].
    pub retryable: RetryablePredicate,
    /// Sleeper used for the backoff delay. Default [`TokioSleeper`].
    pub sleeper: Arc<dyn Sleeper>,
}

impl Default for DefaultModelRetryStrategyOptions {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: Arc::new(ExponentialBackoff::new(ExponentialBackoffOptions {
                base_ms: DEFAULT_BACKOFF_BASE_MS,
                max_ms: DEFAULT_BACKOFF_MAX_MS,
                ..Default::default()
            })),
            retryable: Arc::new(is_default_retryable),
            sleeper: Arc::new(TokioSleeper),
        }
    }
}

#[derive(Default)]
struct BudgetState {
    last_delay_ms: Option<f64>,
    first_failure_at: Option<Instant>,
}

struct Inner {
    options: DefaultModelRetryStrategyOptions,
    state: Mutex<BudgetState>,
}

/// Retries failed model calls classified as retryable, bounded by `max_attempts`.
///
/// Only [`Error::ModelThrottled`] is retryable by default; pass a
/// [`DefaultModelRetryStrategyOptions::retryable`] predicate to expand or narrow that set without
/// reimplementing the rest of the policy.
///
/// State is per retry budget and resets when `event.attempt_count == 1` — a new turn or a router
/// candidate switch. The attempt counter itself is owned by the agent loop.
///
/// Hook precedence: [`AfterModelCallEvent`] callbacks run in reverse registration order, so
/// user-registered hooks run before this strategy. If a user hook sets `event.retry` first, no
/// additional backoff is stacked on top.
///
/// Cloning yields a handle on the same backoff state — create a separate instance per agent.
///
/// ```ignore
/// let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
///     max_attempts: 4,
///     ..Default::default()
/// })?;
/// ```
#[derive(Clone)]
pub struct DefaultModelRetryStrategy {
    inner: Arc<Inner>,
}

impl DefaultModelRetryStrategy {
    /// Strategy with these options. Errors when `max_attempts < 1`.
    pub fn new(options: DefaultModelRetryStrategyOptions) -> Result<Self> {
        if options.max_attempts < 1 {
            return Err(Error::other(format!(
                "DefaultModelRetryStrategy: max_attempts must be an integer >= 1 (got {})",
                options.max_attempts
            )));
        }
        Ok(Self::build(options))
    }

    fn build(options: DefaultModelRetryStrategyOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                options,
                state: Mutex::new(BudgetState::default()),
            }),
        }
    }

    /// Context for the current attempt, recording the first failure's timestamp.
    fn build_context(&self, attempt_count: u32) -> BackoffContext {
        let mut state = self.inner.state.lock();
        let first_failure_at = *state.first_failure_at.get_or_insert_with(Instant::now);
        let mut ctx = BackoffContext::new(attempt_count)
            .with_elapsed_ms(first_failure_at.elapsed().as_secs_f64() * 1000.0);
        if let Some(last) = state.last_delay_ms {
            ctx = ctx.with_last_delay_ms(last);
        }
        ctx
    }
}

impl Default for DefaultModelRetryStrategy {
    fn default() -> Self {
        Self::build(DefaultModelRetryStrategyOptions::default())
    }
}

impl std::fmt::Debug for DefaultModelRetryStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultModelRetryStrategy")
            .field("max_attempts", &self.inner.options.max_attempts)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Plugin for DefaultModelRetryStrategy {
    fn name(&self) -> &str {
        "strands:default-model-retry-strategy"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        register_model_retry_hook(agent, Arc::new(self.clone()));
        Ok(())
    }
}

#[async_trait]
impl ModelRetryStrategy for DefaultModelRetryStrategy {
    async fn compute_retry_decision(&self, event: &AfterModelCallEvent) -> Result<RetryDecision> {
        let options = &self.inner.options;
        let Some(error) = event.error.as_deref() else {
            return Ok(RetryDecision::No);
        };
        if !(options.retryable)(error) {
            return Ok(RetryDecision::No);
        }

        let max_attempts = options.max_attempts;
        if event.attempt_count >= max_attempts {
            tracing::debug!(
                "attempt_count=<{}> max_attempts=<{max_attempts}> | max retry attempts reached",
                event.attempt_count
            );
            return Ok(RetryDecision::No);
        }

        let wait_ms = options
            .backoff
            .next_delay(&self.build_context(event.attempt_count))?;
        tracing::debug!(
            "retry_delay_ms=<{wait_ms}> attempt_count=<{}> max_attempts=<{max_attempts}> | retryable model error, delaying before retry",
            event.attempt_count
        );

        self.inner.state.lock().last_delay_ms = Some(wait_ms);
        Ok(RetryDecision::Retry { wait_ms })
    }

    fn on_first_model_attempt(&self) {
        *self.inner.state.lock() = BudgetState::default();
    }

    fn sleeper(&self) -> Arc<dyn Sleeper> {
        Arc::clone(&self.inner.options.sleeper)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::backoff::{ConstantBackoff, ConstantBackoffOptions};
    use super::*;
    use crate::agent::{test_agent_for_plugins, InvocationState};

    #[derive(Default)]
    struct RecordingSleeper {
        slept: Mutex<Vec<Duration>>,
    }

    #[async_trait]
    impl Sleeper for RecordingSleeper {
        async fn sleep(&self, duration: Duration) {
            self.slept.lock().push(duration);
        }
    }

    struct RecordingBackoff {
        delay_ms: f64,
        calls: Mutex<Vec<BackoffContext>>,
    }

    impl RecordingBackoff {
        fn new(delay_ms: f64) -> Arc<Self> {
            Arc::new(Self {
                delay_ms,
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    impl BackoffStrategy for RecordingBackoff {
        fn next_delay(&self, ctx: &BackoffContext) -> Result<f64> {
            self.calls.lock().push(ctx.clone());
            Ok(self.delay_ms)
        }
    }

    fn constant(delay_ms: f64) -> Arc<dyn BackoffStrategy> {
        Arc::new(ConstantBackoff::new(ConstantBackoffOptions { delay_ms }))
    }

    struct Harness {
        agent: Agent,
        sleeper: Arc<RecordingSleeper>,
    }

    impl Harness {
        /// Agent with `strategy` attached and a sleeper that records instead of waiting.
        async fn new(max_attempts: u32, backoff: Arc<dyn BackoffStrategy>) -> Self {
            Self::with_retryable(max_attempts, backoff, Arc::new(is_default_retryable)).await
        }

        async fn with_retryable(
            max_attempts: u32,
            backoff: Arc<dyn BackoffStrategy>,
            retryable: RetryablePredicate,
        ) -> Self {
            let sleeper = Arc::new(RecordingSleeper::default());
            let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
                max_attempts,
                backoff,
                retryable,
                sleeper: Arc::clone(&sleeper) as Arc<dyn Sleeper>,
            })
            .unwrap();
            let agent = test_agent_for_plugins();
            strategy.init_agent(&agent).await.unwrap();
            Self { agent, sleeper }
        }

        fn event(&self, error: Error, attempt_count: u32) -> AfterModelCallEvent {
            AfterModelCallEvent {
                agent: self.agent.clone(),
                model: self.agent.model(),
                stop_data: None,
                error: Some(Arc::new(error)),
                invocation_state: InvocationState::new(),
                attempt_count,
                retry: false,
            }
        }

        async fn invoke(&self, event: &mut AfterModelCallEvent) {
            self.agent.hooks().invoke_callbacks(event).await.unwrap();
        }

        fn slept(&self) -> Vec<Duration> {
            self.sleeper.slept.lock().clone()
        }
    }

    #[tokio::test]
    async fn registers_an_after_model_call_event_hook() {
        let agent = test_agent_for_plugins();
        DefaultModelRetryStrategy::default()
            .init_agent(&agent)
            .await
            .unwrap();
        assert_eq!(agent.hooks().count::<AfterModelCallEvent>(), 1);
    }

    #[test]
    fn exposes_the_plugin_name() {
        assert_eq!(
            DefaultModelRetryStrategy::default().name(),
            "strands:default-model-retry-strategy"
        );
    }

    #[test]
    fn is_a_model_retry_strategy() {
        let strategy: Arc<dyn ModelRetryStrategy> = Arc::new(DefaultModelRetryStrategy::default());
        assert_eq!(strategy.name(), "strands:default-model-retry-strategy");
    }

    #[test]
    fn rejects_max_attempts_below_1() {
        let err = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
            max_attempts: 0,
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("max_attempts"), "{err}");
    }

    #[tokio::test]
    async fn sets_retry_true_on_model_throttled_error_and_sleeps_for_the_configured_delay() {
        let h = Harness::new(3, constant(500.0)).await;
        let mut event = h.event(Error::throttled("rate limited"), 1);

        h.invoke(&mut event).await;

        assert!(event.retry);
        assert_eq!(h.slept(), vec![Duration::from_millis(500)]);
    }

    #[tokio::test]
    async fn does_not_retry_cancelled_errors() {
        let h = Harness::with_retryable(3, constant(10.0), Arc::new(|_| true)).await;
        let mut event = h.event(Error::Cancelled, 1);

        h.invoke(&mut event).await;

        assert!(!event.retry);
        assert!(h.slept().is_empty());
    }

    #[tokio::test]
    async fn does_not_retry_non_retryable_errors() {
        let h = Harness::new(6, constant(10.0)).await;
        let mut event = h.event(Error::model("boom"), 1);

        h.invoke(&mut event).await;

        assert!(!event.retry);
        assert!(h.slept().is_empty());
    }

    #[tokio::test]
    async fn stops_retrying_once_max_attempts_is_reached() {
        let h = Harness::new(3, constant(1.0)).await;

        for attempt in 1..=2 {
            let mut event = h.event(Error::throttled("x"), attempt);
            h.invoke(&mut event).await;
            assert!(event.retry, "attempt {attempt} should retry");
        }

        let mut event = h.event(Error::throttled("x"), 3);
        h.invoke(&mut event).await;
        assert!(!event.retry);
    }

    #[tokio::test]
    async fn skips_work_if_another_hook_already_requested_retry() {
        let h = Harness::new(5, constant(1000.0)).await;
        let mut event = h.event(Error::throttled("x"), 1);
        event.retry = true;

        h.invoke(&mut event).await;

        assert!(event.retry);
        assert!(h.slept().is_empty());
    }

    #[tokio::test]
    async fn clears_backoff_state_at_the_start_of_each_new_turn() {
        let backoff = RecordingBackoff::new(5.0);
        let h = Harness::new(5, Arc::clone(&backoff) as Arc<dyn BackoffStrategy>).await;

        h.invoke(&mut h.event(Error::throttled("x"), 1)).await;
        h.invoke(&mut h.event(Error::throttled("x"), 1)).await;

        let calls = backoff.calls.lock().clone();
        assert_eq!(calls[1].attempt, 1);
        assert_eq!(calls[1].last_delay_ms, None);
    }

    #[tokio::test]
    async fn passes_backoff_context_with_attempt_and_last_delay_ms_to_the_backoff_strategy() {
        let backoff = RecordingBackoff::new(5.0);
        let h = Harness::new(5, Arc::clone(&backoff) as Arc<dyn BackoffStrategy>).await;

        h.invoke(&mut h.event(Error::throttled("x"), 1)).await;
        h.invoke(&mut h.event(Error::throttled("x"), 2)).await;

        let calls = backoff.calls.lock().clone();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].attempt, calls[0].last_delay_ms), (1, None));
        assert_eq!((calls[1].attempt, calls[1].last_delay_ms), (2, Some(5.0)));
    }

    #[tokio::test]
    async fn clears_per_turn_state_on_attempt_1_even_when_a_prior_hook_already_set_event_retry() {
        let backoff = RecordingBackoff::new(5.0);
        let h = Harness::new(5, Arc::clone(&backoff) as Arc<dyn BackoffStrategy>).await;

        h.invoke(&mut h.event(Error::throttled("x"), 1)).await;

        let mut preset = h.event(Error::throttled("x"), 1);
        preset.retry = true;
        h.invoke(&mut preset).await;

        h.invoke(&mut h.event(Error::throttled("x"), 2)).await;

        let calls = backoff.calls.lock().clone();
        assert_eq!((calls[1].attempt, calls[1].last_delay_ms), (2, None));
    }

    #[tokio::test]
    async fn lets_callers_expand_the_retryable_set_with_a_predicate() {
        let retryable: RetryablePredicate =
            Arc::new(|error| is_default_retryable(error) || matches!(error, Error::ToolValidation(_)));
        let h = Harness::with_retryable(3, constant(10.0), retryable).await;

        let mut event = h.event(Error::ToolValidation("custom".into()), 1);
        h.invoke(&mut event).await;

        assert!(event.retry);
    }

    #[tokio::test]
    async fn short_circuits_without_retry_when_compute_retry_decision_returns_no_for_a_non_max_reason() {
        let retryable: RetryablePredicate = Arc::new(|error| error.to_string() != "skip");
        let h = Harness::with_retryable(5, constant(10.0), retryable).await;

        let mut event = h.event(Error::throttled("skip"), 1);
        h.invoke(&mut event).await;

        assert!(!event.retry);
        assert!(h.slept().is_empty());
    }

    #[tokio::test]
    async fn ignores_success_events() {
        let h = Harness::new(3, constant(10.0)).await;
        let mut event = AfterModelCallEvent {
            error: None,
            ..h.event(Error::throttled("x"), 1)
        };

        h.invoke(&mut event).await;

        assert!(!event.retry);
        assert!(h.slept().is_empty());
    }
}
