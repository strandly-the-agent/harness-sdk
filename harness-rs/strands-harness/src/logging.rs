//! Warn-once logging (TS `logging.ts`; `configureLogging` is replaced by `tracing` subscribers).

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Logs `message` at warn level the first time this process sees it.
pub(crate) fn warn_once(message: &str) {
    let first = WARNED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(message.to_owned());
    if first {
        tracing::warn!("{message}");
    }
}

/// Whether [`warn_once`] has logged `message` in this process (tests can't tell which test's
/// call logged a once-only warning).
#[cfg(test)]
pub(crate) fn warned(message: &str) -> bool {
    WARNED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(message)
}
