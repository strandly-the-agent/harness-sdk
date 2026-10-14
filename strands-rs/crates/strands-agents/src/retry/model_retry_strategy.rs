//! Model-retry orchestration (`strands-ts/src/retry/model-retry-strategy.ts`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::retry_strategy::RetryDecision;
use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::AfterModelCallEvent;
use crate::plugins::Plugin;

/// Sleeps between retry attempts. Injectable so tests don't wait real time.
#[async_trait]
pub trait Sleeper: Send + Sync {
    /// Sleep for `duration`.
    async fn sleep(&self, duration: Duration);
}

/// Default [`Sleeper`]: `tokio::time::sleep`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// A retry policy for failed model calls.
///
/// Implementors decide *whether* to retry and *how long* to wait in
/// [`ModelRetryStrategy::compute_retry_decision`]; [`ModelRetryStrategy::retry_model`] owns the
/// orchestration around it and should not be overridden. As a [`Plugin`], a strategy registers
/// itself on an agent by calling [`register_model_retry_hook`] from `init_agent`.
///
/// Instances carry per-budget state, so one instance belongs to one agent. Handles that share
/// state (`Clone` of an `Arc`-backed strategy) are the same strategy, not a copy of it.
#[async_trait]
pub trait ModelRetryStrategy: Plugin {
    /// Whether to retry the failed model call, and how long to wait first.
    ///
    /// Called only for error events not already marked for retry by another hook.
    async fn compute_retry_decision(&self, event: &AfterModelCallEvent) -> Result<RetryDecision>;

    /// Called when `event.attempt_count == 1`, at the start of a fresh retry budget — a new turn,
    /// or a model-routing candidate switch. Strategies with per-budget state clear it here.
    fn on_first_model_attempt(&self) {}

    /// Sleeper used for the backoff delay. Defaults to [`TokioSleeper`].
    fn sleeper(&self) -> Arc<dyn Sleeper> {
        Arc::new(TokioSleeper)
    }

    /// Hook body invoked for every [`AfterModelCallEvent`]; sets `event.retry` after sleeping.
    ///
    /// Override [`ModelRetryStrategy::compute_retry_decision`] or
    /// [`ModelRetryStrategy::on_first_model_attempt`] instead of this method.
    async fn retry_model(&self, event: &mut AfterModelCallEvent) -> Result<()> {
        // Resets per-budget state before any short-circuit, including when another hook already
        // requested a retry on the first attempt.
        if event.attempt_count == 1 {
            self.on_first_model_attempt();
        }

        if event.retry {
            return Ok(());
        }
        let Some(error) = event.error.as_deref() else {
            return Ok(());
        };
        // The agent loop re-raises cancellation instead of retrying it.
        if matches!(error, Error::Cancelled) {
            return Ok(());
        }

        let RetryDecision::Retry { wait_ms } = self.compute_retry_decision(event).await? else {
            return Ok(());
        };

        self.sleeper()
            .sleep(Duration::from_secs_f64(wait_ms.max(0.0) / 1000.0))
            .await;
        event.retry = true;
        Ok(())
    }
}

/// Register `strategy` as the agent's [`AfterModelCallEvent`] retry hook.
///
/// Called from a strategy's [`Plugin::init_agent`]; hook registration is the framework's job so a
/// strategy cannot forget it.
pub fn register_model_retry_hook(agent: &Agent, strategy: Arc<dyn ModelRetryStrategy>) {
    let _ = agent
        .hooks()
        .add_callback::<AfterModelCallEvent, _>(move |event| {
            let strategy = Arc::clone(&strategy);
            Box::pin(async move { strategy.retry_model(event).await })
        });
}
