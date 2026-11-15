//! The stdout printer (TS `agent/printer.ts`): streams text, reasoning and tool activity as the
//! agent runs. On by default; `.printer(false)` turns it off.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::agent::AgentStreamEvent;
use crate::types::tools::ToolResultStatus;

/// Where printed text goes.
pub(crate) type Appender = Box<dyn FnMut(&str) + Send>;

/// The printer an agent and its event streams share.
pub(crate) type SharedPrinter = Arc<Mutex<AgentPrinter>>;

/// Writes to stdout and flushes, so streamed text shows up as it arrives (TS
/// `getDefaultAppender`).
#[expect(
    clippy::print_stdout,
    reason = "the printer's job is to write agent output to stdout"
)]
pub(crate) fn stdout_appender() -> Appender {
    Box::new(|text| {
        print!("{text}");
        // A closed stdout must not fail the agent.
        drop(std::io::stdout().flush());
    })
}

/// TS `AgentPrinter`.
pub(crate) struct AgentPrinter {
    appender: Appender,
    in_reasoning_block: bool,
    tool_count: usize,
    need_reasoning_indent: bool,
}

impl AgentPrinter {
    pub(crate) fn new(appender: Appender) -> Self {
        Self {
            appender,
            in_reasoning_block: false,
            tool_count: 0,
            need_reasoning_indent: false,
        }
    }

    pub(crate) fn shared(appender: Appender) -> SharedPrinter {
        Arc::new(Mutex::new(Self::new(appender)))
    }

    fn write(&mut self, text: &str) {
        (self.appender)(text);
    }

    pub(crate) fn process_event(&mut self, event: &AgentStreamEvent) {
        match event {
            AgentStreamEvent::ModelStreamUpdate(update) => self.model_stream_event(update.event()),
            AgentStreamEvent::BeforeToolCall(event) => {
                self.tool_count += 1;
                let name = event.tool_use().name();
                let line = match event.cancel() {
                    Some(_) => format!("\n🚫 Tool #{}: {name} (denied)\n", self.tool_count),
                    None => format!("\n🔧 Tool #{}: {name}\n", self.tool_count),
                };
                self.write(&line);
            }
            AgentStreamEvent::BeforeTools(event) if event.cancel().is_some() => {
                self.write("\n🚫 All tools denied\n");
            }
            AgentStreamEvent::ToolResult(event) => match event.result().status() {
                ToolResultStatus::Success => self.write("✓ Tool completed\n"),
                ToolResultStatus::Error => self.write("✗ Tool failed\n"),
                _ => {}
            },
            AgentStreamEvent::AgentResult(_) => self.write("\n"),
            _ => {}
        }
    }

    fn model_stream_event(&mut self, event: &ModelStreamEvent) {
        match event {
            ModelStreamEvent::BlockDelta(delta) => match delta.delta() {
                ContentBlockDelta::Text(delta) if !delta.text().is_empty() => self.write(delta.text()),
                ContentBlockDelta::ReasoningContent(delta) => {
                    if !self.in_reasoning_block {
                        self.in_reasoning_block = true;
                        self.need_reasoning_indent = true;
                        self.write("\n💭 Reasoning:\n");
                    }
                    if let Some(text) = delta.text().filter(|text| !text.is_empty()) {
                        self.write_reasoning(text);
                    }
                }
                _ => {}
            },
            ModelStreamEvent::BlockStart(start) => {
                if let Some(ContentBlockStart::ToolUse(tool_use)) = start.start() {
                    self.write(&format!("\n  ⏳ {}\n", tool_use.name()));
                }
            }
            ModelStreamEvent::BlockStop(_) if self.in_reasoning_block => {
                if !self.need_reasoning_indent {
                    self.write("\n");
                }
                self.in_reasoning_block = false;
                self.need_reasoning_indent = false;
            }
            _ => {}
        }
    }

    /// Indents each reasoning line by three spaces (TS `writeReasoningText`).
    fn write_reasoning(&mut self, text: &str) {
        let mut output = String::with_capacity(text.len());
        for ch in text.chars() {
            if self.need_reasoning_indent && ch != '\n' {
                output.push_str("   ");
                self.need_reasoning_indent = false;
            }
            output.push(ch);
            if ch == '\n' {
                self.need_reasoning_indent = true;
            }
        }
        self.write(&output);
    }
}

/// Feeds `event` to `printer`, if any.
pub(crate) fn print(printer: Option<&SharedPrinter>, event: &AgentStreamEvent) {
    if let Some(printer) = printer {
        printer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .process_event(event);
    }
}

#[cfg(test)]
mod tests;
