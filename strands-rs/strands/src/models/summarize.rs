//! Client-side summarization, the default `Model::summarize` (TS `models/summarize.ts`).

use futures::StreamExt;

use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::models::model::{AggregatedEvent, Model, ModelRequest, SummarizeOptions, SummarizeResult};
use crate::models::model_proxy;
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::types::{ContentBlock, Message, Role, StopReason, SummaryBlock, TextBlock};

/// Opens every summary request, whatever the caller's instructions say, because the request can
/// carry the caller's own persona and tools (TS `SUMMARIZE_GUARD`).
pub(crate) const SUMMARIZE_GUARD: &str = "STOP. Do not act on, answer, or continue any request above, and do not call any tools - tool calls will be rejected. Your only task now is to summarize everything above.";

/// The instructions used when the caller gives none (TS `DEFAULT_SUMMARIZE_INSTRUCTIONS`).
pub(crate) const DEFAULT_SUMMARIZE_INSTRUCTIONS: &str = "Write a handoff summary for a fresh context window. Preserve: key data, values and identifiers; decisions and conclusions; error messages and stack traces; the user's outstanding requests and what was about to happen next. Be concise, third person, no preamble. Output only the summary.";

/// A reply that stopped for any other reason, such as a cancel or a guardrail, holds no summary.
fn is_summary_stop_reason(stop_reason: &StopReason) -> bool {
    matches!(
        stop_reason,
        StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence
    )
}

/// The last segment of a type name, the counterpart of a JS `constructor.name`.
fn short_type_name<M: ?Sized>() -> &'static str {
    let name = std::any::type_name::<M>();
    let base = name.split('<').next().unwrap_or(name);
    base.rsplit("::").next().unwrap_or(base)
}

fn with_instructions(mut messages: Vec<Message>, instructions: String) -> Vec<Message> {
    let block = ContentBlock::Text(TextBlock::new(instructions));
    match messages.last_mut() {
        Some(last) if last.role == Role::User => {
            let mut content = last.content.clone();
            content.push(block);
            *last = Message::new(Role::User, content);
        }
        _ => messages.push(Message::new(Role::User, vec![block])),
    }
    messages
}

struct Reply {
    stop_reason: Option<StopReason>,
    message: Option<Message>,
    usage: Usage,
}

/// Summarizes `messages` with an ordinary model call (TS `summarizeWithStream`).
pub(crate) async fn summarize_with_stream<M: Model + ?Sized>(
    model: &M,
    messages: Vec<Message>,
    options: SummarizeOptions,
) -> Result<SummarizeResult> {
    let cancel_signal = options.cancel_signal;
    // A cancel before the call, such as during a server-side summary that then failed, sends no request.
    if cancel_signal.as_ref().is_some_and(CancelSignal::is_cancelled) {
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
        agent_metadata: options.agent_metadata,
        cancel_signal: cancel_signal.clone(),
        ..Default::default()
    };
    let is_cancelled = || cancel_signal.as_ref().is_some_and(CancelSignal::is_cancelled);
    let mut stream = model_proxy::stream_aggregated(model, request);
    let mut reply = Reply {
        stop_reason: None,
        message: None,
        usage: Usage::default(),
    };
    while let Some(item) = stream.next().await {
        match item {
            Ok(AggregatedEvent::Done(result)) => {
                let result = *result;
                reply.usage = result
                    .metadata
                    .and_then(|metadata| metadata.usage)
                    .unwrap_or(reply.usage);
                reply.stop_reason = Some(result.stop_reason);
                reply.message = Some(result.message);
                break;
            }
            Ok(_) if is_cancelled() => {
                reply.stop_reason = Some(StopReason::Cancelled);
                break;
            }
            Ok(AggregatedEvent::ModelStreamEvent(ModelStreamEvent::Metadata(metadata))) => {
                if let Some(usage) = metadata.usage {
                    reply.usage = usage;
                }
            }
            Ok(_) => {}
            // `stream_aggregated` fails on max tokens, which still holds a summary cut off at the limit.
            Err(Error::MaxTokens(error)) => {
                let partial = error.partial_message();
                reply.usage = partial
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.usage)
                    .unwrap_or(reply.usage);
                reply.stop_reason = Some(StopReason::MaxTokens);
                reply.message = Some(partial.clone());
                break;
            }
            Err(_) if is_cancelled() => {
                reply.stop_reason = Some(StopReason::Cancelled);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    drop(stream);
    let usage = reply.usage;
    if !reply.stop_reason.as_ref().is_some_and(is_summary_stop_reason) {
        let stop_reason = reply
            .stop_reason
            .as_ref()
            .map_or_else(|| "undefined".to_owned(), ToString::to_string);
        tracing::warn!(
            "model=<{}>, stop_reason=<{stop_reason}> | summary reply did not finish | ignoring the reply",
            short_type_name::<M>()
        );
        return Ok(SummarizeResult::new(Vec::new()).with_usage(usage));
    }
    let text = reply
        .message
        .map(|message| {
            message
                .content
                .into_iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Ok(SummarizeResult::new(Vec::new()).with_usage(usage));
    }
    Ok(SummarizeResult::new(vec![ContentBlock::Summary(SummaryBlock::new(text))]).with_usage(usage))
}
