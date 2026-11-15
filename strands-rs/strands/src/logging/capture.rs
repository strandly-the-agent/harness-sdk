//! A `tracing` subscriber that records log messages for tests.

use std::sync::{Arc, Mutex, Once};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::DefaultGuard;
use tracing::{Event, Level, Metadata, Subscriber};

/// Callsite interest is cached process-wide, from the registering thread's subscriber alone when one
/// scoped subscriber is alive; a global subscriber enabling everything keeps it from caching `never`.
static ENABLE_ALL_CALLSITES: Once = Once::new();

struct EnableAll;

impl Subscriber for EnableAll {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// Records the level and `message` of every event emitted while it is the default subscriber.
#[derive(Clone, Default)]
pub(crate) struct LogRecorder(Arc<Mutex<Vec<(Level, String)>>>);

impl LogRecorder {
    /// Makes this recorder the thread's default subscriber until the guard drops.
    pub(crate) fn set_default(&self) -> DefaultGuard {
        ENABLE_ALL_CALLSITES.call_once(|| {
            let _ = tracing::subscriber::set_global_default(EnableAll);
        });
        tracing::subscriber::set_default(self.clone())
    }

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
    let guard = recorder.set_default();
    let result = f();
    drop(guard);
    (result, recorder.messages(Level::WARN))
}
