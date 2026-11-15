//! Ported from strands-ts/src/retry/__tests__/backoff-strategy.test.ts

use super::*;

fn ctx(attempt: u32) -> BackoffContext {
    BackoffContext::new(NonZeroU32::new(attempt).unwrap())
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn half() -> Random {
    Arc::new(|| 0.5)
}

fn linear(options: LinearBackoffOptions) -> LinearBackoff {
    LinearBackoff::new(options).with_random(half())
}

fn exponential(options: ExponentialBackoffOptions) -> ExponentialBackoff {
    ExponentialBackoff::new(options).with_random(half())
}

#[test]
fn returns_the_configured_delay_regardless_of_attempt() {
    let backoff = ConstantBackoff::new(ConstantBackoffOptions { delay: ms(250) });
    assert_eq!(backoff.next_delay(&ctx(1)), ms(250));
    assert_eq!(backoff.next_delay(&ctx(5)), ms(250));
}

#[test]
fn defaults_delay_ms_to_1000() {
    assert_eq!(ConstantBackoff::default().next_delay(&ctx(1)), ms(1000));
    assert_eq!(
        ConstantBackoff::new(ConstantBackoffOptions::builder().build()).next_delay(&ctx(1)),
        ms(1000)
    );
}

#[test]
fn grows_as_base_ms_attempt() {
    let backoff = linear(
        LinearBackoffOptions::builder()
            .base(ms(100))
            .jitter(JitterKind::None)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(1)), ms(100));
    assert_eq!(backoff.next_delay(&ctx(2)), ms(200));
    assert_eq!(backoff.next_delay(&ctx(3)), ms(300));
}

#[test]
fn clamps_to_max_ms_before_jitter() {
    let backoff = linear(
        LinearBackoffOptions::builder()
            .base(ms(1000))
            .max(ms(2500))
            .jitter(JitterKind::None)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(10)), ms(2500));
}

#[test]
fn applies_full_jitter_by_default_math_random_raw() {
    let backoff = linear(LinearBackoffOptions::builder().base(ms(100)).build());
    assert_eq!(backoff.next_delay(&ctx(4)), ms(200));
}

#[test]
fn grows_as_base_ms_multiplier_attempt_1() {
    let backoff = exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .jitter(JitterKind::None)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(1)), ms(100));
    assert_eq!(backoff.next_delay(&ctx(2)), ms(200));
    assert_eq!(backoff.next_delay(&ctx(3)), ms(400));
    assert_eq!(backoff.next_delay(&ctx(4)), ms(800));
}

#[test]
fn honors_custom_multiplier() {
    let backoff = exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .multiplier(3.0)
            .jitter(JitterKind::None)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(3)), ms(900));
}

#[test]
fn clamps_to_max_ms() {
    let backoff = exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .max(ms(500))
            .jitter(JitterKind::None)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(10)), ms(500));
    assert_eq!(backoff.next_delay(&ctx(u32::MAX)), ms(500));
}

#[test]
fn applies_full_jitter_by_default() {
    let backoff = exponential(ExponentialBackoffOptions::builder().base(ms(100)).build());
    assert_eq!(backoff.next_delay(&ctx(3)), ms(200));
}

#[test]
fn applies_equal_jitter_as_raw_2_random_raw_2() {
    let backoff = exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .jitter(JitterKind::Equal)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(2)), ms(150));
}

#[test]
fn applies_no_jitter_when_set_to_none() {
    let backoff = exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .jitter(JitterKind::None)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(3)), ms(400));
}

#[test]
fn falls_back_to_full_jitter_for_decorrelated_when_last_delay_ms_missing() {
    let backoff = exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .jitter(JitterKind::Decorrelated)
            .build(),
    );
    assert_eq!(backoff.next_delay(&ctx(3)), ms(200));
}

fn decorrelated(max: Duration) -> ExponentialBackoff {
    exponential(
        ExponentialBackoffOptions::builder()
            .base(ms(100))
            .max(max)
            .jitter(JitterKind::Decorrelated)
            .build(),
    )
}

#[test]
fn applies_decorrelated_jitter_as_uniform_base_ms_min_max_ms_last_delay_ms_3() {
    let context = ctx(2).with_last_delay(ms(200));
    assert_eq!(decorrelated(ms(10_000)).next_delay(&context), ms(350));
}

#[test]
fn caps_decorrelated_upper_at_max_ms() {
    let context = ctx(2).with_last_delay(ms(1000));
    assert_eq!(decorrelated(ms(500)).next_delay(&context), ms(300));
}

#[test]
fn floors_decorrelated_upper_at_base_ms_when_last_delay_3_base_ms() {
    let context = ctx(2).with_last_delay(ms(10));
    assert_eq!(decorrelated(ms(500)).next_delay(&context), ms(100));
}

#[test]
fn out_of_range_random_values_stay_within_the_raw_delay() {
    let backoff = ExponentialBackoff::new(ExponentialBackoffOptions::builder().base(ms(100)).build());
    for value in [-1.0, 2.0, f64::NAN] {
        let delay = backoff
            .clone()
            .with_random(Arc::new(move || value))
            .next_delay(&ctx(1));
        assert!(delay <= ms(100), "{value}: {delay:?}");
    }
}
