//! Test doubles shared by unit tests.

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Records the messages of `warn!` events emitted on the current thread.
#[derive(Clone, Default)]
pub(crate) struct WarnRecorder {
    messages: Arc<Mutex<Vec<String>>>,
}

impl WarnRecorder {
    /// Runs `f` and returns its result with the warnings it logged.
    pub(crate) fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
        let recorder = Self::default();
        let result = tracing::subscriber::with_default(recorder.clone(), f);
        let messages = recorder.messages.lock().unwrap().clone();
        (result, messages)
    }
}

struct MessageVisitor(Option<String>);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl Subscriber for WarnRecorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() != Level::WARN {
            return;
        }
        let mut visitor = MessageVisitor(None);
        event.record(&mut visitor);
        if let Some(message) = visitor.0 {
            self.messages.lock().unwrap().push(message);
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
