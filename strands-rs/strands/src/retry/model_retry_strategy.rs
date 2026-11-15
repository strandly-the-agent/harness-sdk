//! The base of model retry strategies (TS `retry/model-retry-strategy.ts`).
//!
//! Each strategy is registered as a plugin that installs one `AfterModelCallEvent` hook when
//! the agent initializes.

use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures::future::{self, BoxFuture, Either};

use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::hooks::{AfterModelCallEvent, HookRegistry};
use crate::plugins::Plugin;
use crate::retry::retry_strategy::RetryDecision;
use crate::types::agent::LocalAgent;

/// What a [`ModelRetryStrategy`] knows about a finished model call (the fields of TS
/// `AfterModelCallEvent` the strategies read).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ModelRetryContext<'a> {
    attempt_count: NonZeroU32,
    error: Option<&'a Error>,
}

impl<'a> ModelRetryContext<'a> {
    /// The context of attempt `attempt_count` (1-based) of the current model call.
    pub fn new(attempt_count: NonZeroU32) -> Self {
        Self {
            attempt_count,
            error: None,
        }
    }

    /// Sets the error the attempt failed with.
    #[must_use]
    pub fn with_error(mut self, error: &'a Error) -> Self {
        self.error = Some(error);
        self
    }

    /// The 1-based attempt number; 1 is the first attempt of a model call.
    pub fn attempt_count(&self) -> NonZeroU32 {
        self.attempt_count
    }

    /// The error the attempt failed with; `None` when it succeeded.
    pub fn error(&self) -> Option<&'a Error> {
        self.error
    }
}

/// Decides whether a failed model call is retried (TS `ModelRetryStrategy`).
///
/// The agent calls [`on_first_model_attempt`](ModelRetryStrategy::on_first_model_attempt) on
/// every first attempt, then [`compute_retry_decision`](ModelRetryStrategy::compute_retry_decision)
/// for a failed attempt no other hook already retries, and waits out the delay unless the
/// invocation is cancelled.
#[async_trait]
pub trait ModelRetryStrategy: Send + Sync + 'static {
    /// The strategy's plugin name.
    fn name(&self) -> &str;

    /// Whether to retry the failed attempt in `context`, and after how long.
    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision;

    /// Resets per-call state; called on attempt 1 of every model call.
    fn on_first_model_attempt(&self) {}

    /// The implementing type's name, for the TS duplicate-type warning and attach error.
    #[doc(hidden)]
    fn type_name(&self) -> &'static str {
        let name = std::any::type_name::<Self>();
        name.rsplit("::").next().unwrap_or(name)
    }
}

impl fmt::Debug for dyn ModelRetryStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelRetryStrategy")
            .field("name", &self.name())
            .finish()
    }
}

/// Which agent each strategy instance is attached to (TS `_attachedAgent`), keyed by address;
/// the weak handles keep the addresses unique while an entry lives.
type Attachment = (Weak<dyn ModelRetryStrategy>, Weak<()>);
static ATTACHED: Mutex<Vec<Attachment>> = Mutex::new(Vec::new());

/// Attaches `strategy` to the agent behind `agent` (TS `initAgent`'s check).
///
/// # Errors
///
/// The instance is already attached to another live agent.
pub(crate) fn attach(strategy: &Arc<dyn ModelRetryStrategy>, agent: &Arc<()>) -> Result<()> {
    let mut attached = ATTACHED.lock().unwrap_or_else(PoisonError::into_inner);
    attached.retain(|(strategy, agent)| strategy.strong_count() > 0 && agent.strong_count() > 0);
    let address = Arc::as_ptr(strategy).cast::<()>();
    match attached
        .iter()
        .find(|(existing, _)| existing.as_ptr().cast::<()>() == address)
    {
        Some((_, owner)) if !std::ptr::eq(owner.as_ptr(), Arc::as_ptr(agent)) => Err(Error::other(format!(
            "{}: instance is already attached to another agent. Create a separate instance per agent.",
            strategy.type_name()
        ))),
        Some(_) => Ok(()),
        None => {
            attached.push((Arc::downgrade(strategy), Arc::downgrade(agent)));
            Ok(())
        }
    }
}

