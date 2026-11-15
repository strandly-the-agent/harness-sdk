//! Tests of the cancellation primitives. TS has no spec of its own for them (it uses the platform
//! `AbortController`); the agent-level cases from strands-ts/src/agent/__tests__/agent.cancel.test.ts
//! arrive with the agent's cancel semantics.

use std::time::Duration;

use super::{CancelOptions, CancelSignal};

#[test]
fn a_new_signal_is_not_cancelled() {
    let (signal, _handle) = CancelSignal::new();
    assert!(!signal.is_cancelled());
    assert_eq!(signal.reason(), None);
    assert!(!signal.is_deferred());
}

#[test]
fn cancel_cancels_every_clone_of_the_signal() {
    let (signal, handle) = CancelSignal::new();
    let clone = signal.clone();
    let other_handle = handle.clone();
    other_handle.cancel(CancelOptions::default());
    assert!(signal.is_cancelled());
    assert!(clone.is_cancelled());
    assert!(handle.signal().is_cancelled());
    assert_eq!(signal.reason(), None);
}

#[test]
fn cancel_is_idempotent_and_the_last_message_wins() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::builder().message("first").build());
    handle.cancel(CancelOptions::builder().message("second").build());
    assert!(signal.is_cancelled());
    assert_eq!(signal.reason().as_deref(), Some("second"));
}

#[test]
fn a_cancel_without_a_message_keeps_the_earlier_message() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(
        CancelOptions::builder()
            .message("Task complete")
            .after_current_tools(true)
            .build(),
    );
    handle.cancel(CancelOptions::default());
    assert_eq!(signal.reason().as_deref(), Some("Task complete"));
}

#[test]
fn after_current_tools_defers_without_cancelling() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::builder().after_current_tools(true).build());
    assert!(!signal.is_cancelled());
    assert!(signal.is_deferred());
    handle.cancel(CancelOptions::default());
    assert!(signal.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn cancelled_completes_when_the_handle_cancels() {
    let (signal, handle) = CancelSignal::new();
    let waiter = tokio::spawn({
        let signal = signal.clone();
        async move { signal.cancelled().await }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!waiter.is_finished());
    handle.cancel(CancelOptions::default());
    waiter.await.unwrap();
    signal.cancelled().await;
}

#[test]
fn signals_and_handles_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>(_: &T) {}
    let (signal, handle) = CancelSignal::new();
    assert_send_sync(&signal);
    assert_send_sync(&handle);
}
