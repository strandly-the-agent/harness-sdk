//! Ported from strands-ts/src/agent/__tests__/invocation-context.test.ts.

use std::num::NonZeroU32;

use super::InvocationContext;
use crate::types::agent::InvokeLimits;
use crate::types::messages::StopReason;

fn attempts(n: u32) -> InvokeLimits {
    InvokeLimits {
        structured_output_attempts: NonZeroU32::new(n),
        ..InvokeLimits::default()
    }
}

fn turns(n: u32) -> InvokeLimits {
    InvokeLimits {
        turns: NonZeroU32::new(n),
        ..InvokeLimits::default()
    }
}

#[test]
fn counts_toward_limiting_parents_but_not_through_an_auxiliary_call() {
    let request = InvocationContext::new(Some(attempts(2)), None, false);
    let nested = InvocationContext::new(Some(turns(5)), Some(request.clone()), false);
    let auxiliary = InvocationContext::new(None, Some(request.clone()), true);

    nested.record_structured_output_failure();
    auxiliary.record_structured_output_failure();

    assert_eq!(request.structured_output_failures(), 1);
    assert_eq!(nested.reached_limit(), None);
    nested.record_structured_output_failure();
    assert_eq!(
        nested.reached_limit(),
        Some(StopReason::LimitStructuredOutputAttempts)
    );
}

#[test]
fn has_limit_sees_the_limits_of_limiting_parents_only() {
    let request = InvocationContext::new(Some(attempts(2)), None, false);
    let auxiliary = InvocationContext::new(None, Some(request.clone()), true);

    assert_eq!(
        [
            InvocationContext::new(Some(turns(5)), Some(request), false)
                .has_structured_output_attempts_limit(),
            auxiliary.has_structured_output_attempts_limit(),
            InvocationContext::new(Some(turns(5)), None, false).has_structured_output_attempts_limit(),
        ],
        [true, false, false]
    );
}

#[test]
fn limits_are_checked_in_the_ts_order() {
    let limits = InvokeLimits {
        turns: NonZeroU32::new(1),
        total_tokens: std::num::NonZeroU64::new(1),
        output_tokens: std::num::NonZeroU64::new(1),
        structured_output_attempts: NonZeroU32::new(1),
    };
    let context = InvocationContext::new(Some(limits), None, false);
    context.record_structured_output_failure();
    assert_eq!(
        context.reached_limit(),
        Some(StopReason::LimitStructuredOutputAttempts)
    );
    context.record_usage(&crate::models::streaming::Usage::new(0, 1, 0));
    assert_eq!(context.reached_limit(), Some(StopReason::LimitOutputTokens));
    context.record_usage(&crate::models::streaming::Usage::new(0, 0, 1));
    assert_eq!(context.reached_limit(), Some(StopReason::LimitTotalTokens));
    context.record_turn();
    assert_eq!(context.reached_limit(), Some(StopReason::LimitTurns));
}

#[test]
fn an_auxiliary_context_adds_tokens_to_the_request_but_not_turns() {
    let request = InvocationContext::new(Some(turns(1)), None, false);
    let auxiliary = InvocationContext::auxiliary(Some(&request)).unwrap();
    auxiliary.record_turn();
    auxiliary.record_usage(&crate::models::streaming::Usage::new(1, 2, 3));
    assert_eq!(request.usage().turns_taken(), 0);
    assert_eq!(request.usage().tokens().total_tokens(), 3);
    assert_eq!(auxiliary.reached_limit(), None);
    assert!(InvocationContext::auxiliary(None).is_none());
}
