//! Cancellation signals and handles.
//!
//! A [`CancelSignal`] is what running code checks; a [`CancelHandle`] is what a caller uses to
//! cancel. `CancelSignal::new()` returns a connected pair for signals you control yourself.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

/// Options for [`CancelHandle::cancel`] (TS `CancelOptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
pub struct CancelOptions {
    /// Final assistant message for the cancelled invocation. The last message set wins; a cancel
    /// without one keeps an earlier one.
    #[builder(into)]
    pub message: Option<String>,
    /// Defer cancellation until the running tool batch finishes, so sibling tools complete
    /// normally. A deferred cancel that never applies (the model answers without tools) is a no-op.
    #[builder(default)]
    pub after_current_tools: bool,
}

struct CancelState {
    token: CancellationToken,
    message: Mutex<Option<String>>,
    deferred: AtomicBool,
}

/// A cancellation signal that running code can check or await.
///
/// Cloning is cheap; clones observe the same signal.
///
/// ```
/// use strands::{CancelOptions, CancelSignal};
///
/// let (signal, handle) = CancelSignal::new();
/// assert!(!signal.is_cancelled());
/// handle.cancel(CancelOptions { message: Some("Stopped".into()), ..Default::default() });
/// assert!(signal.is_cancelled());
/// assert_eq!(signal.reason().as_deref(), Some("Stopped"));
/// ```
#[derive(Clone)]
pub struct CancelSignal {
    state: Arc<CancelState>,
}

/// Cancels the [`CancelSignal`] it was created with.
///
/// Cloning is cheap; every clone cancels the same signal.
#[derive(Clone)]
pub struct CancelHandle {
    state: Arc<CancelState>,
}

impl CancelSignal {
    /// A new signal and the handle that cancels it.
    pub fn new() -> (CancelSignal, CancelHandle) {
        let state = Arc::new(CancelState {
            token: CancellationToken::new(),
            message: Mutex::new(None),
            deferred: AtomicBool::new(false),
        });
        (
            CancelSignal {
                state: Arc::clone(&state),
            },
            CancelHandle { state },
        )
    }

    /// Whether the signal has fired. A deferred cancel doesn't fire it until it applies.
    pub fn is_cancelled(&self) -> bool {
        self.state.token.is_cancelled()
    }

    /// Completes once the signal fires.
    pub async fn cancelled(&self) {
        self.state.token.cancelled().await;
    }

    /// The message passed with the cancel, if any.
    pub fn reason(&self) -> Option<String> {
        self.state
            .message
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether a cancel deferred until the current tools finish is pending.
    pub(crate) fn is_deferred(&self) -> bool {
        self.state.deferred.load(Ordering::Acquire)
    }

    /// Fires the signal now, for a deferred cancel that applies.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop"))]
    pub(crate) fn fire(&self) {
        self.state.token.cancel();
    }
}

impl CancelHandle {
    /// Cancels the signal, or with `after_current_tools` marks it to cancel after the running
    /// tool batch.
    pub fn cancel(&self, options: CancelOptions) {
        if let Some(message) = options.message {
            *self.state.message.lock().unwrap_or_else(PoisonError::into_inner) = Some(message);
        }
        if options.after_current_tools {
            self.state.deferred.store(true, Ordering::Release);
        } else {
            self.state.token.cancel();
        }
    }

    /// Whether the signal has fired.
    pub fn is_cancelled(&self) -> bool {
        self.state.token.is_cancelled()
    }
}

impl fmt::Debug for CancelSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelSignal")
            .field("cancelled", &self.is_cancelled())
            .field("deferred", &self.state.deferred.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelHandle")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_fires_the_signal_and_keeps_the_message() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions::builder().message("Done").build());
        assert!(signal.is_cancelled());
        assert!(handle.is_cancelled());
        assert_eq!(signal.reason().as_deref(), Some("Done"));
    }

    #[test]
    fn a_cancel_without_a_message_keeps_the_earlier_message() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions {
            message: Some("first".into()),
            after_current_tools: true,
        });
        handle.cancel(CancelOptions::default());
        assert_eq!(signal.reason().as_deref(), Some("first"));
    }

    #[test]
    fn the_last_message_wins() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions::builder().message("first").build());
        handle.cancel(CancelOptions::builder().message("second").build());
        assert_eq!(signal.reason().as_deref(), Some("second"));
    }

    #[test]
    fn after_current_tools_defers_until_fired() {
        let (signal, handle) = CancelSignal::new();
        handle.cancel(CancelOptions::builder().after_current_tools(true).build());
        assert!(signal.is_deferred());
        assert!(!signal.is_cancelled());
        signal.fire();
        assert!(signal.is_cancelled());
    }

    #[test]
    fn clones_observe_the_same_signal() {
        let (signal, handle) = CancelSignal::new();
        let other = signal.clone();
        let other_handle = handle.clone();
        other_handle.cancel(CancelOptions::default());
        assert!(other.is_cancelled());
        assert!(signal.is_cancelled());
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_completes_once_the_signal_fires() {
        let (signal, handle) = CancelSignal::new();
        let waiter = tokio::spawn({
            let signal = signal.clone();
            async move { signal.cancelled().await }
        });
        handle.cancel(CancelOptions::default());
        waiter.await.unwrap();
        assert!(signal.is_cancelled());
    }
}
