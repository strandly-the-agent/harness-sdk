//! Backoff strategies (`strands-ts/src/retry/backoff-strategy.ts`): pure delay math.
//!
//! A [`BackoffStrategy`] answers "how long before the next attempt". Policy — whether to retry at
//! all, max attempts, time budgets — lives in the retry orchestration layer
//! ([`super::ModelRetryStrategy`]).

use std::sync::Arc;

use rand::Rng;

use crate::error::{Error, Result};

/// Source of uniform `[0, 1)` samples (`Math.random()` in TS).
type RandomFn = Arc<dyn Fn() -> f64 + Send + Sync>;

fn default_random() -> RandomFn {
    Arc::new(|| rand::rng().random::<f64>())
}

/// Context passed to a [`BackoffStrategy`] for each retry decision.
///
/// An open, additive-only contract: fields may be added, never removed or repurposed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BackoffContext {
    /// 1-based index of the attempt that just failed. Must be >= 1.
    pub attempt: u32,
    /// Total milliseconds elapsed since the first attempt started.
    pub elapsed_ms: f64,
    /// Previously computed delay, if any. `None` before the first retry.
    pub last_delay_ms: Option<f64>,
}

impl BackoffContext {
    /// Context for `attempt` with no elapsed time and no previous delay.
    pub fn new(attempt: u32) -> Self {
        Self {
            attempt,
            elapsed_ms: 0.0,
            last_delay_ms: None,
        }
    }

    /// Set [`BackoffContext::elapsed_ms`].
    pub fn with_elapsed_ms(mut self, elapsed_ms: f64) -> Self {
        self.elapsed_ms = elapsed_ms;
        self
    }

    /// Set [`BackoffContext::last_delay_ms`].
    pub fn with_last_delay_ms(mut self, last_delay_ms: f64) -> Self {
        self.last_delay_ms = Some(last_delay_ms);
        self
    }
}

/// Computes the delay in milliseconds before the next retry attempt.
pub trait BackoffStrategy: Send + Sync {
    /// Delay in milliseconds before the next attempt; non-negative and finite.
    ///
    /// Returns an error when `ctx.attempt < 1`, which is a programmer error.
    fn next_delay(&self, ctx: &BackoffContext) -> Result<f64>;
}

/// Jitter modes applied to the raw delay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JitterKind {
    /// Return the raw delay unchanged.
    None,
    /// Uniform in `[0, raw)`.
    #[default]
    Full,
    /// `raw/2 + uniform(0, raw/2)` — half fixed, half random.
    Equal,
    /// `uniform(base_ms, last_delay_ms * 3)` capped at `max_ms`; falls back to
    /// [`JitterKind::Full`] on the first retry, when `last_delay_ms` is absent.
    Decorrelated,
}

fn validate_attempt(attempt: u32, class_name: &str) -> Result<()> {
    if attempt < 1 {
        return Err(Error::other(format!(
            "{class_name}: attempt must be an integer >= 1 (got {attempt})"
        )));
    }
    Ok(())
}

fn jitter(
    raw: f64,
    kind: JitterKind,
    base_ms: f64,
    max_ms: f64,
    last_delay_ms: Option<f64>,
    random: f64,
) -> f64 {
    match kind {
        JitterKind::None => raw,
        JitterKind::Full => random * raw,
        JitterKind::Equal => raw / 2.0 + random * (raw / 2.0),
        JitterKind::Decorrelated => match last_delay_ms {
            None => random * raw,
            Some(last) => {
                // The max() guards the degenerate max_ms < base_ms case, which would otherwise
                // produce an inverted range.
                let upper = base_ms.max(max_ms.min(last * 3.0));
                base_ms + random * (upper - base_ms)
            }
        },
    }
}

/// Options for [`ConstantBackoff`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConstantBackoffOptions {
    /// Delay in ms returned for every retry. Default 1000.
    pub delay_ms: f64,
}

impl Default for ConstantBackoffOptions {
    fn default() -> Self {
        Self { delay_ms: 1000.0 }
    }
}

/// Constant backoff: the same delay for every retry, never jittered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ConstantBackoff {
    options: ConstantBackoffOptions,
}

impl ConstantBackoff {
    /// Strategy with these options (`ConstantBackoffOptions::default()` for the TS defaults).
    pub fn new(options: ConstantBackoffOptions) -> Self {
        Self { options }
    }
}

impl BackoffStrategy for ConstantBackoff {
    fn next_delay(&self, ctx: &BackoffContext) -> Result<f64> {
        validate_attempt(ctx.attempt, "ConstantBackoff")?;
        Ok(self.options.delay_ms)
    }
}

