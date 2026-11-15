//! Process-wide warn-once logging (TS `logging/warn-once.ts`).

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

/// Ported from strands-ts/src/logging/__tests__/warn-once.test.ts
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::WarnRecorder;

    #[test]
    fn emits_a_warning_the_first_time_a_message_is_seen() {
        let ((), warnings) = WarnRecorder::capture(|| warn_once("first-seen-msg"));
        assert_eq!(warnings, ["first-seen-msg"]);
    }

    #[test]
    fn does_not_emit_repeated_warnings_for_the_same_message() {
        let ((), warnings) = WarnRecorder::capture(|| {
            warn_once("repeated-msg");
            warn_once("repeated-msg");
            warn_once("repeated-msg");
        });
        assert_eq!(warnings, ["repeated-msg"]);
    }

    #[test]
    fn emits_distinct_messages_independently() {
        let ((), warnings) = WarnRecorder::capture(|| {
            warn_once("distinct-alpha-msg");
            warn_once("distinct-beta-msg");
            warn_once("distinct-alpha-msg");
        });
        assert_eq!(warnings, ["distinct-alpha-msg", "distinct-beta-msg"]);
    }
}
