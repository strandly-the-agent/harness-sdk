//! Ported from strands-ts/src/agent/__tests__/invocation-context.test.ts

use std::num::NonZeroU64;

use super::{InvocationContext, InvokeLimits};
use crate::models::streaming::Usage;
use crate::types::messages::StopReason;

fn limits(turns: Option<u64>, structured_output_attempts: Option<u64>) -> InvokeLimits {
    InvokeLimits {
        turns: turns.and_then(NonZeroU64::new),
        structured_output_attempts: structured_output_attempts.and_then(NonZeroU64::new),
        ..InvokeLimits::default()
    }
}

#[test]
fn counts_toward_limiting_parents_but_not_through_an_auxiliary_call() {
    let request = InvocationContext::resolve(None, Some(limits(None, Some(2))));
    let nested = InvocationContext::resolve(Some(&request), Some(limits(Some(5), None)));
    let auxiliary = request.auxiliary();

    nested.record_structured_output_failure();
    auxiliary.record_structured_output_failure();

    assert_eq!(request.counters().structured_output_failures, 1);
    assert_eq!(nested.reached_limit(), None);
    nested.record_structured_output_failure();
    assert_eq!(
        nested.reached_limit(),
        Some(StopReason::LimitStructuredOutputAttempts)
    );
}

#[test]
fn sees_the_limits_of_limiting_parents_only() {
    let request = InvocationContext::resolve(None, Some(limits(None, Some(2))));
    let auxiliary = request.auxiliary();

    assert_eq!(
        [
            InvocationContext::resolve(Some(&request), Some(limits(Some(5), None)))
                .has_structured_output_attempts_limit(),
            auxiliary.has_structured_output_attempts_limit(),
            InvocationContext::resolve(None, Some(limits(Some(5), None)))
                .has_structured_output_attempts_limit(),
        ],
        [true, false, false]
    );
}

#[test]
fn resolve_reuses_the_inherited_context_without_limits() {
    let request = InvocationContext::new();
    assert!(InvocationContext::resolve(Some(&request), None).same_request(&request));
    assert!(!InvocationContext::resolve(Some(&request), Some(limits(Some(1), None))).same_request(&request));
}

#[test]
fn usage_flows_to_every_parent_including_through_an_auxiliary_call() {
    let request = InvocationContext::new();
    let nested = InvocationContext::resolve(Some(&request), Some(limits(Some(5), None)));
    let auxiliary = nested.auxiliary();

    auxiliary.record_usage(&Usage::new(3, 4, 7));
    nested.record_turn();
    auxiliary.record_turn();

    assert_eq!(request.usage().usage, Usage::new(3, 4, 7));
    assert_eq!(request.usage().turns_taken, 1);
    assert_eq!(nested.usage().turns_taken, 1);
}

#[test]
fn limits_are_checked_in_the_ts_order() {
    let context = InvocationContext::resolve(
        None,
        Some(InvokeLimits {
            turns: NonZeroU64::new(1),
            total_tokens: NonZeroU64::new(1),
            output_tokens: NonZeroU64::new(1),
            structured_output_attempts: NonZeroU64::new(1),
        }),
    );
    context.record_usage(&Usage::new(1, 1, 2));
    context.record_structured_output_failure();
    assert_eq!(context.reached_limit(), Some(StopReason::LimitTotalTokens));
    context.record_turn();
    assert_eq!(context.reached_limit(), Some(StopReason::LimitTurns));
}
