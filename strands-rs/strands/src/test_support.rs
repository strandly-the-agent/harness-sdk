//! Test doubles shared by unit tests.

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

/// A tool whose stream is a closure (TS `__fixtures__/tool-helpers.ts` tools).
pub(crate) struct MockTool {
    name: String,
    stream: Box<dyn Fn(crate::tools::ToolContext) -> crate::tools::ToolStream + Send + Sync>,
}

impl crate::tools::Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Mock tool"
    }

    fn tool_spec(&self) -> crate::types::tools::ToolSpec {
        crate::types::tools::ToolSpec::new(self.name.as_str(), format!("Mock tool {}", self.name))
    }

    fn stream(&self, context: crate::tools::ToolContext) -> crate::tools::ToolStream {
        (self.stream)(context)
    }
}

/// TS `createMockTool`: a tool returning `result(context)`.
pub(crate) fn mock_tool(
    name: &str,
    result: impl Fn(&crate::tools::ToolContext) -> crate::types::messages::ToolResultBlock + Send + Sync + 'static,
) -> MockTool {
    let result = std::sync::Arc::new(result);
    stream_tool(name, move |context| {
        let result = std::sync::Arc::clone(&result);
        crate::tools::ToolStream::from_future(async move { result(&context).into() })
    })
}

/// A tool whose stream is built by `stream`.
pub(crate) fn stream_tool(
    name: &str,
    stream: impl Fn(crate::tools::ToolContext) -> crate::tools::ToolStream + Send + Sync + 'static,
) -> MockTool {
    MockTool {
        name: name.to_owned(),
        stream: Box::new(stream),
    }
}

/// A success result with one text block.
pub(crate) fn text_result(tool_use_id: &str, text: &str) -> crate::types::messages::ToolResultBlock {
    crate::types::messages::ToolResultBlock::new(
        tool_use_id,
        crate::types::tools::ToolResultStatus::Success,
        vec![text.into()],
    )
}
