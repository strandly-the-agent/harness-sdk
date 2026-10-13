//! Converse response parsing (`_mapBedrockEventToSDKEvent` and friends in `models/bedrock.ts`).

use aws_sdk_bedrockruntime::operation::converse::ConverseOutput;
use aws_sdk_bedrockruntime::types as bt;

use super::format::document_to_json;
use super::{
    BedrockGuardrailConfig, BedrockModelConfig, DEFAULT_REDACT_INPUT_MESSAGE, DEFAULT_REDACT_OUTPUT_MESSAGE,
};
use crate::error::{Error, Result};
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent, RedactInputContent,
    RedactOutputContent};
use crate::types::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, Metrics, Role, StopReason, Usage,
};

/// Bedrock raw stop reason → SDK stop reason (`STOP_REASON_MAP`).
fn stop_reason_map(raw: &str) -> Option<StopReason> {
    Some(match raw {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "content_filtered" => StopReason::ContentFiltered,
        "guardrail_intervened" => StopReason::GuardrailIntervened,
        _ => return None,
    })
}

/// `snake_case` → `camelCase`, matching the TS `snakeToCamel` regex.
fn snake_to_camel(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut upper_next = false;
    for ch in value.chars() {
        if upper_next {
            upper_next = false;
            if ch.is_ascii_lowercase() {
                out.push(ch.to_ascii_uppercase());
                continue;
            }
            out.push('_');
            out.push(ch);
            continue;
        }
        if ch == '_' {
            upper_next = true;
            continue;
        }
        out.push(ch);
    }
    if upper_next {
        out.push('_');
    }
    out
}

/// Map a raw stop reason, adjusting `endTurn` to `toolUse` when the message used a tool.
pub(crate) fn transform_stop_reason(raw: &str, output: Option<&ConverseOutput>) -> StopReason {
    let mapped = match stop_reason_map(raw) {
        Some(mapped) => mapped,
        None => {
            let camel = snake_to_camel(raw);
            tracing::warn!("stop_reason=<{raw}>, fallback=<{camel}> | unknown stop reason, converting to camelCase");
            StopReason::from(camel.as_str())
        }
    };

    if mapped == StopReason::EndTurn {
        let has_tool_use = output
            .and_then(|output| output.output())
            .and_then(|output| output.as_message().ok())
            .map(|message| {
                message
                    .content()
                    .iter()
                    .any(|block| matches!(block, bt::ContentBlock::ToolUse(_)))
            })
            .unwrap_or(false);
        if has_tool_use {
            tracing::warn!("stop_reason=<end_turn> | adjusting to tool_use due to tool use in content blocks");
            return StopReason::ToolUse;
        }
    }

    mapped
}

fn map_usage(usage: &bt::TokenUsage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens().max(0) as u64,
        output_tokens: usage.output_tokens().max(0) as u64,
        total_tokens: usage.total_tokens().max(0) as u64,
        cache_read_input_tokens: usage.cache_read_input_tokens().map(|t| t.max(0) as u64),
        cache_write_input_tokens: usage.cache_write_input_tokens().map(|t| t.max(0) as u64),
    }
}

fn map_role(role: &bt::ConversationRole) -> Result<Role> {
    match role {
        bt::ConversationRole::User => Ok(Role::User),
        bt::ConversationRole::Assistant => Ok(Role::Assistant),
        other => Err(Error::model(format!(
            "Unexpected role from Bedrock: '{}'",
            other.as_str()
        ))),
    }
}

