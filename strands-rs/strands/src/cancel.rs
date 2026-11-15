//! Cooperative cancellation primitives (TS `AbortSignal` and `CancelOptions` in `types/agent.ts`).
//!
//! A [`CancelSignal`] is what cancellable work watches; a [`CancelHandle`] requests cancellation.
//! The agent's cancel semantics (checkpoints, the final message, deferred cancels) build on these.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

/// Options for [`CancelHandle::cancel`] (TS `CancelOptions`).
///
/// ```
/// use strands::CancelOptions;
///
/// let options = CancelOptions::builder().message("Task complete").after_current_tools(true).build();
/// assert_eq!(options, CancelOptions { message: Some("Task complete".into()), after_current_tools: true });
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
pub struct CancelOptions {
    /// Final assistant message for the cancelled invocation. The last message set wins; a cancel
    /// without one keeps a message set earlier.
    #[builder(into)]
    pub message: Option<String>,
    /// Defer cancellation until the running tool batch finishes, so sibling tools complete
    /// normally. A deferred request doesn't flip [`CancelSignal::is_cancelled`].
    #[builder(default)]
    pub after_current_tools: bool,
}

#[derive(Debug, Default)]
struct CancelState {
    token: CancellationToken,
    reason: Mutex<Option<String>>,
    deferred: AtomicBool,
}

/// A cancellation signal that cancellable work watches (TS `AbortSignal`).
///
/// Cloning is cheap and every clone observes the same cancellation.
///
/// ```
/// use strands::{CancelOptions, CancelSignal};
///
/// let (signal, handle) = CancelSignal::new();
/// assert!(!signal.is_cancelled());
/// handle.cancel(CancelOptions { message: Some("stop".into()), ..CancelOptions::default() });
/// assert!(signal.is_cancelled());
/// assert_eq!(signal.reason().as_deref(), Some("stop"));
/// ```
#[derive(Debug, Clone)]
pub struct CancelSignal {
    state: Arc<CancelState>,
}

/// Requests cancellation of the [`CancelSignal`] it was created with.
///
/// Cloning is cheap; any clone can cancel. Cancelling is idempotent. The handle an agent returns
/// from [`Agent::cancel_handle`](crate::Agent::cancel_handle) cancels whichever invocation is
/// running, and does nothing while the agent is idle.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    target: Target,
}

#[derive(Debug, Clone)]
enum Target {
    Signal(Arc<CancelState>),
    Current(Arc<Mutex<Option<Arc<CancelState>>>>),
}

impl CancelSignal {
    /// A new signal and the handle that cancels it.
    #[must_use]
    pub fn new() -> (Self, CancelHandle) {
        let state = Arc::new(CancelState::default());
        (
            Self {
                state: Arc::clone(&state),
            },
            CancelHandle {
                target: Target::Signal(state),
            },
        )
    }

    /// Whether cancellation has been requested without deferral (TS `signal.aborted`).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.token.is_cancelled()
    }

    /// Completes once the signal is cancelled; immediately if it already is.
    pub async fn cancelled(&self) {
        self.state.token.cancelled().await;
    }

    /// The message of the latest cancel request that carried one (TS `signal.reason`).
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.state
            .reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether a cancel with [`CancelOptions::after_current_tools`] was requested.
    pub(crate) fn is_deferred(&self) -> bool {
        self.state.deferred.load(Ordering::Acquire)
    }
}

impl CancelHandle {
    /// Requests cancellation. With [`CancelOptions::after_current_tools`] the request is recorded
    /// as deferred and the signal stays uncancelled; otherwise the signal is cancelled. A message
    /// replaces any earlier one.
    pub fn cancel(&self, options: CancelOptions) {
        let Some(state) = self.state() else {
            return;
        };
        if let Some(message) = options.message {
            *state.reason.lock().unwrap_or_else(PoisonError::into_inner) = Some(message);
        }
        if options.after_current_tools {
            state.deferred.store(true, Ordering::Release);
        } else {
            state.token.cancel();
        }
    }

    /// A signal observing this handle's cancellation.
    #[must_use]
    pub fn signal(&self) -> CancelSignal {
        CancelSignal {
            state: self.state().unwrap_or_default(),
        }
    }

    fn state(&self) -> Option<Arc<CancelState>> {
        match &self.target {
            Target::Signal(state) => Some(Arc::clone(state)),
            Target::Current(current) => current.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        }
    }
}

/// The cancel state of an agent's invocations: each invocation gets a fresh signal, and the
/// agent's stable [`CancelHandle`] reaches whichever one is running (TS `resetCancelState`).
#[derive(Debug, Default)]
pub(crate) struct CancelScope {
    current: Arc<Mutex<Option<Arc<CancelState>>>>,
}

impl CancelScope {
    pub(crate) fn handle(&self) -> CancelHandle {
        CancelHandle {
            target: Target::Current(Arc::clone(&self.current)),
        }
    }

    /// Starts an invocation's cancel state, also cancelled by `external` (TS `AbortSignal.any`).
    pub(crate) fn begin(&self, external: Option<&CancelSignal>) -> CancelSignal {
        let token = external.map_or_else(CancellationToken::new, |external| {
            external.state.token.child_token()
        });
        let state = Arc::new(CancelState {
            token,
            ..CancelState::default()
        });
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&state));
        CancelSignal { state }
    }

    /// Ends the running invocation's cancel state; cancelling while idle does nothing.
    pub(crate) fn end(&self) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl CancelSignal {
    /// A handle that cancels this signal.
    pub(crate) fn handle(&self) -> CancelHandle {
        CancelHandle {
            target: Target::Signal(Arc::clone(&self.state)),
        }
    }
}

#[cfg(test)]
mod tests;
