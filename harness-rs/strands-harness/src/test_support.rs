//! Test doubles shared by unit tests.
#![expect(
    dead_code,
    reason = "used once the modules that assert on warnings land; drop this when they have"
)]

use std::cell::RefCell;
use std::fmt::Debug;
use std::sync::Once;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

thread_local! {
    /// The level being captured and the messages captured so far.
    static CAPTURED: RefCell<Option<(Level, Vec<String>)>> = const { RefCell::new(None) };
}

/// Records the messages of `warn!` events emitted on the current thread, or those of another
/// level with [`capture_level`](Self::capture_level).
///
/// A single global subscriber, not a scoped one per test: a callsite first hit on another thread
/// while no scoped subscriber was alive could cache `Interest::never` and drop the capture.
pub(crate) struct WarnRecorder;

impl WarnRecorder {
    /// Runs `f` and returns its result with the warnings it logged on this thread.
    pub(crate) fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
        Self::capture_level(Level::WARN, f)
    }

    /// [`capture`](Self::capture) for the events of `level`.
    pub(crate) fn capture_level<R>(level: Level, f: impl FnOnce() -> R) -> (R, Vec<String>) {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            tracing::subscriber::set_global_default(WarnRecorder)
                .expect("no other global subscriber in unit tests");
        });
        CAPTURED.with(|captured| *captured.borrow_mut() = Some((level, Vec::new())));
        let result = f();
        let messages = CAPTURED.with(|captured| captured.borrow_mut().take());
        (result, messages.map(|(_, messages)| messages).unwrap_or_default())
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
        let level = CAPTURED.with(|captured| captured.borrow().as_ref().map(|(level, _)| *level));
        if level != Some(*event.metadata().level()) {
            return;
        }
        let mut visitor = MessageVisitor(None);
        event.record(&mut visitor);
        if let Some(message) = visitor.0 {
            CAPTURED.with(|captured| {
                if let Some((_, messages)) = captured.borrow_mut().as_mut() {
                    messages.push(message);
                }
            });
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// A `harness-ts/src` file for the text-parity tests: from the monorepo, or from the spec checkout
/// under `/tmp/ts-spec`; `None` when neither is present.
pub(crate) fn ts_source(relative: &str) -> Option<String> {
    [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../harness-ts/src"),
        "/tmp/ts-spec/harness-ts/src",
    ]
    .iter()
    .find_map(|root| std::fs::read_to_string(format!("{root}/{relative}")).ok())
}
