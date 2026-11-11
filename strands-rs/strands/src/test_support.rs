//! Helpers for the crate's own unit tests.

use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// Runs `f` and returns the `message` of every event it logged, prefixed with the level.
pub(crate) fn capture_logs<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let result = tracing::subscriber::with_default(Capture(Arc::clone(&logs)), f);
    let logs = logs.lock().unwrap().clone();
    (result, logs)
}

struct Capture(Arc<Mutex<Vec<String>>>);

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {}", event.metadata().level(), visitor.0));
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
