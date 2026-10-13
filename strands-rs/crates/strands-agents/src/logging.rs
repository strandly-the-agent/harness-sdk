//! Logging. The SDK logs through [`tracing`]; install any subscriber to see output.
//!
//! TS injects a 4-method `Logger` object (`configureLogging`); Rust has no equivalent because
//! `tracing` already owns that seam — the levels map `logger.debug` → [`tracing::debug!`],
//! `info` → [`tracing::info!`], `warn` → [`tracing::warn!`], `error` → [`tracing::error!`], and
//! filtering is the subscriber's job (the TS default logger drops debug/info; with no subscriber
//! installed Rust drops everything). Message bodies keep the SDK's
//! `key=<value> … | human sentence` format verbatim.

use std::collections::HashSet;
use std::sync::Mutex;

static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Emit a [`tracing::warn!`] once per distinct message for the lifetime of the process.
///
/// The message is its own dedupe key, which keeps repeated nudges (e.g. "using default modelId")
/// out of the logs when many instances are constructed.
pub fn warn_once(message: &str) {
    let mut guard = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if guard.get_or_insert_with(HashSet::new).insert(message.to_owned()) {
        tracing::warn!("{message}");
    }
}

/// Forget every message [`warn_once`] has seen, so the next call warns again.
#[cfg(test)]
pub(crate) fn reset_warn_once() {
    WARNED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use parking_lot::Mutex as PlMutex;

    use super::*;

    struct Writer(Arc<PlMutex<Vec<u8>>>);

    impl io::Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Captured `tracing` output of `f`, for this thread only.
    fn capture(f: impl FnOnce()) -> String {
        let buffer = Arc::new(PlMutex::new(Vec::new()));
        let sink = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || Writer(Arc::clone(&sink)))
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let output = buffer.lock().clone();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn emits_a_warning_the_first_time_a_message_is_seen() {
        let output = capture(|| warn_once("first-seen-msg"));
        assert_eq!(output.matches("first-seen-msg").count(), 1, "{output}");
        assert!(output.contains("WARN"), "{output}");
    }

    #[test]
    fn does_not_emit_repeated_warnings_for_the_same_message() {
        let output = capture(|| {
            warn_once("repeated-msg");
            warn_once("repeated-msg");
            warn_once("repeated-msg");
        });
        assert_eq!(output.matches("repeated-msg").count(), 1, "{output}");
    }

    #[test]
    fn emits_distinct_messages_independently() {
        let output = capture(|| {
            warn_once("distinct-alpha-msg");
            warn_once("distinct-beta-msg");
            warn_once("distinct-alpha-msg");
        });
        assert_eq!(output.matches("distinct-alpha-msg").count(), 1, "{output}");
        assert_eq!(output.matches("distinct-beta-msg").count(), 1, "{output}");
    }

    #[test]
    fn reset_warn_once_lets_a_message_warn_again() {
        let output = capture(|| {
            warn_once("resettable-msg");
            reset_warn_once();
            warn_once("resettable-msg");
        });
        assert_eq!(output.matches("resettable-msg").count(), 2, "{output}");
    }
}
