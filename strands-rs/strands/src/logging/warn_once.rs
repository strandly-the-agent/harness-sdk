//! Warn-once logging (TS `logging/warn-once.ts`).

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Logs `message` as a warning at most once per process; the message is the dedupe key.
pub(crate) fn warn_once(message: &str) {
    if first_time(message) {
        tracing::warn!("{message}");
    }
}

fn first_time(message: &str) -> bool {
    WARNED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(message.to_owned())
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/logging/__tests__/warn-once.test.ts
    use super::*;

    #[test]
    fn emits_a_warning_the_first_time_a_message_is_seen() {
        assert!(first_time("first-seen-msg"));
    }

    #[test]
    fn does_not_emit_repeated_warnings_for_the_same_message() {
        let emitted = (0..3).filter(|_| first_time("repeated-msg")).count();
        assert_eq!(emitted, 1);
    }

    #[test]
    fn emits_distinct_messages_independently() {
        let seen: Vec<bool> = ["distinct-alpha-msg", "distinct-beta-msg", "distinct-alpha-msg"]
            .into_iter()
            .map(first_time)
            .collect();
        assert_eq!(seen, [true, true, false]);
    }
}