/// Options for [`LinearBackoff`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearBackoffOptions {
    /// Base delay in ms; the delay grows as `base_ms * attempt`. Default 1000.
    pub base_ms: f64,
    /// Upper bound applied before jitter. Default 30_000.
    pub max_ms: f64,
    /// Jitter mode. Default [`JitterKind::Full`].
    pub jitter: JitterKind,
}

impl Default for LinearBackoffOptions {
    fn default() -> Self {
        Self {
            base_ms: 1000.0,
            max_ms: 30_000.0,
            jitter: JitterKind::Full,
        }
    }
}

/// Linear backoff: `base_ms * attempt`, capped at `max_ms`, then jittered.
#[derive(Clone)]
pub struct LinearBackoff {
    options: LinearBackoffOptions,
    random: RandomFn,
}

impl LinearBackoff {
    /// Strategy with these options (`LinearBackoffOptions::default()` for the TS defaults).
    pub fn new(options: LinearBackoffOptions) -> Self {
        Self {
            options,
            random: default_random(),
        }
    }

    #[cfg(test)]
    fn with_random(options: LinearBackoffOptions, random: RandomFn) -> Self {
        Self { options, random }
    }
}

impl Default for LinearBackoff {
    fn default() -> Self {
        Self::new(LinearBackoffOptions::default())
    }
}

impl std::fmt::Debug for LinearBackoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearBackoff")
            .field("options", &self.options)
            .finish()
    }
}

impl BackoffStrategy for LinearBackoff {
    fn next_delay(&self, ctx: &BackoffContext) -> Result<f64> {
        validate_attempt(ctx.attempt, "LinearBackoff")?;
        let o = &self.options;
        let raw = o.max_ms.min(o.base_ms * f64::from(ctx.attempt));
        Ok(jitter(
            raw,
            o.jitter,
            o.base_ms,
            o.max_ms,
            ctx.last_delay_ms,
            (self.random)(),
        ))
    }
}

/// Options for [`ExponentialBackoff`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExponentialBackoffOptions {
    /// Base delay in ms; the delay grows as `base_ms * multiplier^(attempt-1)`. Default 1000.
    pub base_ms: f64,
    /// Upper bound applied before jitter. Default 30_000.
    pub max_ms: f64,
    /// Growth factor per attempt. Default 2.
    pub multiplier: f64,
    /// Jitter mode. Default [`JitterKind::Full`].
    pub jitter: JitterKind,
}

impl Default for ExponentialBackoffOptions {
    fn default() -> Self {
        Self {
            base_ms: 1000.0,
            max_ms: 30_000.0,
            multiplier: 2.0,
            jitter: JitterKind::Full,
        }
    }
}

/// Exponential backoff: `base_ms * multiplier^(attempt-1)`, capped at `max_ms`, then jittered.
#[derive(Clone)]
pub struct ExponentialBackoff {
    options: ExponentialBackoffOptions,
    random: RandomFn,
}

impl ExponentialBackoff {
    /// Strategy with these options (`ExponentialBackoffOptions::default()` for the TS defaults).
    pub fn new(options: ExponentialBackoffOptions) -> Self {
        Self {
            options,
            random: default_random(),
        }
    }

    #[cfg(test)]
    fn with_random(options: ExponentialBackoffOptions, random: RandomFn) -> Self {
        Self { options, random }
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new(ExponentialBackoffOptions::default())
    }
}

impl std::fmt::Debug for ExponentialBackoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExponentialBackoff")
            .field("options", &self.options)
            .finish()
    }
}

