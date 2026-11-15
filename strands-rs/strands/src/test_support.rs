//! Test doubles shared by unit tests.

use std::cell::RefCell;
use std::fmt::Debug;
use std::sync::Once;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

thread_local! {
    static CAPTURED: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Records the messages of `warn!` events emitted on the current thread.
///
/// A single global subscriber, not a scoped one per test: a callsite first hit on another thread
/// while no scoped subscriber was alive could cache `Interest::never` and drop the capture.
pub(crate) struct WarnRecorder;

impl WarnRecorder {
    /// Runs `f` and returns its result with the warnings it logged on this thread.
    pub(crate) fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            tracing::subscriber::set_global_default(WarnRecorder)
                .expect("no other global subscriber in unit tests");
        });
        CAPTURED.with(|captured| *captured.borrow_mut() = Some(Vec::new()));
        let result = f();
        let messages = CAPTURED.with(|captured| captured.borrow_mut().take().unwrap_or_default());
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
            CAPTURED.with(|captured| {
                if let Some(messages) = captured.borrow_mut().as_mut() {
                    messages.push(message);
                }
            });
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