/// Bedrock citation location → SDK citation location; `None` drops the citation.
fn map_citation_location(location: &bt::CitationLocation) -> Option<CitationLocation> {
    match location {
        bt::CitationLocation::DocumentChar(loc) => Some(CitationLocation::DocumentChar {
            document_index: loc.document_index().unwrap_or(0).max(0) as u64,
            start: loc.start().unwrap_or(0).max(0) as u64,
            end: loc.end().unwrap_or(0).max(0) as u64,
        }),
        bt::CitationLocation::DocumentPage(loc) => Some(CitationLocation::DocumentPage {
            document_index: loc.document_index().unwrap_or(0).max(0) as u64,
            start: loc.start().unwrap_or(0).max(0) as u64,
            end: loc.end().unwrap_or(0).max(0) as u64,
        }),
        bt::CitationLocation::DocumentChunk(loc) => Some(CitationLocation::DocumentChunk {
            document_index: loc.document_index().unwrap_or(0).max(0) as u64,
            start: loc.start().unwrap_or(0).max(0) as u64,
            end: loc.end().unwrap_or(0).max(0) as u64,
        }),
        bt::CitationLocation::SearchResultLocation(loc) => Some(CitationLocation::SearchResult {
            search_result_index: loc.search_result_index().unwrap_or(0).max(0) as u64,
            start: loc.start().unwrap_or(0).max(0) as u64,
            end: loc.end().unwrap_or(0).max(0) as u64,
        }),
        bt::CitationLocation::Web(loc) => Some(CitationLocation::Web {
            url: loc.url().unwrap_or_default().to_owned(),
            domain: loc.domain().filter(|d| !d.is_empty()).map(str::to_owned),
        }),
        other => {
            tracing::warn!("citation_location=<{other:?}> | unknown citation location type");
            None
        }
    }
}

fn map_citations_data(block: &bt::CitationsContentBlock) -> (Vec<Citation>, Vec<CitationGeneratedContent>) {
    let citations = block
        .citations()
        .iter()
        .filter_map(|citation| {
            let location = map_citation_location(citation.location()?)?;
            Some(Citation {
                location,
                source: citation.source().unwrap_or_default().to_owned(),
                title: citation.title().unwrap_or_default().to_owned(),
                source_content: citation
                    .source_content()
                    .iter()
                    .filter_map(|entry| entry.as_text().ok())
                    .map(|text| CitationSourceContent { text: text.clone() })
                    .collect(),
            })
        })
        .collect();
    let content = block
        .content()
        .iter()
        .filter_map(|entry| entry.as_text().ok())
        .map(|text| CitationGeneratedContent { text: text.clone() })
        .collect();
    (citations, content)
}

