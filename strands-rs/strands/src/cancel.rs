//! Cancellation primitives (TS `AbortSignal` and `CancelOptions` from `types/agent.ts`).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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
    /// The external signal this one joins (TS `AbortSignal.any`).
    parent: Option<CancelSignal>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, CancelState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cancel(&self, options: CancelOptions) {
        {
            let mut state = self.state();
            if options.message.is_some() {
                state.reason = options.message;
            }
            if options.after_current_tools {
                state.deferred = true;
                return;
            }
        }
        self.token.cancel();
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

/// The triggering side of a cancellation. Cheap to clone; clones cancel the same target.
///
/// The handle from [`CancelSignal::new`] cancels its signal. The handle from
/// `Agent::cancel_handle` cancels whichever invocation of that agent is running, and does
/// nothing while the agent is idle.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    target: Arc<Mutex<Option<Arc<Shared>>>>,
}

impl CancelSignal {
    /// A new signal and the handle that cancels it.
    pub fn new() -> (Self, CancelHandle) {
        let signal = Self::joining(None);
        let handle = CancelHandle::idle();
        handle.attach(&signal);
        (signal, handle)
    }

    /// A fresh signal that is also cancelled when `parent` is.
    pub(crate) fn joining(parent: Option<&Self>) -> Self {
        let token = parent.map_or_else(CancellationToken::new, |parent| parent.shared.token.child_token());
        Self {
            shared: Arc::new(Shared {
                token,
                state: Mutex::default(),
                parent: parent.cloned(),
            }),
        }
    }

    /// A signal cancelled when this one is, without its message or deferral; what a sub-agent
    /// joins (TS forwards only the `AbortSignal`).
    pub(crate) fn abort_only(&self) -> Self {
        Self {
            shared: Arc::new(Shared {
                token: self.shared.token.child_token(),
                state: Mutex::default(),
                parent: None,
            }),
        }
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
        let own = self.shared.state().reason.clone();
        own.or_else(|| self.shared.parent.as_ref().and_then(Self::reason))
    }

    /// Whether both observe the same cancellation (TS `toBe` on signals).
    #[cfg(test)]
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Whether a deferred (`after_current_tools`) cancel is pending.
    pub(crate) fn is_deferred(&self) -> bool {
        self.shared.state().deferred || self.shared.parent.as_ref().is_some_and(Self::is_deferred)
    }
}

impl CancelHandle {
    /// A handle with nothing to cancel until [`attach`](Self::attach).
    pub(crate) fn idle() -> Self {
        Self {
            target: Arc::default(),
        }
    }

    fn target(&self) -> MutexGuard<'_, Option<Arc<Shared>>> {
        self.target.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Points this handle and its clones at `signal`.
    pub(crate) fn attach(&self, signal: &CancelSignal) {
        *self.target() = Some(Arc::clone(&signal.shared));
    }

    /// Leaves this handle and its clones with nothing to cancel.
    pub(crate) fn detach(&self) {
        *self.target() = None;
    }

    /// Requests cancellation; a no-op when there is nothing to cancel.
    ///
    /// With `after_current_tools`, the cancel is recorded as deferred and the signal stays
    /// uncancelled until the running tool batch finishes.
    pub fn cancel(&self, options: CancelOptions) {
        let target = self.target().clone();
        if let Some(shared) = target {
            shared.cancel(options);
        }
    }

    /// Whether the target signal has been cancelled; `false` when there is none.
    pub fn is_cancelled(&self) -> bool {
        self.target()
            .as_ref()
            .is_some_and(|shared| shared.token.is_cancelled())
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
        assert!(signal.is_cancelled() && clone.is_cancelled() && handle.is_cancelled());
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

    #[test]
    fn an_abort_only_view_follows_the_abort_but_not_the_message_or_deferral() {
        let (parent, handle) = CancelSignal::new();
        let view = parent.abort_only();
        handle.cancel(CancelOptions {
            message: Some("stop".to_owned()),
            after_current_tools: true,
        });
        assert!(!view.is_deferred() && !view.is_cancelled());
        handle.cancel(CancelOptions::default());
        assert!(view.is_cancelled());
        assert_eq!(view.reason(), None);
    }

    #[test]
    fn a_joined_signal_follows_its_parent_but_not_the_reverse() {
        let (parent, parent_handle) = CancelSignal::new();
        let joined = CancelSignal::joining(Some(&parent));
        let handle = CancelHandle::idle();
        handle.attach(&joined);
        handle.cancel(CancelOptions::default());
        assert!(joined.is_cancelled() && !parent.is_cancelled());

        let joined = CancelSignal::joining(Some(&parent));
        parent_handle.cancel(CancelOptions::builder().message("outer").build());
        assert!(joined.is_cancelled());
        assert_eq!(joined.reason().as_deref(), Some("outer"));
    }

    #[test]
    fn an_idle_handle_cancels_nothing() {
        let handle = CancelHandle::idle();
        handle.cancel(CancelOptions::default());
        assert!(!handle.is_cancelled());
        let signal = CancelSignal::joining(None);
        handle.attach(&signal);
        handle.detach();
        handle.cancel(CancelOptions::default());
        assert!(!signal.is_cancelled());
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
