//! Printing agent activity to a writer (TS `printer.ts`).
//!
//! Divergences from TS: the appender callback becomes a `std::io::Write` sink
//! (`AgentPrinter<W>`, defaulting to [`std::io::Stdout`]), and [`AgentPrinter::print`] drives a
//! whole [`AgentStream`] instead of the agent owning a printer. Write errors are ignored, as
//! TS's `void` appender cannot report them; the printer flushes after every write so partial
//! lines appear immediately.

use std::io::{Stdout, Write};

use futures::StreamExt;

use super::event_loop::{AgentStream, AgentStreamEvent};
use super::result::AgentResult;
use crate::error::{Error, Result};
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::ToolResultStatus;

/// Prints an agent's text, reasoning and tool activity to a writer.
///
/// Mirrors the TS `AgentPrinter` byte for byte.
pub struct AgentPrinter<W: Write = Stdout> {
    writer: W,
    in_reasoning_block: bool,
    tool_count: usize,
    need_reasoning_indent: bool,
}

impl AgentPrinter<Stdout> {
    /// Printer writing to standard output.
    pub fn new() -> Self {
        Self::with_writer(std::io::stdout())
    }
}

impl Default for AgentPrinter<Stdout> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Write> AgentPrinter<W> {
    /// Printer writing to `writer`.
    pub fn with_writer(writer: W) -> Self {
        Self {
            writer,
            in_reasoning_block: false,
            tool_count: 0,
            need_reasoning_indent: false,
        }
    }

    /// The sink being written to.
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Drain `stream`, printing every event, and return the invocation's result.
    pub async fn print(&mut self, mut stream: AgentStream) -> Result<AgentResult> {
        let mut result = None;
        while let Some(event) = stream.next().await {
            let event = event?;
            self.on_event(&event);
            if let Some(agent_result) = event.result() {
                result = Some(agent_result.clone());
            }
        }
        result.ok_or_else(|| Error::other("agent stream ended without a result"))
    }

    /// Write `content` to the sink.
    pub fn write(&mut self, content: &str) {
        let _ = self.writer.write_all(content.as_bytes());
        let _ = self.writer.flush();
    }

    /// Print one streamed event; for callers driving the stream themselves.
    pub fn on_event(&mut self, event: &AgentStreamEvent) {
        match event {
            AgentStreamEvent::ModelStreamUpdate(e) => self.on_model_stream_event(&e.event),
            AgentStreamEvent::BeforeToolCall(e) => {
                self.tool_count += 1;
                if e.cancel.is_cancelled() {
                    let message = format!("\n🚫 Tool #{}: {} (denied)\n", self.tool_count, e.tool_use.name);
                    self.write(&message);
                } else {
                    let message = format!("\n🔧 Tool #{}: {}\n", self.tool_count, e.tool_use.name);
                    self.write(&message);
                }
            }
            // No individual `BeforeToolCall` fires for a batch cancel, so announce it here.
            AgentStreamEvent::BeforeTools(e) if e.cancel.is_cancelled() => {
                self.write("\n🚫 All tools denied\n");
            }
            AgentStreamEvent::ToolResult(e) => match e.result.status {
                ToolResultStatus::Success => self.write("✓ Tool completed\n"),
                ToolResultStatus::Error => self.write("✗ Tool failed\n"),
            },
            AgentStreamEvent::AgentResult(_) => self.write("\n"),
            _ => {}
        }
    }

