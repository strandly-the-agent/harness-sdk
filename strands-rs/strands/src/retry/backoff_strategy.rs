//! Backoff strategies (TS `retry/backoff-strategy.ts`): the delay before the next retry attempt.
//!
//! Policy (whether to retry, max attempts) lives in the retry strategies, not here.

use std::fmt;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const DEFAULT_BASE: Duration = Duration::from_secs(1);
const DEFAULT_MAX: Duration = Duration::from_secs(30);
const DEFAULT_MULTIPLIER: f64 = 2.0;
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const TWO_POW_53: f64 = 9_007_199_254_740_992.0;

/// Context passed to a [`BackoffStrategy`] for each retry decision.
///
/// An additive-only contract: fields may be added, never removed or repurposed.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct BackoffContext {
    /// 1-based index of the attempt that just failed.
    pub attempt: NonZeroU32,
    /// Time elapsed since the first failure of the current retry budget.
    pub elapsed: Duration,
    /// The previously computed delay, if any. `None` before the first retry.
    pub last_delay: Option<Duration>,
}

impl BackoffContext {
    /// Creates a context for `attempt` with no elapsed time and no previous delay.
    #[must_use]
    pub fn new(attempt: NonZeroU32) -> Self {
        Self {
            attempt,
            elapsed: Duration::ZERO,
            last_delay: None,
        }
    }

    /// Sets [`elapsed`](Self::elapsed).
    #[must_use]
    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    /// Sets [`last_delay`](Self::last_delay).
    #[must_use]
    pub fn with_last_delay(mut self, last_delay: Duration) -> Self {
        self.last_delay = Some(last_delay);
        self
    }
}

/// Computes the delay before the next retry attempt.
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// use strands::retry::{BackoffContext, BackoffStrategy};
///
/// /// Waits 100 ms more per attempt, never more than 1 s.
/// struct Stepped;
///
/// impl BackoffStrategy for Stepped {
///     fn next_delay(&self, context: &BackoffContext) -> Duration {
///         Duration::from_millis(100 * u64::from(context.attempt.get())).min(Duration::from_secs(1))
///     }
/// }
///
/// let context = BackoffContext::new(NonZeroU32::new(3).unwrap());
/// assert_eq!(Stepped.next_delay(&context), Duration::from_millis(300));
/// ```
pub trait BackoffStrategy: Send + Sync + 'static {
    /// Returns the delay before the next attempt.
    fn next_delay(&self, context: &BackoffContext) -> Duration;
}

/// Supported jitter modes.
///
/// For jitter outside these modes, implement [`BackoffStrategy`] directly.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JitterKind {
    /// The raw delay, unchanged.
    None,
    /// Uniform random in `[0, raw]`.
    #[default]
    Full,
    /// `raw / 2 + uniform(0, raw / 2)`: half fixed, half random.
    Equal,
    /// `uniform(base, last_delay * 3)`, capped at `max`; full jitter on the first retry, when there is
    /// no last delay.
    Decorrelated,
}

/// Source of uniform samples in `[0, 1)` for jitter (TS `Math.random()`).
///
/// The default is a generator seeded from process entropy. Tests pass [`RandomSource::seeded`] or a
/// fixed closure so delays are deterministic.
#[derive(Clone)]
pub struct RandomSource(Arc<dyn Fn() -> f64 + Send + Sync>);

impl RandomSource {
    /// Samples from `sample`. Values outside `[0, 1]` are clamped, and a non-finite value counts as 0.
    pub fn new(sample: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(sample))
    }

    /// A deterministic generator (`SplitMix64`) that yields the same sequence for the same seed.
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        let state = AtomicU64::new(seed);
        Self::new(move || {
            let mut z = state
                .fetch_add(SPLITMIX_GAMMA, Ordering::Relaxed)
                .wrapping_add(SPLITMIX_GAMMA);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            (z >> 11) as f64 / TWO_POW_53
        })
    }

    fn sample(&self) -> f64 {
        let value = (self.0)();
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

impl Default for RandomSource {
    fn default() -> Self {
        Self::seeded(RandomState::new().build_hasher().finish())
    }
}

impl fmt::Debug for RandomSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RandomSource(..)")
    }
}

/// Options for [`ConstantBackoff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, bon::Builder)]
pub struct ConstantBackoffOptions {
    /// Delay returned for every retry. Default 1 s.
    #[builder(default = DEFAULT_BASE)]
    pub delay: Duration,
}

impl Default for ConstantBackoffOptions {
    fn default() -> Self {
        Self { delay: DEFAULT_BASE }
    }
}

/// Constant backoff: the same delay for every retry.
#[derive(Debug, Clone)]
pub struct ConstantBackoff {
    delay: Duration,
}

impl Default for ConstantBackoff {
    fn default() -> Self {
        Self::new(ConstantBackoffOptions::default())
    }
}

impl ConstantBackoff {
    /// Creates a constant backoff.
    #[must_use]
    pub fn new(options: ConstantBackoffOptions) -> Self {
        Self { delay: options.delay }
    }
}

impl BackoffStrategy for ConstantBackoff {
    fn next_delay(&self, _context: &BackoffContext) -> Duration {
        self.delay
    }
}

/// Options for [`LinearBackoff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, bon::Builder)]
pub struct LinearBackoffOptions {
    /// Base delay; the delay grows as `base * attempt`. Default 1 s.
    #[builder(default = DEFAULT_BASE)]
    pub base: Duration,
    /// Upper bound applied before jitter. Default 30 s.
    #[builder(default = DEFAULT_MAX)]
    pub max: Duration,
    /// Jitter mode. Default [`JitterKind::Full`].
    #[builder(default)]
    pub jitter: JitterKind,
}

