//! A `tracing` subscriber that records log messages for tests.

use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Records the level and `message` of every event emitted while it is the default subscriber.
#[derive(Clone, Default)]
pub(crate) struct LogRecorder(Arc<Mutex<Vec<(Level, String)>>>);

impl LogRecorder {
    /// The messages recorded at `level`, oldest first.
    pub(crate) fn messages(&self, level: Level) -> Vec<String> {
        let records = self.0.lock().unwrap();
        records
            .iter()
            .filter(|(recorded, _)| *recorded == level)
            .map(|(_, message)| message.clone())
            .collect()
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for LogRecorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        self.0.lock().unwrap().push((*event.metadata().level(), message));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// Runs `f` with a [`LogRecorder`] as the default subscriber and returns the warnings it logged.
pub(crate) fn capture_warnings<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let recorder = LogRecorder::default();
    let result = tracing::subscriber::with_default(recorder.clone(), f);
    (result, recorder.messages(Level::WARN))
}
