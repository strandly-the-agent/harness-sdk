//! `stream_aggregated` (TS `Model.streamAggregated`): collects stream events into content blocks
//! and the final message.

use futures::StreamExt;

use super::{AggregatedEvent, BoxStream, Redaction, StreamAggregatedResult};
use crate::error::{Error, MaxTokensError, ModelError, Result};
use crate::models::streaming::{
    ContentBlockDelta, ContentBlockStart, ModelMetadataEventData, ModelStreamEvent, ReasoningContentDelta,
};
use crate::text::js_trim;
use crate::types::citations::{Citation, CitationGeneratedContent, CitationsBlock};
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, Message, MessageMetadata, ReasoningBlock, Role, StopReason, TextBlock, ToolUseBlock,
};

const MAX_TOKENS_MESSAGE: &str =
    "Model reached maximum token limit. This is an unrecoverable state that requires intervention.";

/// Aggregates a model's event stream (the body of the default [`super::Model::stream_aggregated`]).
pub(crate) fn aggregate(
    mut events: BoxStream<'static, Result<ModelStreamEvent>>,
) -> BoxStream<'static, Result<AggregatedEvent>> {
    Box::pin(async_stream::stream! {
        let mut aggregator = Aggregator::default();
        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    yield Err(into_model_error(error));
                    return;
                }
            };
            let block = aggregator.apply(&event);
            yield Ok(AggregatedEvent::ModelStreamEvent(event));
            if let Some(block) = block {
                yield Ok(AggregatedEvent::ContentBlock(block));
            }
        }
        yield aggregator.finish().map(|result| AggregatedEvent::Done(Box::new(result)));
    })
}

/// Keeps model errors as they are and wraps anything else in a [`ModelError`] carrying it.
fn into_model_error(error: Error) -> Error {
    if error.is_model_error() {
        return error;
    }
    ModelError::new(error.to_string()).with_source(error).into()
}

#[derive(Debug, Default)]
struct ToolUseState {
    name: String,
    tool_use_id: String,
    reasoning_signature: String,
}

#[derive(Debug, Default)]
struct Aggregator {
    role: Option<Role>,
    blocks: Vec<ContentBlock>,
    text: String,
    tool_input: String,
    tool_use: ToolUseState,
    reasoning: ReasoningBlock,
    citations: Vec<Citation>,
    citation_content: Vec<CitationGeneratedContent>,
    stopped: Option<(Message, StopReason)>,
    metadata: Option<ModelMetadataEventData>,
    redaction: Option<String>,
    tool_input_error: Option<serde_json::Error>,
}

impl Aggregator {
    /// Applies one event, returning the content block it completed.
    fn apply(&mut self, event: &ModelStreamEvent) -> Option<ContentBlock> {
        match event {
            ModelStreamEvent::ModelMessageStart(start) => {
                self.role = Some(start.role);
                self.blocks.clear();
            }
            ModelStreamEvent::ModelContentBlockStart(start) => self.start_block(start.start.as_ref()),
            ModelStreamEvent::ModelContentBlockDelta(delta) => self.apply_delta(&delta.delta),
            ModelStreamEvent::ModelContentBlockStop(_) => return self.stop_block(),
            ModelStreamEvent::ModelMessageStop(stop) => {
                let content = self
                    .blocks
                    .iter()
                    .filter(|block| !is_blank_text(block))
                    .cloned()
                    .collect();
                self.stopped = Some((Message::new(self.role(), content), stop.stop_reason.clone()));
            }
            ModelStreamEvent::ModelMetadata(metadata) => self.metadata = Some(metadata.clone()),
            ModelStreamEvent::ModelRedaction(redaction) => {
                if let Some(input) = &redaction.input_redaction {
                    self.redaction = Some(input.replace_content.clone());
                }
                if let Some(output) = &redaction.output_redaction {
                    let message = Message::new(
                        Role::Assistant,
                        vec![TextBlock::new(&output.replace_content).into()],
                    );
                    let stop_reason = self.stopped.take().map(|(_, stop_reason)| stop_reason);
                    self.stopped = stop_reason.map(|stop_reason| (message, stop_reason));
                }
            }
        }
        None
    }

    /// Providers can end an empty turn with only a stop event, so the role defaults to assistant.
    fn role(&self) -> Role {
        self.role.unwrap_or(Role::Assistant)
    }

    fn start_block(&mut self, start: Option<&ContentBlockStart>) {
        if let Some(ContentBlockStart::ToolUse(tool_use)) = start {
            self.tool_use = ToolUseState {
                name: tool_use.name.clone(),
                tool_use_id: tool_use.tool_use_id.clone(),
                reasoning_signature: tool_use.reasoning_signature.clone().unwrap_or_default(),
            };
        }
        self.tool_input.clear();
        self.text.clear();
        self.reasoning = ReasoningBlock::default();
        self.clear_citations();
    }