impl Default for LinearBackoffOptions {
    fn default() -> Self {
        Self {
            base: DEFAULT_BASE,
            max: DEFAULT_MAX,
            jitter: JitterKind::Full,
        }
    }
}

/// Linear backoff: the delay grows as `base * attempt`, capped at `max`, then jittered.
#[derive(Debug, Clone, Default)]
pub struct LinearBackoff {
    options: LinearBackoffOptions,
    random: RandomSource,
}

impl LinearBackoff {
    /// Creates a linear backoff.
    #[must_use]
    pub fn new(options: LinearBackoffOptions) -> Self {
        Self {
            options,
            random: RandomSource::default(),
        }
    }

    /// Replaces the jitter random source.
    #[must_use]
    pub fn with_random(mut self, random: RandomSource) -> Self {
        self.random = random;
        self
    }
}

impl BackoffStrategy for LinearBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        let LinearBackoffOptions { base, max, jitter } = self.options;
        let raw = base
            .checked_mul(context.attempt.get())
            .map_or(max, |delay| delay.min(max));
        apply_jitter(raw, jitter, base, max, context.last_delay, &self.random)
    }
}

/// Options for [`ExponentialBackoff`].
#[derive(Debug, Clone, Copy, PartialEq, bon::Builder)]
pub struct ExponentialBackoffOptions {
    /// Base delay; the delay grows as `base * multiplier^(attempt - 1)`. Default 1 s.
    #[builder(default = DEFAULT_BASE)]
    pub base: Duration,
    /// Upper bound applied before jitter. Default 30 s.
    #[builder(default = DEFAULT_MAX)]
    pub max: Duration,
    /// Growth factor per attempt. Default 2.
    #[builder(default = DEFAULT_MULTIPLIER)]
    pub multiplier: f64,
    /// Jitter mode. Default [`JitterKind::Full`].
    #[builder(default)]
    pub jitter: JitterKind,
}

impl Default for ExponentialBackoffOptions {
    fn default() -> Self {
        Self {
            base: DEFAULT_BASE,
            max: DEFAULT_MAX,
            multiplier: DEFAULT_MULTIPLIER,
            jitter: JitterKind::Full,
        }
    }
}

/// Exponential backoff: the delay grows as `base * multiplier^(attempt - 1)`, capped at `max`, then
/// jittered.
///
/// Seed the jitter with [`RandomSource::seeded`] for repeatable delays:
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// use strands::retry::{BackoffContext, BackoffStrategy, ExponentialBackoff, ExponentialBackoffOptions, RandomSource};
///
/// let backoff = |seed| {
///     ExponentialBackoff::new(ExponentialBackoffOptions::builder().base(Duration::from_millis(500)).build())
///         .with_random(RandomSource::seeded(seed))
/// };
/// let context = BackoffContext::new(NonZeroU32::new(3).unwrap());
/// let delay = backoff(7).next_delay(&context);
/// assert!(delay <= Duration::from_secs(2), "full jitter stays within 500 ms * 2^2");
/// assert_eq!(backoff(7).next_delay(&context), delay);
/// ```
#[derive(Debug, Clone, Default)]
pub struct ExponentialBackoff {
    options: ExponentialBackoffOptions,
    random: RandomSource,
}

impl ExponentialBackoff {
    /// Creates an exponential backoff.
    #[must_use]
    pub fn new(options: ExponentialBackoffOptions) -> Self {
        Self {
            options,
            random: RandomSource::default(),
        }
    }

    /// Replaces the jitter random source.
    #[must_use]
    pub fn with_random(mut self, random: RandomSource) -> Self {
        self.random = random;
        self
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
        let growth = multiplier.powf(f64::from(context.attempt.get() - 1));
        let raw = nanos_to_duration(base.as_nanos() as f64 * growth, max);
        apply_jitter(raw, jitter, base, max, context.last_delay, &self.random)
    }
}

fn apply_jitter(
    raw: Duration,
    kind: JitterKind,
    base: Duration,
    max: Duration,
    last_delay: Option<Duration>,
    random: &RandomSource,
) -> Duration {
    match (kind, last_delay) {
        (JitterKind::None, _) => raw,
        (JitterKind::Full, _) | (JitterKind::Decorrelated, None) => scale(raw, random.sample()),
        (JitterKind::Equal, _) => {
            let half = raw / 2;
            half.saturating_add(scale(half, random.sample()))
        }
        (JitterKind::Decorrelated, Some(last_delay)) => {
            // The outer max() keeps the range non-inverted when `max` < `base`.
            let upper = base.max(max.min(last_delay.saturating_mul(3)));
            base.saturating_add(scale(upper.saturating_sub(base), random.sample()))
        }
    }
}

/// `delay * factor` for a `factor` in `[0, 1]`.
fn scale(delay: Duration, factor: f64) -> Duration {
    nanos_to_duration(delay.as_nanos() as f64 * factor, delay)
}

/// Rounds `nanos` to a whole nanosecond, capped at `cap`. NaN and negative values are zero, as a JS
/// timer treats them.
fn nanos_to_duration(nanos: f64, cap: Duration) -> Duration {
    if nanos.is_nan() || nanos <= 0.0 {
        return Duration::ZERO;
    }
    if nanos >= cap.as_nanos() as f64 {
        return cap;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "nanos is finite, positive and below the cap; the cast saturates past u64::MAX"
    )]
    Duration::from_nanos(nanos.round() as u64)
}

#[cfg(test)]
mod tests;