/// Streaming chunk → SDK events, plus the raw stop reason when the chunk carried one.
pub(crate) fn map_streamed_event(
    chunk: &bt::ConverseStreamOutput,
    last_stop_reason: Option<String>,
) -> Result<(Vec<ModelStreamEvent>, Option<String>)> {
    let mut events = Vec::new();
    let mut stop_reason = last_stop_reason;

    match chunk {
        bt::ConverseStreamOutput::MessageStart(data) => {
            events.push(ModelStreamEvent::MessageStart {
                role: map_role(data.role())?,
            });
        }
        bt::ConverseStreamOutput::ContentBlockStart(data) => {
            let start = match data.start() {
                Some(bt::ContentBlockStart::ToolUse(tool_use)) => Some(ContentBlockStart::ToolUse {
                    name: tool_use.name().to_owned(),
                    tool_use_id: tool_use.tool_use_id().to_owned(),
                    reasoning_signature: None,
                }),
                _ => None,
            };
            events.push(ModelStreamEvent::ContentBlockStart { start });
        }
        bt::ConverseStreamOutput::ContentBlockDelta(data) => {
            match data.delta() {
                Some(bt::ContentBlockDelta::Text(text)) => {
                    events.push(ModelStreamEvent::ContentBlockDelta {
                        delta: ContentBlockDelta::Text { text: text.clone() },
                    });
                }
                Some(bt::ContentBlockDelta::ToolUse(tool_use)) => {
                    let input = tool_use.input();
                    if !input.is_empty() {
                        events.push(ModelStreamEvent::ContentBlockDelta {
                            delta: ContentBlockDelta::ToolUseInput {
                                input: input.to_owned(),
                            },
                        });
                    }
                }
                Some(bt::ContentBlockDelta::ReasoningContent(reasoning)) => {
                    let (text, signature, redacted_content) = match reasoning {
                        bt::ReasoningContentBlockDelta::Text(text) => {
                            (Some(text.clone()).filter(|t| !t.is_empty()), None, None)
                        }
                        bt::ReasoningContentBlockDelta::Signature(signature) => {
                            (None, Some(signature.clone()).filter(|s| !s.is_empty()), None)
                        }
                        bt::ReasoningContentBlockDelta::RedactedContent(blob) => {
                            (None, None, Some(blob.as_ref().to_vec()).filter(|b| !b.is_empty()))
                        }
                        _ => (None, None, None),
                    };
                    if text.is_some() || signature.is_some() || redacted_content.is_some() {
                        events.push(ModelStreamEvent::ContentBlockDelta {
                            delta: ContentBlockDelta::ReasoningContent {
                                text,
                                signature,
                                redacted_content,
                            },
                        });
                    }
                }
                Some(bt::ContentBlockDelta::Citation(citation)) => {
                    if let Some(location) = citation.location().and_then(map_citation_location) {
                        events.push(ModelStreamEvent::ContentBlockDelta {
                            delta: ContentBlockDelta::Citations {
                                citations: vec![Citation {
                                    location,
                                    source_content: citation
                                        .source_content()
                                        .iter()
                                        .filter_map(|entry| entry.text())
                                        .map(|text| CitationSourceContent { text: text.to_owned() })
                                        .collect(),
                                    source: citation.source().unwrap_or_default().to_owned(),
                                    title: citation.title().unwrap_or_default().to_owned(),
                                }],
                                content: Vec::new(),
                            },
                        });
                    }
                }
                Some(other) => {
                    tracing::warn!("delta_key=<{other:?}> | skipping unsupported delta key");
                }
                None => {
                    return Err(Error::model(
                        "Expected contentBlockDelta.delta to be defined, but got undefined",
                    ))
                }
            };
        }
        bt::ConverseStreamOutput::ContentBlockStop(_) => {
            events.push(ModelStreamEvent::ContentBlockStop {});
        }
        bt::ConverseStreamOutput::MessageStop(data) => {
            let raw = data.stop_reason().as_str().to_owned();
            events.push(ModelStreamEvent::MessageStop {
                stop_reason: transform_stop_reason(&raw, None),
                additional_model_response_fields: data.additional_model_response_fields().map(document_to_json),
            });
            stop_reason = Some(raw);
        }
        bt::ConverseStreamOutput::Metadata(data) => {
            events.push(ModelStreamEvent::Metadata {
                usage: data.usage().map(map_usage),
                metrics: data.metrics().map(|metrics| Metrics {
                    latency_ms: metrics.latency_ms().max(0) as u64,
                    time_to_first_byte_ms: None,
                }),
                trace: data.trace().map(trace_to_json),
            });
        }
        other => {
            tracing::warn!("event_type=<{other:?}> | unsupported bedrock event type");
        }
    }

    Ok((events, stop_reason))
}

