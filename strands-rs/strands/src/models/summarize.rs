//! The client-side summarizer behind the default [`Model::summarize`] (TS `models/summarize.ts`).

use futures::StreamExt;

use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::models::model::{AggregatedEvent, Model, ModelRequest, SummarizeOptions};
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::text::js_trim;
use crate::types::messages::{ContentBlock, Message, Role, StopReason, SummaryBlock, TextBlock};

/// Opens every summarize instruction, whatever the caller's instructions say, because the request
/// can carry the caller's own persona and tools (TS `SUMMARIZE_GUARD`).
pub(crate) const SUMMARIZE_GUARD: &str = "STOP. Do not act on, answer, or continue any request above, and do not call any tools - tool calls will be rejected. Your only task now is to summarize everything above.";

/// The default summarize instructions (TS `DEFAULT_SUMMARIZE_INSTRUCTIONS`).
pub(crate) const DEFAULT_SUMMARIZE_INSTRUCTIONS: &str = "Write a handoff summary for a fresh context window. Preserve: key data, values and identifiers; decisions and conclusions; error messages and stack traces; the user's outstanding requests and what was about to happen next. Be concise, third person, no preamble. Output only the summary.";

/// The result of [`Model::summarize`] (TS `SummarizeResult`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct SummarizeResult {
    /// The summary as summary blocks, empty when the model wrote none. Built-in implementations
    /// return at most one block; a caller placing one summary uses the first.
    pub content: Vec<ContentBlock>,
    /// Token usage of the calls made for the summary, `None` when an implementation reports none.
    /// The default implementation always reports usage, with zero counts when the provider sent none.
    pub usage: Option<Usage>,
}

impl SummarizeResult {
    /// Creates a result without usage.
    #[must_use]
    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self { content, usage: None }
    }

    /// Sets the usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

/// Summarizes `messages` by sending them as a request closed by one summarize instruction (TS
/// `summarizeWithStream`). The request is otherwise sent as given, so it can share the caller's
/// prompt cache.
pub(crate) async fn summarize_with_stream<M: Model + ?Sized>(
    model: &M,
    messages: Vec<Message>,
    options: SummarizeOptions,
) -> Result<SummarizeResult> {
    let cancel_signal = options.cancel_signal;
    let mut usage = Usage::default();
    // A cancel before the call, such as during a server-side summary that then failed, sends no request.
    if cancel_signal.as_ref().is_some_and(CancelSignal::is_cancelled) {
        return Ok(SummarizeResult::new(Vec::new()).with_usage(usage));
    }
    let instructions = options
        .instructions
        .filter(|instructions| !instructions.is_empty());
    let instructions = format!(
        "{SUMMARIZE_GUARD}\n\n{}",
        instructions.as_deref().unwrap_or(DEFAULT_SUMMARIZE_INSTRUCTIONS)
    );
    let request = ModelRequest {
        messages: with_instructions(messages, instructions),
        system_prompt: options.system_prompt,
        tool_specs: options.tool_specs,
        agent_metadata: options.agent_metadata,
        cancel_signal: cancel_signal.clone(),
        ..ModelRequest::default()
    };
    let is_cancelled = || cancel_signal.as_ref().is_some_and(CancelSignal::is_cancelled);

    let mut stop_reason = None;
    let mut reply = None;
    let mut stream = model.stream_aggregated(request);
    while let Some(item) = stream.next().await {
        match item {
            Ok(AggregatedEvent::Done(result)) => {
                let result = *result;
                usage = result
                    .metadata
                    .and_then(|metadata| metadata.usage)
                    .unwrap_or(usage);
                stop_reason = Some(result.stop_reason);
                reply = Some(result.message);
                break;
            }
            Ok(_) if is_cancelled() => {
                stop_reason = Some(StopReason::Cancelled);
                break;
            }
            Ok(AggregatedEvent::ModelStreamEvent(ModelStreamEvent::ModelMetadata(metadata))) => {
                usage = metadata.usage.unwrap_or(usage);
            }
            Ok(_) => {}
            // `stream_aggregated` fails on max tokens, which still holds a summary cut off at the limit.
            Err(Error::MaxTokens(error)) => {
                let partial = error.partial_message().clone();
                usage = partial
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.usage)
                    .unwrap_or(usage);
                stop_reason = Some(StopReason::MaxTokens);
                reply = Some(partial);
                break;
            }
            Err(_) if is_cancelled() => {
                stop_reason = Some(StopReason::Cancelled);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    drop(stream);

    let finished = matches!(
        stop_reason,
        Some(StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence)
    );
    if !finished {
        let model_id = model.model_id().unwrap_or_default();
        tracing::warn!("{}", unfinished_reply_warning(&model_id, stop_reason.as_ref()));
        return Ok(SummarizeResult::new(Vec::new()).with_usage(usage));
    }
    let text = reply_text(reply.as_ref());
    if text.is_empty() {
        return Ok(SummarizeResult::new(Vec::new()).with_usage(usage));
    }
    Ok(SummarizeResult::new(vec![SummaryBlock::new(text).into()]).with_usage(usage))
}

fn unfinished_reply_warning(model_id: &str, stop_reason: Option<&StopReason>) -> String {
    let stop_reason = stop_reason.map_or("undefined", StopReason::as_str);
    format!(
        "model=<{model_id}>, stop_reason=<{stop_reason}> | summary reply did not finish | ignoring the reply"
    )
}

fn reply_text(reply: Option<&Message>) -> String {
    let texts: Vec<&str> = reply
        .into_iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    js_trim(&texts.join("\n")).to_owned()
}

/// Returns `messages` with `instructions` closing the final user turn, or in a user turn after it.
fn with_instructions(mut messages: Vec<Message>, instructions: String) -> Vec<Message> {
    let block = ContentBlock::from(TextBlock::new(instructions));
    match messages.last_mut() {
        Some(last) if last.role == Role::User => {
            let mut content = std::mem::take(&mut last.content);
            content.push(block);
            *last = Message::new(Role::User, content);
        }
        _ => messages.push(Message::new(Role::User, vec![block])),
    }
    messages
}

#[cfg(test)]
mod tests;
