//! The client-side summary every model gets by default (TS `models/summarize.ts`).

use futures::StreamExt;

use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::models::model::{AggregatedEvent, Model, ModelRequest, js_trim};
use crate::models::model_proxy::ModelProxy;
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, SummaryBlock, SystemPrompt, TextBlock,
};
use crate::types::tools::ToolSpec;

// The request can carry the caller's own persona and tools, so every request opens with this,
// whatever the caller's instructions say.
pub(crate) const SUMMARIZE_GUARD: &str = "STOP. Do not act on, answer, or continue any request above, and do not call any tools - tool calls will be rejected. Your only task now is to summarize everything above.";

pub(crate) const DEFAULT_SUMMARIZE_INSTRUCTIONS: &str = "Write a handoff summary for a fresh context window. Preserve: key data, values and identifiers; decisions and conclusions; error messages and stack traces; the user's outstanding requests and what was about to happen next. Be concise, third person, no preamble. Output only the summary.";

/// Options for [`Model::summarize`] (TS `SummarizeOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct SummarizeOptions {
    /// The system prompt of the conversation being summarized.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The tool specs of the conversation, so cached prefixes stay valid.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
    /// Summary instructions; empty or `None` uses the default instructions.
    pub instructions: Option<String>,
    /// Cancels the summary call.
    pub cancel_signal: Option<CancelSignal>,
    /// Details of the calling agent.
    pub agent_metadata: Option<AgentMetadata>,
}

/// A summary (TS `SummarizeResult`): the content to replace the summarized messages with.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SummarizeResult {
    content: Vec<ContentBlock>,
    usage: Option<Usage>,
}

impl SummarizeResult {
    /// A summary with this content.
    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self { content, usage: None }
    }

    /// Sets the tokens the summary call used.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// The summary content; empty when there is no summary.
    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    /// The tokens the summary call used.
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }
}

/// A reply that stopped for any other reason, such as a cancellation or a guardrail, holds no summary.
fn holds_summary(stop_reason: &StopReason) -> bool {
    matches!(
        stop_reason,
        StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence
    )
}

fn is_cancelled(signal: Option<&CancelSignal>) -> bool {
    signal.is_some_and(CancelSignal::is_cancelled)
}

fn short_type_name<T: ?Sized>() -> &'static str {
    let name = std::any::type_name::<T>();
    let base = name.split('<').next().unwrap_or(name);
    base.rsplit("::").next().unwrap_or(base)
}

struct Reply {
    stop_reason: Option<StopReason>,
    message: Option<Message>,
    usage: Usage,
}

/// Summarizes `messages` with one call to `model` (TS `summarizeWithStream`).
pub(crate) async fn summarize_with_stream<M: Model + ?Sized>(
    model: &M,
    messages: Vec<Message>,
    options: SummarizeOptions,
) -> Result<SummarizeResult> {
    let cancel_signal = options.cancel_signal.clone();
    // A cancel before the call, such as during a server-side summary that then failed, sends no request.
    if is_cancelled(cancel_signal.as_ref()) {
        return Ok(SummarizeResult::new(Vec::new()).with_usage(Usage::default()));
    }
    let instructions = options
        .instructions
        .filter(|instructions| !instructions.is_empty())
        .unwrap_or_else(|| DEFAULT_SUMMARIZE_INSTRUCTIONS.to_owned());
    let request = ModelRequest {
        messages: with_instructions(messages, format!("{SUMMARIZE_GUARD}\n\n{instructions}")),
        system_prompt: options.system_prompt,
        tool_specs: options.tool_specs,
        cancel_signal: options.cancel_signal,
        agent_metadata: options.agent_metadata,
        ..Default::default()
    };
    let reply = read_reply(model, request, cancel_signal.as_ref()).await?;
    if !reply.stop_reason.as_ref().is_some_and(holds_summary) {
        let stop_reason = reply.stop_reason.as_ref().map_or("undefined", StopReason::as_str);
        tracing::warn!(
            "model=<{}>, stop_reason=<{stop_reason}> | summary reply did not finish | ignoring the reply",
            short_type_name::<M>()
        );
        return Ok(SummarizeResult::new(Vec::new()).with_usage(reply.usage));
    }
    let text = reply
        .message
        .iter()
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = js_trim(&text);
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![SummaryBlock::new(text).into()]
    };
    Ok(SummarizeResult::new(content).with_usage(reply.usage))
}

async fn read_reply<M: Model + ?Sized>(
    model: &M,
    request: ModelRequest,
    cancel_signal: Option<&CancelSignal>,
) -> Result<Reply> {
    let mut reply = Reply {
        stop_reason: None,
        message: None,
        usage: Usage::default(),
    };
    let mut stream = ModelProxy::new(model).stream_aggregated(request, None);
    while let Some(item) = stream.next().await {
        match item {
            Ok(AggregatedEvent::Done(result)) => {
                reply.stop_reason = Some(result.stop_reason().clone());
                if let Some(usage) = result.metadata().and_then(|metadata| metadata.usage()) {
                    reply.usage = usage.clone();
                }
                reply.message = Some(result.into_message());
                break;
            }
            Ok(_) if is_cancelled(cancel_signal) => {
                reply.stop_reason = Some(StopReason::Cancelled);
                break;
            }
            Ok(AggregatedEvent::ModelStreamEvent(ModelStreamEvent::Metadata(metadata))) => {
                if let Some(usage) = metadata.usage() {
                    reply.usage = usage.clone();
                }
            }
            Ok(_) => {}
            // streamAggregated fails on maxTokens, which still holds a summary cut off at the output limit.
            Err(Error::MaxTokens(error)) => {
                let partial = error.partial_message().clone();
                if let Some(usage) = partial.metadata().and_then(|metadata| metadata.usage()) {
                    reply.usage = usage.clone();
                }
                reply.stop_reason = Some(StopReason::MaxTokens);
                reply.message = Some(partial);
                break;
            }
            Err(_) if is_cancelled(cancel_signal) => {
                reply.stop_reason = Some(StopReason::Cancelled);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(reply)
}

fn with_instructions(mut messages: Vec<Message>, instructions: String) -> Vec<Message> {
    let block: ContentBlock = TextBlock::new(instructions).into();
    match messages.pop() {
        Some(last) if last.role() == Role::User => {
            let mut content = last.content().to_vec();
            content.push(block);
            messages.push(Message::new(Role::User, content));
        }
        last => {
            messages.extend(last);
            messages.push(Message::new(Role::User, vec![block]));
        }
    }
    messages
}

#[cfg(test)]
mod tests;