/// Non-streaming response → the same event sequence a stream would produce.
pub(crate) fn map_converse_output(
    config: &BedrockModelConfig,
    output: &ConverseOutput,
) -> Result<Vec<ModelStreamEvent>> {
    let mut events = Vec::new();

    let message = output
        .output()
        .ok_or_else(|| Error::model("Expected event.output to be defined, but got undefined"))?
        .as_message()
        .map_err(|_| Error::model("Expected output.message to be defined, but got undefined"))?;

    events.push(ModelStreamEvent::MessageStart {
        role: map_role(message.role())?,
    });

    for block in message.content() {
        match block {
            bt::ContentBlock::Text(text) => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                events.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::Text { text: text.clone() },
                });
                events.push(ModelStreamEvent::ContentBlockStop {});
            }
            bt::ContentBlock::ToolUse(tool_use) => {
                events.push(ModelStreamEvent::ContentBlockStart {
                    start: Some(ContentBlockStart::ToolUse {
                        name: tool_use.name().to_owned(),
                        tool_use_id: tool_use.tool_use_id().to_owned(),
                        reasoning_signature: None,
                    }),
                });
                events.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ToolUseInput {
                        input: serde_json::to_string(&document_to_json(tool_use.input()))
                            .map_err(|e| super::model_error("unable to serialize tool input", e))?,
                    },
                });
                events.push(ModelStreamEvent::ContentBlockStop {});
            }
            bt::ContentBlock::ReasoningContent(reasoning) => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                let delta = match reasoning {
                    bt::ReasoningContentBlock::ReasoningText(text) => Some(ContentBlockDelta::ReasoningContent {
                        text: Some(text.text().to_owned()),
                        signature: text.signature().filter(|s| !s.is_empty()).map(str::to_owned),
                        redacted_content: None,
                    }),
                    bt::ReasoningContentBlock::RedactedContent(blob) if !blob.as_ref().is_empty() => {
                        Some(ContentBlockDelta::ReasoningContent {
                            text: None,
                            signature: None,
                            redacted_content: Some(blob.as_ref().to_vec()),
                        })
                    }
                    _ => None,
                };
                if let Some(delta) = delta {
                    events.push(ModelStreamEvent::ContentBlockDelta { delta });
                }
                events.push(ModelStreamEvent::ContentBlockStop {});
            }
            bt::ContentBlock::CitationsContent(citations) => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                let (citations, content) = map_citations_data(citations);
                events.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::Citations { citations, content },
                });
                events.push(ModelStreamEvent::ContentBlockStop {});
            }
            other => {
                tracing::warn!("block_key=<{other:?}> | skipping unsupported block key");
            }
        }
    }

    let raw_stop_reason = output.stop_reason().as_str().to_owned();
    events.push(ModelStreamEvent::MessageStop {
        stop_reason: transform_stop_reason(&raw_stop_reason, Some(output)),
        additional_model_response_fields: None,
    });

    let usage = output
        .usage()
        .ok_or_else(|| Error::model("Expected output.usage to be defined, but got undefined"))?;
    let metadata = ModelStreamEvent::Metadata {
        usage: Some(map_usage(usage)),
        metrics: output.metrics().map(|metrics| Metrics {
            latency_ms: metrics.latency_ms().max(0) as u64,
            time_to_first_byte_ms: None,
        }),
        trace: output.trace().map(converse_trace_to_json),
    };

    if let Some(guardrail_config) = &config.guardrail_config {
        if raw_stop_reason == "guardrail_intervened" {
            let guardrail = output.trace().and_then(|trace| trace.guardrail());
            let should_redact = guardrail.map(has_blocked_guardrail_policy).unwrap_or(true);
            if should_redact {
                events.extend(generate_redaction_events(guardrail_config, guardrail));
            }
        }
    }

    events.push(metadata);
    Ok(events)
}

