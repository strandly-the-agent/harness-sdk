//! Ported from strands-ts/src/retry/__tests__/backoff-strategy.test.ts

use super::*;

fn ctx(attempt: u32) -> BackoffContext {
    BackoffContext::new(NonZeroU32::new(attempt).unwrap())
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn half() -> Random {
    Random::new(|| 0.5)
}

fn linear(base: u64, max: Option<u64>, jitter: JitterKind) -> LinearBackoff {
    let defaults = LinearBackoffOptions::default();
    LinearBackoff::new(LinearBackoffOptions {
        base: ms(base),
        max: max.map_or(defaults.max, ms),
        jitter,
    })
    .with_random(half())
}

fn exponential(base: u64, max: Option<u64>, jitter: JitterKind) -> ExponentialBackoff {
    let defaults = ExponentialBackoffOptions::default();
    ExponentialBackoff::new(ExponentialBackoffOptions {
        base: ms(base),
        max: max.map_or(defaults.max, ms),
        jitter,
        ..defaults
    })
    .with_random(half())
}

#[test]
fn returns_the_configured_delay_regardless_of_attempt() {
    let backoff = ConstantBackoff::new(ConstantBackoffOptions { delay: ms(250) });
    assert_eq!(backoff.next_delay(&ctx(1)), ms(250));
    assert_eq!(backoff.next_delay(&ctx(5)), ms(250));
}

#[test]
fn defaults_delay_ms_to_1000() {
    assert_eq!(
        ConstantBackoff::new(ConstantBackoffOptions::default()).next_delay(&ctx(1)),
        ms(1000)
    );
    assert_eq!(ConstantBackoff::default().next_delay(&ctx(1)), ms(1000));
}

#[test]
fn builders_without_fields_match_the_defaults() {
    assert_eq!(
        ConstantBackoffOptions::builder().build(),
        ConstantBackoffOptions::default()
    );
    assert_eq!(
        LinearBackoffOptions::builder().build(),
        LinearBackoffOptions::default()
    );
    assert_eq!(
        ExponentialBackoffOptions::builder().build(),
        ExponentialBackoffOptions::default()
    );
}

#[test]
fn grows_as_base_ms_attempt() {
    let backoff = linear(100, None, JitterKind::None);
    assert_eq!(backoff.next_delay(&ctx(1)), ms(100));
    assert_eq!(backoff.next_delay(&ctx(2)), ms(200));
    assert_eq!(backoff.next_delay(&ctx(3)), ms(300));
}

#[test]
fn clamps_to_max_ms_before_jitter() {
    assert_eq!(
        linear(1000, Some(2500), JitterKind::None).next_delay(&ctx(10)),
        ms(2500)
    );
}

#[test]
fn applies_full_jitter_by_default_math_random_raw() {
    let backoff = LinearBackoff::new(LinearBackoffOptions {
        base: ms(100),
        ..Default::default()
    })
    .with_random(half());
    assert_eq!(backoff.next_delay(&ctx(4)), ms(200));
}

#[test]
fn grows_as_base_ms_multiplier_attempt_1() {
    let backoff = exponential(100, None, JitterKind::None);
    let delays: Vec<Duration> = (1..=4).map(|attempt| backoff.next_delay(&ctx(attempt))).collect();
    assert_eq!(delays, [ms(100), ms(200), ms(400), ms(800)]);
}

#[test]
fn honors_custom_multiplier() {
    let backoff = ExponentialBackoff::new(ExponentialBackoffOptions {
        base: ms(100),
        multiplier: 3.0,
        jitter: JitterKind::None,
        ..Default::default()
    });
    assert_eq!(backoff.next_delay(&ctx(3)), ms(900));
}

#[test]
fn clamps_to_max_ms() {
    assert_eq!(
        exponential(100, Some(500), JitterKind::None).next_delay(&ctx(10)),
        ms(500)
    );
}

#[test]
fn applies_full_jitter_by_default() {
    let backoff = ExponentialBackoff::new(ExponentialBackoffOptions {
        base: ms(100),
        ..Default::default()
    })
    .with_random(half());
    assert_eq!(backoff.next_delay(&ctx(3)), ms(200));
}

#[test]
fn applies_equal_jitter_as_raw_2_random_raw_2() {
    assert_eq!(
        exponential(100, None, JitterKind::Equal).next_delay(&ctx(2)),
        ms(150)
    );
}

#[test]
fn applies_no_jitter_when_set_to_none() {
    assert_eq!(
        exponential(100, None, JitterKind::None).next_delay(&ctx(3)),
        ms(400)
    );
}

#[test]
fn falls_back_to_full_jitter_for_decorrelated_when_last_delay_ms_missing() {
    assert_eq!(
        exponential(100, None, JitterKind::Decorrelated).next_delay(&ctx(3)),
        ms(200)
    );
}

fn with_last(attempt: u32, last: u64) -> BackoffContext {
    ctx(attempt).with_last_delay(ms(last))
}

#[test]
fn applies_decorrelated_jitter_as_uniform_base_ms_min_max_ms_last_delay_ms_3() {
    let backoff = exponential(100, Some(10_000), JitterKind::Decorrelated);
    assert_eq!(backoff.next_delay(&with_last(2, 200)), ms(350));
}

#[test]
fn caps_decorrelated_upper_at_max_ms() {
    let backoff = exponential(100, Some(500), JitterKind::Decorrelated);
    assert_eq!(backoff.next_delay(&with_last(2, 1000)), ms(300));
}

#[test]
fn floors_decorrelated_upper_at_base_ms_when_last_delay_3_base_ms() {
    let backoff = exponential(100, Some(500), JitterKind::Decorrelated);
    assert_eq!(backoff.next_delay(&with_last(2, 10)), ms(100));
}

#[test]
fn huge_attempts_saturate_at_the_cap() {
    assert_eq!(
        exponential(100, Some(500), JitterKind::None).next_delay(&ctx(u32::MAX)),
        ms(500)
    );
    assert_eq!(
        linear(100, Some(500), JitterKind::None).next_delay(&ctx(u32::MAX)),
        ms(500)
    );
}
