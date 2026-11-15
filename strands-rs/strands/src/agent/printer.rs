//! Prints an agent's output to stdout as it streams (TS `agent/printer.ts`).

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use crate::hooks::events::{
    AgentResultEvent, BeforeToolCallEvent, BeforeToolsEvent, ModelStreamUpdateEvent, ToolResultEvent,
};
use crate::hooks::registry::HookRegistry;
use crate::hooks::types::{HookOptions, HookOrder};
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::tools::ToolResultStatus;

type Appender = dyn Fn(&str) + Send + Sync;

/// Writes text, reasoning, tool calls and tool outcomes as the agent produces them (TS
/// `AgentPrinter`). It runs as hooks ordered after every user hook, as TS prints each event after
/// its callbacks.
pub(crate) struct AgentPrinter {
    appender: Box<Appender>,
    state: Mutex<PrinterState>,
}

#[derive(Default)]
struct PrinterState {
    in_reasoning_block: bool,
    tool_count: u64,
    need_reasoning_indent: bool,
}

impl AgentPrinter {
    pub(crate) fn new(appender: impl Fn(&str) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            appender: Box::new(appender),
            state: Mutex::default(),
        })
    }

    /// A printer writing to stdout (TS `getDefaultAppender`).
    pub(crate) fn stdout() -> Arc<Self> {
        Self::new(|text| {
            let mut stdout = std::io::stdout().lock();
            // Printing is best effort: a closed stdout must not fail the invocation.
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
        })
    }

    pub(crate) fn install(self: Arc<Self>, hooks: &HookRegistry) {
        let options = HookOptions {
            order: HookOrder(i32::MAX),
        };
        let printer = Arc::clone(&self);
        hooks.add_hook_with(options, move |event: &mut ModelStreamUpdateEvent<'_>| {
            printer.model_stream_event(event.event());
        });
        let printer = Arc::clone(&self);
        hooks.add_hook_with(options, move |event: &mut BeforeToolCallEvent| {
            printer.before_tool_call(&event.tool_use().name, event.cancel().is_some());
        });
        let printer = Arc::clone(&self);
        hooks.add_hook_with(options, move |event: &mut BeforeToolsEvent<'_>| {
            if event.cancel().is_some() {
                printer.write("\n🚫 All tools denied\n");
            }
        });
        let printer = Arc::clone(&self);
        hooks.add_hook_with(options, move |event: &mut ToolResultEvent| {
            printer.tool_result(&event.result().status);
        });
        hooks.add_hook_with(options, move |_: &mut AgentResultEvent<'_>| self.write("\n"));
    }

    fn write(&self, text: &str) {
        (self.appender)(text);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PrinterState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn model_stream_event(&self, event: &ModelStreamEvent) {
        match event {
            ModelStreamEvent::ModelContentBlockDelta(data) => self.content_block_delta(&data.delta),
            ModelStreamEvent::ModelContentBlockStart(data) => {
                if let Some(ContentBlockStart::ToolUse(start)) = &data.start {
                    self.write(&format!("\n  ⏳ {}\n", start.name));
                }
            }
            ModelStreamEvent::ModelContentBlockStop(_) => self.content_block_stop(),
            _ => {}
        }
    }

    fn content_block_delta(&self, delta: &ContentBlockDelta) {
        match delta {
            ContentBlockDelta::Text(delta) if !delta.text.is_empty() => self.write(&delta.text),
            ContentBlockDelta::ReasoningContent(delta) => {
                let mut state = self.state();
                if !state.in_reasoning_block {
                    state.in_reasoning_block = true;
                    state.need_reasoning_indent = true;
                    self.write("\n💭 Reasoning:\n");
                }
                if let Some(text) = delta.text.as_deref().filter(|text| !text.is_empty()) {
                    let output = indent_reasoning(text, &mut state.need_reasoning_indent);
                    self.write(&output);
                }
            }
            _ => {}
        }
    }

    fn content_block_stop(&self) {
        let mut state = self.state();
        if !state.in_reasoning_block {
            return;
        }
        if !state.need_reasoning_indent {
            self.write("\n");
        }
        state.in_reasoning_block = false;
        state.need_reasoning_indent = false;
    }

    pub(crate) fn before_tool_call(&self, name: &str, cancelled: bool) {
        let mut state = self.state();
        state.tool_count = state.tool_count.saturating_add(1);
        let count = state.tool_count;
        drop(state);
        if cancelled {
            self.write(&format!("\n🚫 Tool #{count}: {name} (denied)\n"));
        } else {
            self.write(&format!("\n🔧 Tool #{count}: {name}\n"));
        }
    }

    pub(crate) fn tool_result(&self, status: &ToolResultStatus) {
        match status {
            ToolResultStatus::Success => self.write("✓ Tool completed\n"),
            ToolResultStatus::Error => self.write("✗ Tool failed\n"),
            _ => {}
        }
    }
}

/// Indents each reasoning line by three spaces (TS `writeReasoningText`).
fn indent_reasoning(text: &str, need_indent: &mut bool) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if *need_indent && character != '\n' {
            output.push_str("   ");
            *need_indent = false;
        }
        output.push(character);
        if character == '\n' {
            *need_indent = true;
        }
    }
    output
}

#[cfg(test)]
mod tests;
