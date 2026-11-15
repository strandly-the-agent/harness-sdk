//! Cancellation primitives (TS `AbortSignal` and `CancelOptions` from `types/agent.ts`).

use std::sync::{Arc, Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

/// Options for cancelling (TS `CancelOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct CancelOptions {
    /// Final assistant message for the cancelled invocation. Last write wins; `None` keeps a
    /// previously stored message.
    pub message: Option<String>,
    /// Defer cancellation until the running tool batch finishes, so sibling tools complete.
    #[builder(default)]
    pub after_current_tools: bool,
}

#[derive(Debug, Default)]
struct CancelState {
    reason: Option<String>,
    deferred: bool,
}

#[derive(Debug, Default)]
struct Shared {
    token: CancellationToken,
    state: Mutex<CancelState>,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, CancelState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The observing side of a cancellation (the TS `AbortSignal` role).
///
/// Cheap to clone; clones observe the same cancellation.
///
/// ```
/// use strands::{CancelOptions, CancelSignal};
///
/// let (signal, handle) = CancelSignal::new();
/// handle.cancel(CancelOptions { message: Some("stop".into()), ..Default::default() });
/// assert!(signal.is_cancelled());
/// assert_eq!(signal.reason().as_deref(), Some("stop"));
/// ```
#[derive(Debug, Clone)]
pub struct CancelSignal {
    shared: Arc<Shared>,
}

/// The triggering side of a cancellation. Cheap to clone.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    shared: Arc<Shared>,
}

impl CancelSignal {
    /// A new signal and the handle that cancels it.
    pub fn new() -> (Self, CancelHandle) {
        let shared = Arc::new(Shared::default());
        (
            Self {
                shared: Arc::clone(&shared),
            },
            CancelHandle { shared },
        )
    }

    /// Whether the signal has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.shared.token.is_cancelled()
    }

    /// Completes once the signal is cancelled; immediately when it already is.
    pub async fn cancelled(&self) {
        self.shared.token.cancelled().await;
    }

    /// The message given with the cancellation, if any.
    pub fn reason(&self) -> Option<String> {
        self.shared.state().reason.clone()
    }

    /// Whether a deferred (`after_current_tools`) cancel is pending.
    #[allow(dead_code, reason = "read by the agent loop's tool batches")]
    pub(crate) fn is_deferred(&self) -> bool {
        self.shared.state().deferred
    }
}

impl CancelHandle {
    /// Requests cancellation.
    ///
    /// With `after_current_tools`, the cancel is recorded as deferred and the signal stays
    /// uncancelled until the running tool batch finishes.
    pub fn cancel(&self, options: CancelOptions) {
        {
            let mut state = self.shared.state();
            if options.message.is_some() {
                state.reason = options.message;
            }
            if options.after_current_tools {
                state.deferred = true;
                return;
            }
        }
        self.shared.token.cancel();
    }

    /// Whether the signal has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.shared.token.is_cancelled()
    }

    /// A signal observing this handle's cancellation.
    pub fn signal(&self) -> CancelSignal {
        CancelSignal {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// Tests for the cancel primitives. TS has no separate spec for them (it uses `AbortSignal`); the
/// agent semantics in strands-ts/src/agent/__tests__/agent.cancel.test.ts are ported with the agent.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_new_signal_is_not_cancelled() {
        let (signal, handle) = CancelSignal::new();
        assert!(!signal.is_cancelled());
        assert!(!handle.is_cancelled());
        assert_eq!(signal.reason(), None);
    }

    #[test]
    fn cancel_cancels_every_clone_and_keeps_the_message() {
        let (signal, handle) = CancelSignal::new();
        let clone = signal.clone();
        handle.cancel(CancelOptions::builder().message("Task complete").build());
        assert!(signal.is_cancelled() && clone.is_cancelled() && handle.signal().is_cancelled());
        assert_eq!(clone.reason().as_deref(), Some("Task complete"));
    }

    #[test]
    fn last_message_wins_and_a_cancel_without_a_message_keeps_it() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions::builder().message("first").build());
        handle.cancel(CancelOptions::builder().message("second").build());
        handle.cancel(CancelOptions::default());
        assert_eq!(signal.reason().as_deref(), Some("second"));
    }

    #[test]
    fn after_current_tools_defers_the_cancel() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions {
            after_current_tools: true,
            ..Default::default()
        });
        assert!(!signal.is_cancelled());
        assert!(signal.is_deferred());
        handle.cancel(CancelOptions::default());
        assert!(signal.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_completes_when_the_handle_cancels() {
        let (signal, handle) = CancelSignal::new();
        let waiter = tokio::spawn(async move { signal.cancelled().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!waiter.is_finished());
        handle.cancel(CancelOptions::default());
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_completes_at_once_when_already_cancelled() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions::default());
        signal.cancelled().await;
    }
}