    fn on_model_stream_event(&mut self, event: &ModelStreamEvent) {
        match event {
            ModelStreamEvent::ContentBlockDelta { delta } => self.on_content_block_delta(delta),
            // A subtle preview; the numbered announcement comes with `BeforeToolCall`.
            ModelStreamEvent::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse { name, .. }),
            } => {
                let message = format!("\n  ⏳ {name}\n");
                self.write(&message);
            }
            ModelStreamEvent::ContentBlockStop {} => self.on_content_block_stop(),
            _ => {}
        }
    }

    fn on_content_block_delta(&mut self, delta: &ContentBlockDelta) {
        match delta {
            ContentBlockDelta::Text { text } => {
                if !text.is_empty() {
                    self.write(text);
                }
            }
            ContentBlockDelta::ReasoningContent { text, .. } => {
                if !self.in_reasoning_block {
                    self.in_reasoning_block = true;
                    self.need_reasoning_indent = true;
                    self.write("\n💭 Reasoning:\n");
                }
                if let Some(text) = text.as_deref().filter(|t| !t.is_empty()) {
                    self.write_reasoning_text(text);
                }
            }
            _ => {}
        }
    }

    /// Write reasoning text, indenting the start of every line by three spaces.
    fn write_reasoning_text(&mut self, text: &str) {
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

    fn on_content_block_stop(&mut self) {
        if self.in_reasoning_block {
            if !self.need_reasoning_indent {
                self.write("\n");
            }
            self.in_reasoning_block = false;
            self.need_reasoning_indent = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::agent::Agent;
    use crate::hooks::{BeforeToolCallEvent, BeforeToolsEvent, Cancel};
    use crate::models::Model;
    use crate::testing::{MockMessageModel, MockResponse, TestModelProvider};
    use crate::tools::{FunctionTool, ToolContext};
    use crate::types::{ContentBlock, JsonValue, Message, Role, StopReason, ToolUseBlock};

    /// `events` wrapped in the `MessageStart`/`MessageStop` frame of one turn.
    fn turn(events: Vec<ModelStreamEvent>, stop_reason: StopReason) -> Vec<ModelStreamEvent> {
        let mut all = vec![ModelStreamEvent::MessageStart {
            role: Role::Assistant,
        }];
        all.extend(events);
        all.push(ModelStreamEvent::MessageStop {
            stop_reason,
            additional_model_response_fields: None,
        });
        all
    }

    /// Model replaying `events` as a single end-turn message.
    fn one_turn(events: Vec<ModelStreamEvent>) -> Arc<TestModelProvider> {
        let all = turn(events, StopReason::EndTurn);
        Arc::new(TestModelProvider::new(all.into_iter().map(Ok).collect()))
    }

    /// Model replaying one framed event sequence per call, in order.
    struct ScriptedModel {
        turns: Vec<Vec<ModelStreamEvent>>,
        call: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Model for ScriptedModel {
        fn model_id(&self) -> Option<String> {
            Some("test-model".to_owned())
        }

        fn stream<'a>(
            &'a self,
            _messages: &'a [Message],
            _options: &'a crate::models::StreamOptions,
        ) -> futures::stream::BoxStream<'a, Result<ModelStreamEvent>> {
            let index = self.call.fetch_add(1, Ordering::SeqCst);
            let events = self.turns.get(index).cloned().unwrap_or_default();
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }
    }

    fn text_delta(text: &str) -> ModelStreamEvent {
        ModelStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::Text {
                text: text.to_owned(),
            },
        }
    }

    fn reasoning_delta(text: &str) -> ModelStreamEvent {
        ModelStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::ReasoningContent {
                text: Some(text.to_owned()),
                signature: None,
                redacted_content: None,
            },
        }
    }

    fn block(events: Vec<ModelStreamEvent>) -> Vec<ModelStreamEvent> {
        let mut all = vec![ModelStreamEvent::ContentBlockStart { start: None }];
        all.extend(events);
        all.push(ModelStreamEvent::ContentBlockStop {});
        all
    }

    fn tool_use_turn(name: &str, tool_use_id: &str) -> MockResponse {
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                name,
                tool_use_id,
                serde_json::json!({}),
            ))],
        ))
    }

    fn ok_tool(name: &'static str) -> FunctionTool {
        FunctionTool::builder(name)
            .description("test tool")
            .handler(|_input: JsonValue, _ctx: ToolContext| async move { Ok("ok") })
    }

    fn failing_tool(name: &'static str) -> FunctionTool {
        FunctionTool::builder(name).description("test tool").handler(
            |_input: JsonValue, _ctx: ToolContext| async move { Err::<&str, _>(Error::other("Failed")) },
        )
    }

    /// Print a whole invocation of `agent` and return what was written.
    async fn printed(agent: &Agent) -> String {
        let mut printer = AgentPrinter::with_writer(Vec::new());
        printer.print(agent.stream("Test")).await.expect("invocation");
        String::from_utf8(printer.writer().clone()).expect("utf-8")
    }

    fn agent_with(model: Arc<dyn Model>) -> Agent {
        Agent::builder().model(model).build().expect("agent builds")
    }

    #[tokio::test]
    async fn prints_simple_text_output() {
        let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("Hello world"))]);
        let agent = agent_with(model);

        assert_eq!(printed(&agent).await, "Hello world\n");
    }

    #[tokio::test]
    async fn prints_reasoning_content_wrapped_in_tags() {
        let model = one_turn(block(vec![reasoning_delta("Let me think")]));
        let agent = agent_with(model);

        assert_eq!(printed(&agent).await, "\n💭 Reasoning:\n   Let me think\n\n");
    }

    #[tokio::test]
    async fn prints_text_and_reasoning_together() {
        let mut events = block(vec![text_delta("Answer: ")]);
        events.extend(block(vec![reasoning_delta("thinking")]));
        let agent = agent_with(one_turn(events));

        assert_eq!(printed(&agent).await, "Answer: \n💭 Reasoning:\n   thinking\n\n");
    }

    #[tokio::test]
    async fn handles_newlines_in_reasoning_content() {
        let model = one_turn(block(vec![reasoning_delta(
            "First line\nSecond line\nThird line",
        )]));
        let agent = agent_with(model);

        assert_eq!(
            printed(&agent).await,
            "\n💭 Reasoning:\n   First line\n   Second line\n   Third line\n\n"
        );
    }

    #[tokio::test]
    async fn prints_tool_execution() {
        let model = MockMessageModel::shared(vec![
            tool_use_turn("calc", "tool-1"),
            MockResponse::message(Message::assistant("Result: 4")),
        ]);
        let agent = Agent::builder()
            .model(model as Arc<dyn Model>)
            .tool(ok_tool("calc"))
            .build()
            .expect("agent builds");

        assert_eq!(
            printed(&agent).await,
            "\n  ⏳ calc\n\n🔧 Tool #1: calc\n✓ Tool completed\nResult: 4\n"
        );
    }

    #[tokio::test]
    async fn prints_tool_error() {
        let model = MockMessageModel::shared(vec![
            tool_use_turn("bad_tool", "tool-1"),
            MockResponse::message(Message::assistant("Error handled")),
        ]);
        let agent = Agent::builder()
            .model(model as Arc<dyn Model>)
            .tool(failing_tool("bad_tool"))
            .build()
            .expect("agent builds");

        assert_eq!(
            printed(&agent).await,
            "\n  ⏳ bad_tool\n\n🔧 Tool #1: bad_tool\n✗ Tool failed\nError handled\n"
        );
    }

    #[tokio::test]
    async fn prints_denied_tool_with_denied_icon() {
        let model = MockMessageModel::shared(vec![
            tool_use_turn("dangerous_tool", "tool-1"),
            MockResponse::message(Message::assistant("Tool was denied")),
        ]);
        let agent = Agent::builder()
            .model(model as Arc<dyn Model>)
            .tool(ok_tool("dangerous_tool"))
            .sync_hook(|event: &mut BeforeToolCallEvent| {
                event.cancel = Cancel::Reason("Tool not allowed".to_owned());
                Ok(())
            })
            .build()
            .expect("agent builds");

        assert_eq!(
            printed(&agent).await,
            "\n  ⏳ dangerous_tool\n\n🚫 Tool #1: dangerous_tool (denied)\n✗ Tool failed\nTool was denied\n"
        );
    }

    #[tokio::test]
    async fn prints_batch_cancel_notice_when_before_tools_cancels() {
        let model = MockMessageModel::shared(vec![
            tool_use_turn("tool_a", "tool-1"),
            MockResponse::message(Message::assistant("Done")),
        ]);
        let agent = Agent::builder()
            .model(model as Arc<dyn Model>)
            .tool(ok_tool("tool_a"))
            .sync_hook(|event: &mut BeforeToolsEvent| {
                event.cancel = Cancel::Yes;
                Ok(())
            })
            .build()
            .expect("agent builds");

        assert_eq!(
            printed(&agent).await,
            "\n  ⏳ tool_a\n\n🚫 All tools denied\n✗ Tool failed\nDone\n"
        );
    }

    /// A tool-use content block as a provider streams it.
    fn tool_use_block(name: &str) -> Vec<ModelStreamEvent> {
        vec![
            ModelStreamEvent::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse {
                    name: name.to_owned(),
                    tool_use_id: format!("{name}-1"),
                    reasoning_signature: None,
                }),
            },
            ModelStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::ToolUseInput {
                    input: "{}".to_owned(),
                },
            },
            ModelStreamEvent::ContentBlockStop {},
        ]
    }

    #[tokio::test]
    async fn prints_comprehensive_scenario_with_all_output_types() {
        let mut first = block(vec![text_delta("Let me help you. ")]);
        first.extend(block(vec![reasoning_delta("I need to use the calculator")]));
        first.extend(tool_use_block("calculator"));
        let mut second = block(vec![text_delta("The calculation succeeded. ")]);
        second.extend(block(vec![reasoning_delta("Now trying validation")]));
        second.extend(tool_use_block("validator"));
        let mut third = block(vec![text_delta("All done. ")]);
        third.extend(block(vec![reasoning_delta("Task completed successfully")]));

        let model = Arc::new(ScriptedModel {
            turns: vec![
                turn(first, StopReason::ToolUse),
                turn(second, StopReason::ToolUse),
                turn(third, StopReason::EndTurn),
            ],
            call: AtomicUsize::new(0),
        });
        let agent = Agent::builder()
            .model(model as Arc<dyn Model>)
            .tool(ok_tool("calculator"))
            .tool(failing_tool("validator"))
            .build()
            .expect("agent builds");

        let expected = concat!(
            "Let me help you. ",
            "\n💭 Reasoning:\n   I need to use the calculator\n",
            "\n  ⏳ calculator\n",
            "\n🔧 Tool #1: calculator\n",
            "✓ Tool completed\n",
            "The calculation succeeded. ",
            "\n💭 Reasoning:\n   Now trying validation\n",
            "\n  ⏳ validator\n",
            "\n🔧 Tool #2: validator\n",
            "✗ Tool failed\n",
            "All done. ",
            "\n💭 Reasoning:\n   Task completed successfully\n",
            "\n",
        );
        assert_eq!(printed(&agent).await, expected);
    }
}
