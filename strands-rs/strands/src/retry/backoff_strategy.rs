//! Backoff strategies for retries (TS `retry/backoff-strategy.ts`).

use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// What a [`BackoffStrategy`] sees when asked for the next delay (TS `BackoffContext`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackoffContext {
    /// The attempt that just failed, starting at 1.
    pub attempt: NonZeroU32,
    /// Time since the first failure of this run of retries.
    pub elapsed: Duration,
    /// The delay returned for the previous attempt, if any (decorrelated jitter uses it).
    pub last_delay: Option<Duration>,
}

impl BackoffContext {
    /// A context for `attempt` with no elapsed time and no previous delay.
    pub fn new(attempt: NonZeroU32) -> Self {
        Self {
            attempt,
            elapsed: Duration::ZERO,
            last_delay: None,
        }
    }

    /// Sets the time since the first failure.
    #[must_use]
    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    /// Sets the delay returned for the previous attempt.
    #[must_use]
    pub fn with_last_delay(mut self, last_delay: Duration) -> Self {
        self.last_delay = Some(last_delay);
        self
    }
}

/// Computes how long to wait before the next attempt (TS `BackoffStrategy`).
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// use strands::retry::{BackoffContext, BackoffStrategy};
///
/// /// Waits one more second per attempt, without jitter.
/// struct Stepped;
///
/// impl BackoffStrategy for Stepped {
///     fn next_delay(&self, context: &BackoffContext) -> Duration {
///         Duration::from_secs(context.attempt.get().into())
///     }
/// }
///
/// let context = BackoffContext::new(NonZeroU32::new(3).unwrap());
/// assert_eq!(Stepped.next_delay(&context), Duration::from_secs(3));
/// ```
pub trait BackoffStrategy: Send + Sync + 'static {
    /// The delay before retrying after `context.attempt` failed.
    fn next_delay(&self, context: &BackoffContext) -> Duration;
}

/// How a computed delay is randomized (TS `JitterKind`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JitterKind {
    /// The computed delay, unchanged.
    None,
    /// Uniform between zero and the computed delay.
    #[default]
    Full,
    /// Half the computed delay plus up to the other half.
    Equal,
    /// Uniform between the base delay and three times the previous delay, capped at the maximum.
    Decorrelated,
}

/// A source of uniform random numbers in `[0, 1)`; injectable so tests are deterministic.
#[derive(Clone)]
pub(crate) struct Random(Arc<dyn Fn() -> f64 + Send + Sync>);

impl Random {
    pub(crate) fn new(f: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    fn next(&self) -> f64 {
        (self.0)()
    }
}

impl Default for Random {
    fn default() -> Self {
        Self::new(fastrand::f64)
    }
}

impl fmt::Debug for Random {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Random")
    }
}

const DEFAULT_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_MAX: Duration = Duration::from_secs(30);
const DEFAULT_MULTIPLIER: f64 = 2.0;

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn from_millis(ms: f64) -> Duration {
    Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).unwrap_or(Duration::MAX)
}

fn jitter(
    raw: f64,
    kind: JitterKind,
    base: f64,
    max: f64,
    last_delay: Option<Duration>,
    random: &Random,
) -> f64 {
    match (kind, last_delay) {
        (JitterKind::None, _) => raw,
        (JitterKind::Full, _) | (JitterKind::Decorrelated, None) => random.next() * raw,
        (JitterKind::Equal, _) => raw / 2.0 + random.next() * (raw / 2.0),
        (JitterKind::Decorrelated, Some(last)) => {
            let upper = base.max(max.min(millis(last) * 3.0));
            base + random.next() * (upper - base)
        }
    }
}

/// Options for [`ConstantBackoff`] (TS `ConstantBackoffOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, bon::Builder)]
pub struct ConstantBackoffOptions {
    /// The delay between attempts. Defaults to 1 s.
    #[builder(default = DEFAULT_DELAY)]
    pub delay: Duration,
}

impl Default for ConstantBackoffOptions {
    fn default() -> Self {
        Self { delay: DEFAULT_DELAY }
    }
}

/// The same delay before every retry (TS `ConstantBackoff`).
#[derive(Debug, Clone, Copy)]
pub struct ConstantBackoff {
    delay: Duration,
}

impl Default for ConstantBackoff {
    fn default() -> Self {
        Self::new(ConstantBackoffOptions::default())
    }
}

impl ConstantBackoff {
    /// A constant backoff.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, bon::Builder)]
pub struct LinearBackoffOptions {
    /// Delay added per attempt. Defaults to 1 s.
    #[builder(default = DEFAULT_DELAY)]
    pub base: Duration,
    /// Cap applied before jitter. Defaults to 30 s.
    #[builder(default = DEFAULT_MAX)]
    pub max: Duration,
    /// Randomization. Defaults to [`JitterKind::Full`].
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

/// A delay of `base * attempt`, capped at `max`, then jittered (TS `LinearBackoff`).
#[derive(Debug, Clone, Default)]
pub struct LinearBackoff {
    options: LinearBackoffOptions,
    random: Random,
}

impl LinearBackoff {
    /// A linear backoff.
    pub fn new(options: LinearBackoffOptions) -> Self {
        Self {
            options,
            random: Random::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_random(mut self, random: Random) -> Self {
        self.random = random;
        self
    }
}

impl BackoffStrategy for LinearBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        let (base, max) = (millis(self.options.base), millis(self.options.max));
        let raw = max.min(base * f64::from(context.attempt.get()));
        from_millis(jitter(
            raw,
            self.options.jitter,
            base,
            max,
            context.last_delay,
            &self.random,
        ))
    }
}

/// Options for [`ExponentialBackoff`] (TS `ExponentialBackoffOptions`).
#[derive(Debug, Clone, Copy, PartialEq, bon::Builder)]
pub struct ExponentialBackoffOptions {
    /// Delay of the first retry. Defaults to 1 s.
    #[builder(default = DEFAULT_DELAY)]
    pub base: Duration,
    /// Cap applied before jitter. Defaults to 30 s.
    #[builder(default = DEFAULT_MAX)]
    pub max: Duration,
    /// Growth factor per attempt. Defaults to 2.
    #[builder(default = DEFAULT_MULTIPLIER)]
    pub multiplier: f64,
    /// Randomization. Defaults to [`JitterKind::Full`].
    #[builder(default)]
    pub jitter: JitterKind,
}

impl Default for ExponentialBackoffOptions {
    fn default() -> Self {
        Self {
            base: DEFAULT_DELAY,
            max: DEFAULT_MAX,
            multiplier: DEFAULT_MULTIPLIER,
            jitter: JitterKind::Full,
        }
    }
}

/// A delay of `base * multiplier^(attempt - 1)`, capped at `max`, then jittered (TS
/// `ExponentialBackoff`).
#[derive(Debug, Clone, Default)]
pub struct ExponentialBackoff {
    options: ExponentialBackoffOptions,
    random: Random,
}

impl ExponentialBackoff {
    /// An exponential backoff.
    pub fn new(options: ExponentialBackoffOptions) -> Self {
        Self {
            options,
            random: Random::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_random(mut self, random: Random) -> Self {
        self.random = random;
        self
    }
}

impl BackoffStrategy for ExponentialBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        let (base, max) = (millis(self.options.base), millis(self.options.max));
        let exponent = f64::from(context.attempt.get() - 1);
        let raw = max.min(base * self.options.multiplier.powf(exponent));
        from_millis(jitter(
            raw,
            self.options.jitter,
            base,
            max,
            context.last_delay,
            &self.random,
        ))
    }
}

#[cfg(test)]
mod tests;