impl BackoffStrategy for ExponentialBackoff {
    fn next_delay(&self, ctx: &BackoffContext) -> Result<f64> {
        validate_attempt(ctx.attempt, "ExponentialBackoff")?;
        let o = &self.options;
        let raw = o
            .max_ms
            .min(o.base_ms * o.multiplier.powi(ctx.attempt as i32 - 1));
        Ok(jitter(
            raw,
            o.jitter,
            o.base_ms,
            o.max_ms,
            ctx.last_delay_ms,
            (self.random)(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Math.random()` mocked to 0.5, as the TS tests do.
    fn half() -> RandomFn {
        Arc::new(|| 0.5)
    }

    fn ctx(attempt: u32) -> BackoffContext {
        BackoffContext::new(attempt)
    }

    #[test]
    fn returns_the_configured_delay_regardless_of_attempt() {
        let bo = ConstantBackoff::new(ConstantBackoffOptions { delay_ms: 250.0 });
        assert_eq!(bo.next_delay(&ctx(1)).unwrap(), 250.0);
        assert_eq!(bo.next_delay(&ctx(5)).unwrap(), 250.0);
    }

    #[test]
    fn defaults_delay_ms_to_1000() {
        assert_eq!(ConstantBackoff::default().next_delay(&ctx(1)).unwrap(), 1000.0);
    }

    #[test]
    fn constant_rejects_attempts_below_1() {
        let err = ConstantBackoff::default().next_delay(&ctx(0)).unwrap_err();
        assert!(err.to_string().contains("attempt must be an integer"), "{err}");
    }

    #[test]
    fn grows_as_base_ms_times_attempt() {
        let bo = LinearBackoff::with_random(
            LinearBackoffOptions {
                base_ms: 100.0,
                jitter: JitterKind::None,
                ..Default::default()
            },
            half(),
        );
        assert_eq!(bo.next_delay(&ctx(1)).unwrap(), 100.0);
        assert_eq!(bo.next_delay(&ctx(2)).unwrap(), 200.0);
        assert_eq!(bo.next_delay(&ctx(3)).unwrap(), 300.0);
    }

    #[test]
    fn clamps_to_max_ms_before_jitter() {
        let bo = LinearBackoff::with_random(
            LinearBackoffOptions {
                base_ms: 1000.0,
                max_ms: 2500.0,
                jitter: JitterKind::None,
            },
            half(),
        );
        assert_eq!(bo.next_delay(&ctx(10)).unwrap(), 2500.0);
    }

    #[test]
    fn applies_full_jitter_by_default_linear() {
        let bo = LinearBackoff::with_random(
            LinearBackoffOptions {
                base_ms: 100.0,
                ..Default::default()
            },
            half(),
        );
        assert_eq!(bo.next_delay(&ctx(4)).unwrap(), 200.0);
    }

    #[test]
    fn linear_rejects_attempts_below_1() {
        let err = LinearBackoff::default().next_delay(&ctx(0)).unwrap_err();
        assert!(err.to_string().contains("attempt must be an integer"), "{err}");
    }

    fn expo(options: ExponentialBackoffOptions) -> ExponentialBackoff {
        ExponentialBackoff::with_random(options, half())
    }

    #[test]
    fn grows_as_base_ms_times_multiplier_pow_attempt_minus_1() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            jitter: JitterKind::None,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(1)).unwrap(), 100.0);
        assert_eq!(bo.next_delay(&ctx(2)).unwrap(), 200.0);
        assert_eq!(bo.next_delay(&ctx(3)).unwrap(), 400.0);
        assert_eq!(bo.next_delay(&ctx(4)).unwrap(), 800.0);
    }

    #[test]
    fn honors_custom_multiplier() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            multiplier: 3.0,
            jitter: JitterKind::None,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(3)).unwrap(), 900.0);
    }

    #[test]
    fn clamps_to_max_ms() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            max_ms: 500.0,
            jitter: JitterKind::None,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(10)).unwrap(), 500.0);
    }

    #[test]
    fn applies_full_jitter_by_default() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(3)).unwrap(), 200.0);
    }

    #[test]
    fn applies_equal_jitter_as_raw_half_plus_random_times_raw_half() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            jitter: JitterKind::Equal,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(2)).unwrap(), 150.0);
    }

    #[test]
    fn applies_no_jitter_when_set_to_none() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            jitter: JitterKind::None,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(3)).unwrap(), 400.0);
    }

    #[test]
    fn falls_back_to_full_jitter_for_decorrelated_when_last_delay_ms_missing() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            jitter: JitterKind::Decorrelated,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(3)).unwrap(), 200.0);
    }

    #[test]
    fn applies_decorrelated_jitter_as_uniform_base_to_min_max_last_times_3() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            max_ms: 10_000.0,
            jitter: JitterKind::Decorrelated,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(2).with_last_delay_ms(200.0)).unwrap(), 350.0);
    }

    #[test]
    fn caps_decorrelated_upper_at_max_ms() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            max_ms: 500.0,
            jitter: JitterKind::Decorrelated,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(2).with_last_delay_ms(1000.0)).unwrap(), 300.0);
    }

    #[test]
    fn floors_decorrelated_upper_at_base_ms_when_last_delay_times_3_below_base_ms() {
        let bo = expo(ExponentialBackoffOptions {
            base_ms: 100.0,
            max_ms: 500.0,
            jitter: JitterKind::Decorrelated,
            ..Default::default()
        });
        assert_eq!(bo.next_delay(&ctx(2).with_last_delay_ms(10.0)).unwrap(), 100.0);
    }

    #[test]
    fn exponential_rejects_attempts_below_1() {
        let err = ExponentialBackoff::default().next_delay(&ctx(0)).unwrap_err();
        assert!(err.to_string().contains("attempt must be an integer"), "{err}");
    }

    #[test]
    fn default_random_is_uniform_in_unit_range() {
        let random = default_random();
        for _ in 0..100 {
            let r = random();
            assert!((0.0..1.0).contains(&r), "{r}");
        }
    }
}