/// Logs the TS warning for two strategies of one type (TS `warnOnDuplicateRetryStrategyTypes`).
pub(crate) fn warn_on_duplicate_types(strategies: &[Arc<dyn ModelRetryStrategy>]) {
    let mut seen = Vec::new();
    for strategy in strategies {
        let name = strategy.type_name();
        if seen.contains(&name) {
            tracing::warn!(
                "retry_strategy_type=<{name}> | multiple instances provided; only the first will be used"
            );
        } else {
            seen.push(name);
        }
    }
}

/// Registers the `AfterModelCallEvent` hook that applies `strategy` (TS `initAgent`): it sleeps
/// out the backoff inside the hook, then sets `retry`.
pub(crate) fn install(strategy: Arc<dyn ModelRetryStrategy>, hooks: &HookRegistry, sleeper: Sleeper) {
    hooks.add_async_hook::<AfterModelCallEvent<'static>>(move |event| {
        let strategy = Arc::clone(&strategy);
        let sleeper = Arc::clone(&sleeper);
        Box::pin(async move {
            let attempt = event.attempt_count();
            let mut context = ModelRetryContext::new(attempt);
            if let Some(error) = event.error() {
                context = context.with_error(error);
            }
            let cancel_signal = event.agent().cancel_signal().clone();
            if retry_model(
                &*strategy,
                &context,
                event.retry(),
                Some(&cancel_signal),
                &sleeper,
            )
            .await
            {
                event.set_retry(true);
            }
        })
    });
}

/// A retry strategy registered as the plugin it is in TS (`ModelRetryStrategy implements
/// Plugin`): `init_agent` attaches it to the agent behind `agent` and installs its hook.
pub(crate) struct RetryStrategyInstaller {
    pub(crate) strategy: Arc<dyn ModelRetryStrategy>,
    pub(crate) agent: Arc<()>,
    pub(crate) sleeper: Sleeper,
}

#[async_trait]
impl Plugin for RetryStrategyInstaller {
    fn name(&self) -> &str {
        self.strategy.name()
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        attach(&self.strategy, &self.agent)?;
        install(
            Arc::clone(&self.strategy),
            agent.hooks(),
            Arc::clone(&self.sleeper),
        );
        Ok(())
    }
}

/// Waits out a retry delay; tests inject one that records instead of sleeping.
pub(crate) type Sleeper = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// The sleeper that waits on the tokio timer.
pub(crate) fn tokio_sleeper() -> Sleeper {
    Arc::new(|delay| Box::pin(tokio::time::sleep(delay)))
}

/// Applies `strategy` to one finished attempt (TS `retryModel`); returns whether the call is
/// retried. `retry_requested` is whether an earlier hook already asked for a retry.
pub(crate) async fn retry_model(
    strategy: &dyn ModelRetryStrategy,
    context: &ModelRetryContext<'_>,
    retry_requested: bool,
    cancel_signal: Option<&CancelSignal>,
    sleeper: &Sleeper,
) -> bool {
    // Reset per-call state before any short-circuit, including another hook's retry on attempt 1.
    if context.attempt_count() == NonZeroU32::MIN {
        strategy.on_first_model_attempt();
    }
    if retry_requested {
        return true;
    }
    if context.error().is_none() {
        return false;
    }
    let RetryDecision::Retry(delay) = strategy.compute_retry_decision(context).await else {
        return false;
    };
    sleep_unless_cancelled(sleeper, delay, cancel_signal).await;
    true
}

/// Sleeps for `delay`, returning early on cancel so the loop re-checks cancellation before the
/// next attempt.
async fn sleep_unless_cancelled(sleeper: &Sleeper, delay: Duration, cancel_signal: Option<&CancelSignal>) {
    let Some(signal) = cancel_signal else {
        sleeper(delay).await;
        return;
    };
    if signal.is_cancelled() {
        return;
    }
    let cancelled = signal.cancelled();
    futures::pin_mut!(cancelled);
    match future::select(sleeper(delay), cancelled).await {
        Either::Left(((), _)) | Either::Right(((), _)) => {}
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::Sleeper;

    /// A sleeper that records each delay and returns at once.
    pub(crate) fn recording_sleeper() -> (Sleeper, Arc<Mutex<Vec<Duration>>>) {
        let delays = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&delays);
        let sleeper: Sleeper = Arc::new(move |delay| {
            recorded.lock().unwrap().push(delay);
            Box::pin(async {})
        });
        (sleeper, delays)
    }
}
