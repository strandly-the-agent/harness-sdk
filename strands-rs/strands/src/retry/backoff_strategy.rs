//! Backoff strategies: the delay before the next retry attempt (TS `retry/backoff-strategy.ts`).
//!
//! A [`BackoffStrategy`] is pure delay math. Whether to retry, max attempts and time budgets live
//! in the retry strategy.

use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// What a [`BackoffStrategy`] knows when it computes a delay (TS `BackoffContext`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackoffContext {
    /// The 1-based index of the attempt that just failed.
    pub attempt: NonZeroU32,
    /// Time since the first failed attempt.
    pub elapsed: Duration,
    /// The previously computed delay; `None` before the first retry.
    pub last_delay: Option<Duration>,
}

impl BackoffContext {
    /// The context after failed attempt `attempt`, with no time elapsed and no earlier delay.
    pub fn new(attempt: NonZeroU32) -> Self {
        Self {
            attempt,
            elapsed: Duration::ZERO,
            last_delay: None,
        }
    }

    /// Sets the time since the first failed attempt.
    #[must_use]
    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    /// Sets the previously computed delay.
    #[must_use]
    pub fn with_last_delay(mut self, last_delay: Duration) -> Self {
        self.last_delay = Some(last_delay);
        self
    }
}

/// Computes the delay before the next retry attempt (TS `BackoffStrategy`).
pub trait BackoffStrategy: Send + Sync {
    /// The delay before the attempt after `context.attempt`.
    fn next_delay(&self, context: &BackoffContext) -> Duration;
}

/// How a computed delay is randomized (TS `JitterKind`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum JitterKind {
    /// The raw delay unchanged (`none`).
    None,
    /// Uniform in `[0, raw]` (`full`).
    #[default]
    Full,
    /// `raw / 2` plus uniform in `[0, raw / 2]` (`equal`).
    Equal,
    /// Uniform in `[base, min(max, last_delay * 3)]`; full jitter on the first retry
    /// (`decorrelated`).
    Decorrelated,
}

/// A source of uniform random numbers in `[0, 1)`.
pub(crate) type Random = Arc<dyn Fn() -> f64 + Send + Sync>;

fn default_random() -> Random {
    Arc::new(fastrand::f64)
}

const DEFAULT_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_MAX: Duration = Duration::from_secs(30);

/// Options for [`ConstantBackoff`] (TS `ConstantBackoffOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct ConstantBackoffOptions {
    /// The delay for every retry (TS `delayMs`). Default 1 s.
    #[builder(default = DEFAULT_DELAY)]
    pub delay: Duration,
}

impl Default for ConstantBackoffOptions {
    fn default() -> Self {
        Self { delay: DEFAULT_DELAY }
    }
}

/// The same delay for every retry (TS `ConstantBackoff`).
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// use strands::retry::{BackoffContext, BackoffStrategy, ConstantBackoff, ConstantBackoffOptions};
///
/// let backoff = ConstantBackoff::new(ConstantBackoffOptions { delay: Duration::from_millis(250) });
/// let context = BackoffContext::new(NonZeroU32::MIN);
/// assert_eq!(backoff.next_delay(&context), Duration::from_millis(250));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantBackoff {
    delay: Duration,
}

impl Default for ConstantBackoff {
    fn default() -> Self {
        Self::new(ConstantBackoffOptions::default())
    }
}

impl ConstantBackoff {
    /// A constant backoff with these options.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its options by value like every other options parameter"
    )]
    pub fn new(options: ConstantBackoffOptions) -> Self {
        Self { delay: options.delay }
    }
}

impl BackoffStrategy for ConstantBackoff {
    fn next_delay(&self, _context: &BackoffContext) -> Duration {
        self.delay
    }
}

/// Options for [`LinearBackoff`] (TS `LinearBackoffOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct LinearBackoffOptions {
    /// The delay grows as `base * attempt` (TS `baseMs`). Default 1 s.
    #[builder(default = DEFAULT_DELAY)]
    pub base: Duration,
    /// The upper bound applied before jitter (TS `maxMs`). Default 30 s.
    #[builder(default = DEFAULT_MAX)]
    pub max: Duration,
    /// The jitter mode. Default [`JitterKind::Full`].
    #[builder(default)]
    pub jitter: JitterKind,
}

impl Default for LinearBackoffOptions {
    fn default() -> Self {
        Self {
            base: DEFAULT_DELAY,
            max: DEFAULT_MAX,
            jitter: JitterKind::Full,
        }
    }
}

/// A delay that grows as `base * attempt`, capped at `max`, then jittered (TS `LinearBackoff`).
#[derive(Clone)]
pub struct LinearBackoff {
    options: LinearBackoffOptions,
    random: Random,
}