    fn apply_delta(&mut self, delta: &ContentBlockDelta) {
        match delta {
            ContentBlockDelta::Text(delta) => self.text.push_str(&delta.text),
            ContentBlockDelta::ToolUseInput(delta) => self.tool_input.push_str(&delta.input),
            ContentBlockDelta::ReasoningContent(delta) => self.apply_reasoning(delta),
            ContentBlockDelta::Citations(delta) => {
                self.citations.extend(delta.citations.iter().cloned());
                self.citation_content.extend(delta.content.iter().cloned());
            }
        }
    }

    fn apply_reasoning(&mut self, delta: &ReasoningContentDelta) {
        if let Some(text) = delta.text.as_deref().filter(|text| !text.is_empty()) {
            self.reasoning.text.get_or_insert_default().push_str(text);
        }
        if let Some(signature) = delta
            .signature
            .as_deref()
            .filter(|signature| !signature.is_empty())
        {
            self.reasoning.signature = Some(signature.to_owned());
        }
        if let Some(redacted_content) = &delta.redacted_content {
            self.reasoning.redacted_content = Some(redacted_content.clone());
        }
    }

    fn stop_block(&mut self) -> Option<ContentBlock> {
        let block = if self.tool_use.tool_use_id.is_empty() {
            self.finish_content_block()
        } else {
            self.finish_tool_use()?
        };
        self.blocks.push(block.clone());
        Some(block)
    }

    /// Builds the tool use block. Unparsable input is logged and recorded, and the block is
    /// dropped with the tool state left in place, as TS does.
    fn finish_tool_use(&mut self) -> Option<ContentBlock> {
        let input = if self.tool_input.is_empty() {
            JsonValue::Object(serde_json::Map::new())
        } else {
            match serde_json::from_str(&self.tool_input) {
                Ok(input) => input,
                Err(error) => {
                    tracing::error!("error=<{error}> | unable to parse tool input JSON");
                    self.tool_input_error = Some(error);
                    return None;
                }
            }
        };
        let tool_use = std::mem::take(&mut self.tool_use);
        let mut block = ToolUseBlock::new(tool_use.name, tool_use.tool_use_id, input);
        if !tool_use.reasoning_signature.is_empty() {
            block = block.with_reasoning_signature(tool_use.reasoning_signature);
        }
        Some(block.into())
    }

    fn finish_content_block(&mut self) -> ContentBlock {
        if self.reasoning != ReasoningBlock::default() {
            return std::mem::take(&mut self.reasoning).into();
        }
        let text = std::mem::take(&mut self.text);
        if self.citations.is_empty() {
            return TextBlock::new(text).into();
        }
        let mut content = std::mem::take(&mut self.citation_content);
        // Citations deltas that ground already-streamed text carry no content of their own.
        if content.is_empty() && !text.is_empty() {
            content.push(CitationGeneratedContent::new(text));
        }
        let block = CitationsBlock::new(std::mem::take(&mut self.citations), content);
        block.into()
    }

    fn clear_citations(&mut self) {
        self.citations.clear();
        self.citation_content.clear();
    }

    fn finish(self) -> Result<StreamAggregatedResult> {
        let Some((mut message, stop_reason)) = self.stopped else {
            let error = ModelError::new("Stream ended without completing a message");
            return Err(match self.tool_input_error {
                Some(cause) => error.with_source(cause),
                None => error,
            }
            .into());
        };
        if let Some(metadata) = message_metadata(self.metadata.as_ref()) {
            message = message.with_metadata(metadata);
        }
        if stop_reason == StopReason::MaxTokens {
            return Err(MaxTokensError::new(MAX_TOKENS_MESSAGE, message).into());
        }
        if let Some(cause) = self.tool_input_error {
            return Err(ModelError::new("unable to parse tool input JSON")
                .with_source(cause)
                .into());
        }
        Ok(StreamAggregatedResult {
            message,
            stop_reason,
            metadata: self.metadata,
            redaction: self.redaction.map(Redaction::new),
        })
    }
}

fn is_blank_text(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Text(text) if js_trim(&text.text).is_empty())
}

fn message_metadata(metadata: Option<&ModelMetadataEventData>) -> Option<MessageMetadata> {
    let metadata = metadata?;
    if metadata.usage.is_none() && metadata.metrics.is_none() {
        return None;
    }
    Some(MessageMetadata {
        usage: metadata.usage,
        metrics: metadata.metrics,
        custom: None,
    })
}
