//! Logging helpers (TS `logging/`). `tracing` replaces the TS logger and `configureLogging`.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Emits `message` as a warning at most once per process (TS `warnOnce`), so repeated nudges such
/// as the default model id warning don't flood logs when many instances are built.
#[cfg_attr(not(test), expect(dead_code, reason = "used by the model defaults"))]
pub(crate) fn warn_once(message: &str) {
    warn_once_in(&WARNED, message, |message| tracing::warn!("{message}"));
}

fn warn_once_in(warned: &Mutex<HashSet<String>>, message: &str, emit: impl FnOnce(&str)) {
    let first = warned
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(message.to_owned());
    if first {
        emit(message);
    }
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/logging/__tests__/warn-once.test.ts

    use std::collections::HashSet;
    use std::sync::Mutex;

    use super::warn_once_in;

    fn warn(warned: &Mutex<HashSet<String>>, emitted: &mut Vec<String>, message: &str) {
        warn_once_in(warned, message, |message| emitted.push(message.to_owned()));
    }

    #[test]
    fn emits_a_warning_the_first_time_a_message_is_seen() {
        let warned = Mutex::default();
        let mut emitted = Vec::new();
        warn(&warned, &mut emitted, "first-seen-msg");
        assert_eq!(emitted, ["first-seen-msg"]);
    }

    #[test]
    fn does_not_emit_repeated_warnings_for_the_same_message() {
        let warned = Mutex::default();
        let mut emitted = Vec::new();
        for _ in 0..3 {
            warn(&warned, &mut emitted, "repeated-msg");
        }
        assert_eq!(emitted, ["repeated-msg"]);
    }

    #[test]
    fn emits_distinct_messages_independently() {
        let warned = Mutex::default();
        let mut emitted = Vec::new();
        warn(&warned, &mut emitted, "distinct-alpha-msg");
        warn(&warned, &mut emitted, "distinct-beta-msg");
        warn(&warned, &mut emitted, "distinct-alpha-msg");
        assert_eq!(emitted, ["distinct-alpha-msg", "distinct-beta-msg"]);
    }

    #[test]
    fn the_process_wide_set_dedupes_too() {
        super::warn_once("warn-once-process-wide-test-msg");
        let first = super::WARNED
            .lock()
            .unwrap()
            .contains("warn-once-process-wide-test-msg");
        assert!(first);
    }
}