/// `true` when any assessment reports `action = BLOCKED` with `detected = true`.
pub(crate) fn has_blocked_guardrail_policy(trace: &bt::GuardrailTraceAssessment) -> bool {
    fn blocked(action: &str, detected: Option<bool>) -> bool {
        action == "BLOCKED" && detected == Some(true)
    }

    fn assessment_blocked(assessment: &bt::GuardrailAssessment) -> bool {
        if let Some(topics) = assessment.topic_policy() {
            if topics
                .topics()
                .iter()
                .any(|topic| blocked(topic.action().as_str(), topic.detected()))
            {
                return true;
            }
        }
        if let Some(content) = assessment.content_policy() {
            if content
                .filters()
                .iter()
                .any(|filter| blocked(filter.action().as_str(), filter.detected()))
            {
                return true;
            }
        }
        if let Some(words) = assessment.word_policy() {
            if words
                .custom_words()
                .iter()
                .any(|word| blocked(word.action().as_str(), word.detected()))
                || words
                    .managed_word_lists()
                    .iter()
                    .any(|word| blocked(word.action().as_str(), word.detected()))
            {
                return true;
            }
        }
        if let Some(sensitive) = assessment.sensitive_information_policy() {
            if sensitive
                .pii_entities()
                .iter()
                .any(|entity| blocked(entity.action().as_str(), entity.detected()))
                || sensitive
                    .regexes()
                    .iter()
                    .any(|regex| blocked(regex.action().as_str(), regex.detected()))
            {
                return true;
            }
        }
        if let Some(grounding) = assessment.contextual_grounding_policy() {
            if grounding
                .filters()
                .iter()
                .any(|filter| blocked(filter.action().as_str(), filter.detected()))
            {
                return true;
            }
        }
        false
    }

    trace
        .input_assessment()
        .map(|assessments| assessments.values().any(assessment_blocked))
        .unwrap_or(false)
        || trace
            .output_assessments()
            .map(|assessments| assessments.values().flatten().any(assessment_blocked))
            .unwrap_or(false)
}

/// Redaction events for a blocked guardrail (`_generateRedactionEvents`).
pub(crate) fn generate_redaction_events(
    guardrail_config: &BedrockGuardrailConfig,
    trace: Option<&bt::GuardrailTraceAssessment>,
) -> Vec<ModelStreamEvent> {
    let mut events = Vec::new();
    let redaction = guardrail_config.redaction.as_ref();

    if redaction.and_then(|r| r.input) != Some(false) {
        tracing::debug!("redacting input due to guardrail");
        events.push(ModelStreamEvent::Redaction {
            input_redaction: Some(RedactInputContent {
                replace_content: redaction
                    .and_then(|r| r.input_message.clone())
                    .unwrap_or_else(|| DEFAULT_REDACT_INPUT_MESSAGE.to_owned()),
            }),
            output_redaction: None,
        });
    }

    if redaction.and_then(|r| r.output) == Some(true) {
        tracing::debug!("redacting output due to guardrail");
        let model_output = trace
            .map(|trace| trace.model_output())
            .filter(|output| !output.is_empty())
            .map(|output| output.concat());
        events.push(ModelStreamEvent::Redaction {
            input_redaction: None,
            output_redaction: Some(RedactOutputContent {
                redacted_content: model_output,
                replace_content: redaction
                    .and_then(|r| r.output_message.clone())
                    .unwrap_or_else(|| DEFAULT_REDACT_OUTPUT_MESSAGE.to_owned()),
            }),
        });
    }

    events
}

fn trace_to_json(trace: &bt::ConverseStreamTrace) -> crate::types::JsonValue {
    serde_json::json!({ "guardrail": trace.guardrail().map(guardrail_trace_to_json) })
}

fn converse_trace_to_json(trace: &bt::ConverseTrace) -> crate::types::JsonValue {
    serde_json::json!({ "guardrail": trace.guardrail().map(guardrail_trace_to_json) })
}

/// Minimal JSON view of a guardrail trace, enough for consumers to log it.
fn guardrail_trace_to_json(trace: &bt::GuardrailTraceAssessment) -> crate::types::JsonValue {
    serde_json::json!({
        "modelOutput": trace.model_output(),
        "actionReason": trace.action_reason(),
        "blocked": has_blocked_guardrail_policy(trace),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_to_camel_matches_ts_regex() {
        assert_eq!(snake_to_camel("model_context_window_exceeded"), "modelContextWindowExceeded");
        assert_eq!(snake_to_camel("new_stop_reason"), "newStopReason");
        assert_eq!(snake_to_camel("plain"), "plain");
        assert_eq!(snake_to_camel("_leading"), "Leading");
    }
}
