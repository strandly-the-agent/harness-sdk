//! The stdout printer (TS `agent/printer.ts`).

use std::io::Write;

use crate::agent::AgentStreamEvent;
use crate::models::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::tools::ToolResultStatus;

type Appender = Box<dyn FnMut(&str) + Send>;

/// Prints model text, reasoning and tool progress as the stream runs (TS `AgentPrinter`).
pub(crate) struct AgentPrinter {
    appender: Appender,
    in_reasoning_block: bool,
    need_reasoning_indent: bool,
    tool_count: u64,
}

impl AgentPrinter {
    pub(crate) fn new(appender: impl FnMut(&str) + Send + 'static) -> Self {
        Self {
            appender: Box::new(appender),
            in_reasoning_block: false,
            need_reasoning_indent: false,
            tool_count: 0,
        }
    }

    /// A printer writing to stdout (TS `getDefaultAppender`).
    pub(crate) fn stdout() -> Self {
        Self::new(|text| {
            let mut stdout = std::io::stdout().lock();
            // Printing is best effort, as `process.stdout.write` is in TS.
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
        })
    }

    fn write(&mut self, text: &str) {
        (self.appender)(text);
    }

    pub(crate) fn process_event(&mut self, event: &AgentStreamEvent) {
        match event {
            AgentStreamEvent::ModelStreamUpdate(update) => self.handle_model_stream_event(&update.event),
            AgentStreamEvent::BeforeToolCall(data) => {
                self.tool_count = self.tool_count.saturating_add(1);
                let line = if data.denied {
                    format!(
                        "\n🚫 Tool #{}: {} (denied)\n",
                        self.tool_count, data.tool_use.name
                    )
                } else {
                    format!("\n🔧 Tool #{}: {}\n", self.tool_count, data.tool_use.name)
                };
                self.write(&line);
            }
            AgentStreamEvent::BeforeTools(data) if data.denied => self.write("\n🚫 All tools denied\n"),
            AgentStreamEvent::ToolResult(data) => match data.result.status {
                ToolResultStatus::Success => self.write("✓ Tool completed\n"),
                ToolResultStatus::Error => self.write("✗ Tool failed\n"),
                _ => {}
            },
            AgentStreamEvent::AgentResult(_) => self.write("\n"),
            _ => {}
        }
    }

    fn handle_model_stream_event(&mut self, event: &ModelStreamEvent) {
        match event {
            ModelStreamEvent::BlockDelta(delta) => match &delta.delta {
                ContentBlockDelta::Text(text) if !text.text.is_empty() => {
                    let text = text.text.clone();
                    self.write(&text);
                }
                ContentBlockDelta::ReasoningContent(reasoning) => {
                    if !self.in_reasoning_block {
                        self.in_reasoning_block = true;
                        self.need_reasoning_indent = true;
                        self.write("\n💭 Reasoning:\n");
                    }
                    if let Some(text) = reasoning.text.as_deref().filter(|text| !text.is_empty()) {
                        let text = text.to_owned();
                        self.write_reasoning_text(&text);
                    }
                }
                _ => {}
            },
            ModelStreamEvent::BlockStart(crate::models::ModelContentBlockStartEvent {
                start: Some(ContentBlockStart::ToolUse(tool_use)),
                ..
            }) => {
                let line = format!("\n  ⏳ {}\n", tool_use.name);
                self.write(&line);
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

    fn write_reasoning_text(&mut self, text: &str) {
        let mut output = String::with_capacity(text.len());
        for c in text.chars() {
            if self.need_reasoning_indent && c != '\n' {
                output.push_str("   ");
                self.need_reasoning_indent = false;
            }
            output.push(c);
            if c == '\n' {
                self.need_reasoning_indent = true;
            }
        }
        self.write(&output);
    }
}

#[cfg(test)]
mod tests;
