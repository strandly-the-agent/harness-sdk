//! Ported from strands-ts/src/retry/__tests__/backoff-strategy.test.ts

use super::*;

fn ctx(attempt: u32) -> BackoffContext {
    BackoffContext::new(NonZeroU32::new(attempt).expect("attempt >= 1"))
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn half() -> RandomSource {
    RandomSource::new(|| 0.5)
}

mod constant_backoff {
    use super::*;

    #[test]
    fn returns_the_configured_delay_regardless_of_attempt() {
        let backoff = ConstantBackoff::new(ConstantBackoffOptions { delay: ms(250) });
        assert_eq!(backoff.next_delay(&ctx(1)), ms(250));
        assert_eq!(backoff.next_delay(&ctx(5)), ms(250));
    }

    #[test]
    fn defaults_delay_ms_to_1000() {
        assert_eq!(ConstantBackoff::default().next_delay(&ctx(1)), ms(1000));
    }
}

mod linear_backoff {
    use super::*;

    fn linear(base: u64, max: u64, jitter: JitterKind) -> LinearBackoff {
        LinearBackoff::new(LinearBackoffOptions {
            base: ms(base),
            max: ms(max),
            jitter,
        })
        .with_random(half())
    }

    #[test]
    fn grows_as_base_ms_attempt() {
        let backoff = linear(100, 30_000, JitterKind::None);
        assert_eq!(backoff.next_delay(&ctx(1)), ms(100));
        assert_eq!(backoff.next_delay(&ctx(2)), ms(200));
        assert_eq!(backoff.next_delay(&ctx(3)), ms(300));
    }

    #[test]
    fn clamps_to_max_ms_before_jitter() {
        assert_eq!(
            linear(1000, 2500, JitterKind::None).next_delay(&ctx(10)),
            ms(2500)
        );
    }

    #[test]
    fn applies_full_jitter_by_default_math_random_raw() {
        let backoff =
            LinearBackoff::new(LinearBackoffOptions::builder().base(ms(100)).build()).with_random(half());
        assert_eq!(backoff.next_delay(&ctx(4)), ms(200));
    }
}

mod exponential_backoff {
    use super::*;

    fn exponential(base: u64, max: u64, jitter: JitterKind) -> ExponentialBackoff {
        ExponentialBackoff::new(ExponentialBackoffOptions {
            base: ms(base),
            max: ms(max),
            jitter,
            ..ExponentialBackoffOptions::default()
        })
        .with_random(half())
    }

    #[test]
    fn grows_as_base_ms_multiplier_attempt_1() {
        let backoff = exponential(100, 30_000, JitterKind::None);
        assert_eq!(backoff.next_delay(&ctx(1)), ms(100));
        assert_eq!(backoff.next_delay(&ctx(2)), ms(200));
        assert_eq!(backoff.next_delay(&ctx(3)), ms(400));
        assert_eq!(backoff.next_delay(&ctx(4)), ms(800));
    }

    #[test]
    fn honors_custom_multiplier() {
        let backoff = ExponentialBackoff::new(
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
        assert_eq!(
            exponential(100, 500, JitterKind::None).next_delay(&ctx(10)),
            ms(500)
        );
    }

    #[test]
    fn applies_full_jitter_by_default() {
        let backoff = ExponentialBackoff::new(ExponentialBackoffOptions::builder().base(ms(100)).build())
            .with_random(half());
        assert_eq!(backoff.next_delay(&ctx(3)), ms(200));
    }

    #[test]
    fn applies_equal_jitter_as_raw_2_random_raw_2() {
        assert_eq!(
            exponential(100, 30_000, JitterKind::Equal).next_delay(&ctx(2)),
            ms(150)
        );
    }

    #[test]
    fn applies_no_jitter_when_set_to_none() {
        assert_eq!(
            exponential(100, 30_000, JitterKind::None).next_delay(&ctx(3)),
            ms(400)
        );
    }

    #[test]
    fn falls_back_to_full_jitter_for_decorrelated_when_last_delay_ms_missing() {
        assert_eq!(
            exponential(100, 30_000, JitterKind::Decorrelated).next_delay(&ctx(3)),
            ms(200)
        );
    }

    #[test]
    fn applies_decorrelated_jitter_as_uniform_base_ms_min_max_ms_last_delay_ms_3() {
        let backoff = exponential(100, 10_000, JitterKind::Decorrelated);
        assert_eq!(backoff.next_delay(&ctx(2).with_last_delay(ms(200))), ms(350));
    }

    #[test]
    fn caps_decorrelated_upper_at_max_ms() {
        let backoff = exponential(100, 500, JitterKind::Decorrelated);
        assert_eq!(backoff.next_delay(&ctx(2).with_last_delay(ms(1000))), ms(300));
    }

    #[test]
    fn floors_decorrelated_upper_at_base_ms_when_last_delay_3_base_ms() {
        let backoff = exponential(100, 500, JitterKind::Decorrelated);
        assert_eq!(backoff.next_delay(&ctx(2).with_last_delay(ms(10))), ms(100));
    }

    #[test]
    fn caps_at_max_when_growth_overflows() {
        let backoff = exponential(100, 500, JitterKind::None);
        assert_eq!(backoff.next_delay(&ctx(u32::MAX)), ms(500));
    }
}

mod random_source {
    use super::*;

    #[test]
    fn seeded_sources_repeat_their_sequence_within_unit_range() {
        let (a, b) = (RandomSource::seeded(42), RandomSource::seeded(42));
        let first: Vec<f64> = (0..16).map(|_| a.sample()).collect();
        let second: Vec<f64> = (0..16).map(|_| b.sample()).collect();
        assert_eq!(first, second);
        assert!(first.iter().all(|value| (0.0..1.0).contains(value)));
        assert_ne!(first[0], first[1]);
    }

    #[test]
    fn clamps_out_of_range_samples() {
        assert_eq!(RandomSource::new(|| 2.0).sample(), 1.0);
        assert_eq!(RandomSource::new(|| -1.0).sample(), 0.0);
        assert_eq!(RandomSource::new(|| f64::NAN).sample(), 0.0);
    }
}