impl LinearBackoff {
    /// A linear backoff with these options.
    pub fn new(options: LinearBackoffOptions) -> Self {
        Self {
            options,
            random: default_random(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_random(mut self, random: Random) -> Self {
        self.random = random;
        self
    }
}

impl Default for LinearBackoff {
    fn default() -> Self {
        Self::new(LinearBackoffOptions::default())
    }
}

impl fmt::Debug for LinearBackoff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinearBackoff")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl BackoffStrategy for LinearBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        let LinearBackoffOptions { base, max, jitter } = self.options;
        let raw = max.min(base.saturating_mul(context.attempt.get()));
        apply_jitter(raw, jitter, base, max, context.last_delay, &self.random)
    }
}

/// Options for [`ExponentialBackoff`] (TS `ExponentialBackoffOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, PartialEq, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct ExponentialBackoffOptions {
    /// The delay grows as `base * multiplier^(attempt - 1)` (TS `baseMs`). Default 1 s.
    #[builder(default = DEFAULT_DELAY)]
    pub base: Duration,
    /// The upper bound applied before jitter (TS `maxMs`). Default 30 s.
    #[builder(default = DEFAULT_MAX)]
    pub max: Duration,
    /// The growth factor per attempt. Default 2.
    #[builder(default = 2.0)]
    pub multiplier: f64,
    /// The jitter mode. Default [`JitterKind::Full`].
    #[builder(default)]
    pub jitter: JitterKind,
}

impl Default for ExponentialBackoffOptions {
    fn default() -> Self {
        Self {
            base: DEFAULT_DELAY,
            max: DEFAULT_MAX,
            multiplier: 2.0,
            jitter: JitterKind::Full,
        }
    }
}

/// A delay that grows as `base * multiplier^(attempt - 1)`, capped at `max`, then jittered
/// (TS `ExponentialBackoff`).
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// use strands::retry::{BackoffContext, BackoffStrategy, ExponentialBackoff, ExponentialBackoffOptions, JitterKind};
///
/// let backoff = ExponentialBackoff::new(ExponentialBackoffOptions {
///     base: Duration::from_millis(100),
///     jitter: JitterKind::None,
///     ..Default::default()
/// });
/// let third = BackoffContext::new(NonZeroU32::new(3).ok_or("zero")?);
/// assert_eq!(backoff.next_delay(&third), Duration::from_millis(400));
/// # Ok::<(), &str>(())
/// ```
#[derive(Clone)]
pub struct ExponentialBackoff {
    options: ExponentialBackoffOptions,
    random: Random,
}

impl ExponentialBackoff {
    /// An exponential backoff with these options.
    pub fn new(options: ExponentialBackoffOptions) -> Self {
        Self {
            options,
            random: default_random(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_random(mut self, random: Random) -> Self {
        self.random = random;
        self
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new(ExponentialBackoffOptions::default())
    }
}

impl fmt::Debug for ExponentialBackoff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExponentialBackoff")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl BackoffStrategy for ExponentialBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        let ExponentialBackoffOptions {
            base,
            max,
            multiplier,
            jitter,
        } = self.options;
        let exponent = i32::try_from(context.attempt.get() - 1).unwrap_or(i32::MAX);
        let raw = base.as_secs_f64() * multiplier.powi(exponent);
        let raw = match Duration::try_from_secs_f64(raw) {
            Ok(raw) => raw.min(max),
            Err(_) if raw > 0.0 => max,
            Err(_) => Duration::ZERO,
        };
        apply_jitter(raw, jitter, base, max, context.last_delay, &self.random)
    }
}

/// `delay * factor` for a factor in `[0, 1]`.
fn scale(delay: Duration, factor: f64) -> Duration {
    let factor = if factor.is_finite() {
        factor.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Duration::try_from_secs_f64(delay.as_secs_f64() * factor)
        .unwrap_or(delay)
        .min(delay)
}

fn apply_jitter(
    raw: Duration,
    kind: JitterKind,
    base: Duration,
    max: Duration,
    last_delay: Option<Duration>,
    random: &Random,
) -> Duration {
    match (kind, last_delay) {
        (JitterKind::None, _) => raw,
        (JitterKind::Full, _) | (JitterKind::Decorrelated, None) => scale(raw, random()),
        (JitterKind::Equal, _) => (raw / 2).saturating_add(scale(raw / 2, random())),
        (JitterKind::Decorrelated, Some(last_delay)) => {
            // The outer max() keeps the range from inverting when max < base.
            let upper = base.max(max.min(last_delay.saturating_mul(3)));
            base.saturating_add(scale(upper.saturating_sub(base), random()))
        }
    }
}

#[cfg(test)]
mod tests;
