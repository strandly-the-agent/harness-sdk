//! Stream aggregation for `Model::stream_aggregated` (TS `Model.streamAggregated`).

use futures::StreamExt;

use crate::error::{Error, MaxTokensError, ModelError, Result};
use crate::models::model::{AggregatedEvent, BoxStream, Redaction, StreamAggregatedResult};
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelMetadataEvent, ModelStreamEvent};
use crate::types::json::{self, JsonValue};
use crate::types::{
    Citation, CitationGeneratedContent, CitationsBlock, ContentBlock, Message, MessageMetadata,
    ReasoningBlock, Role, StopReason, TextBlock, ToolUseBlock,
};

const MAX_TOKENS_MESSAGE: &str =
    "Model reached maximum token limit. This is an unrecoverable state that requires intervention.";
const INCOMPLETE_MESSAGE: &str = "Stream ended without completing a message";
const TOOL_INPUT_MESSAGE: &str = "unable to parse tool input JSON";

#[derive(Default)]
struct Reasoning {
    text: Option<String>,
    signature: Option<String>,
    redacted_content: Option<Vec<u8>>,
}

impl Reasoning {
    fn is_empty(&self) -> bool {
        self.text.is_none() && self.signature.is_none() && self.redacted_content.is_none()
    }

    fn into_block(self) -> ReasoningBlock {
        let mut block = ReasoningBlock::new();
        block.text = self.text;
        block.signature = self.signature;
        block.redacted_content = self.redacted_content;
        block
    }
}

/// The accumulation state of TS `streamAggregated`, field for field.
struct Aggregator {
    role: Role,
    blocks: Vec<ContentBlock>,
    text: String,
    tool_input: String,
    tool_name: String,
    tool_use_id: String,
    tool_reasoning_signature: String,
    reasoning: Reasoning,
    citations: Vec<Citation>,
    citation_content: Vec<CitationGeneratedContent>,
    stopped: Option<Message>,
    stop_reason: Option<StopReason>,
    metadata: Option<ModelMetadataEvent>,
    redaction: Option<String>,
    tool_input_error: Option<serde_json::Error>,
}

impl Aggregator {
    fn new() -> Self {
        Self {
            role: Role::Assistant,
            blocks: Vec::new(),
            text: String::new(),
            tool_input: String::new(),
            tool_name: String::new(),
            tool_use_id: String::new(),
            tool_reasoning_signature: String::new(),
            reasoning: Reasoning::default(),
            citations: Vec::new(),
            citation_content: Vec::new(),
            stopped: None,
            stop_reason: None,
            metadata: None,
            redaction: None,
            tool_input_error: None,
        }
    }

    /// Applies one event, returning the content block it completes.
    fn apply(&mut self, event: &ModelStreamEvent) -> Option<ContentBlock> {
        match event {
            ModelStreamEvent::MessageStart(start) => {
                self.role = start.role;
                self.blocks.clear();
            }
            ModelStreamEvent::BlockStart(start) => self.start_block(start.start.as_ref()),
            ModelStreamEvent::BlockDelta(delta) => self.apply_delta(&delta.delta),
            ModelStreamEvent::BlockStop(_) => return self.stop_block(),
            ModelStreamEvent::MessageStop(stop) => {
                let content = self
                    .blocks
                    .iter()
                    .filter(|block| !matches!(block, ContentBlock::Text(text) if text.text.trim().is_empty()))
                    .cloned()
                    .collect();
                self.stopped = Some(Message::new(self.role, content));
                self.stop_reason = Some(stop.stop_reason.clone());
            }
            ModelStreamEvent::Metadata(metadata) => self.metadata = Some(metadata.clone()),
            ModelStreamEvent::Redaction(redaction) => {
                if let Some(input) = &redaction.input_redaction {
                    self.redaction = Some(input.replace_content.clone());
                }
                if let Some(output) = &redaction.output_redaction {
                    let replacement = ContentBlock::Text(TextBlock::new(output.replace_content.clone()));
                    self.stopped = Some(Message::new(Role::Assistant, vec![replacement]));
                }
            }
        }
        None
    }

    fn start_block(&mut self, start: Option<&ContentBlockStart>) {
        if let Some(ContentBlockStart::ToolUse(tool)) = start {
            self.tool_name.clone_from(&tool.name);
            self.tool_use_id.clone_from(&tool.tool_use_id);
            self.tool_reasoning_signature = tool.reasoning_signature.clone().unwrap_or_default();
        }
        self.tool_input.clear();
        self.text.clear();
        self.reasoning = Reasoning::default();
        self.citations.clear();
        self.citation_content.clear();
    }

    fn apply_delta(&mut self, delta: &ContentBlockDelta) {
        match delta {
            ContentBlockDelta::Text(delta) => self.text.push_str(&delta.text),
            ContentBlockDelta::ToolUseInput(delta) => self.tool_input.push_str(&delta.input),
            ContentBlockDelta::ReasoningContent(delta) => {
                if let Some(text) = delta.text.as_deref().filter(|text| !text.is_empty()) {
                    self.reasoning.text.get_or_insert_with(String::new).push_str(text);
                }
                if let Some(signature) = delta
                    .signature
                    .as_deref()
                    .filter(|signature| !signature.is_empty())
                {
                    self.reasoning.signature = Some(signature.to_owned());
                }
                if let Some(redacted) = &delta.redacted_content {
                    self.reasoning.redacted_content = Some(redacted.clone());
                }
            }
            ContentBlockDelta::Citations(delta) => {
                self.citations.extend(delta.citations.iter().cloned());
                self.citation_content.extend(delta.content.iter().cloned());
            }
        }
    }

    fn stop_block(&mut self) -> Option<ContentBlock> {
        let block = if self.tool_use_id.is_empty() {
            self.stop_non_tool_block()
        } else {
            self.stop_tool_block()?
        };
        self.blocks.push(block.clone());
        Some(block)
    }

    /// A tool use block, or `None` when its input isn't valid JSON. On failure the tool fields
    /// stay set, as in TS, where the parse throws before they reset.
    fn stop_tool_block(&mut self) -> Option<ContentBlock> {
        let input = if self.tool_input.is_empty() {
            JsonValue::Object(serde_json::Map::new())
        } else {
            match json::from_slice::<JsonValue>(self.tool_input.as_bytes()) {
                Ok(input) => input,
                Err(error) => {
                    tracing::error!(error = %error, "unable to parse tool input JSON");
                    self.tool_input_error = Some(error);
                    return None;
                }
            }
        };
        let mut block = ToolUseBlock::new(
            std::mem::take(&mut self.tool_name),
            std::mem::take(&mut self.tool_use_id),
            input,
        );
        let signature = std::mem::take(&mut self.tool_reasoning_signature);
        if !signature.is_empty() {
            block = block.with_reasoning_signature(signature);
        }
        Some(ContentBlock::ToolUse(block))
    }

    fn stop_non_tool_block(&mut self) -> ContentBlock {
        if !self.reasoning.is_empty() {
            return ContentBlock::Reasoning(std::mem::take(&mut self.reasoning).into_block());
        }
        if !self.citations.is_empty() {
            let text = std::mem::take(&mut self.text);
            let mut content = std::mem::take(&mut self.citation_content);
            if content.is_empty() && !text.is_empty() {
                content.push(CitationGeneratedContent::new(text));
            }
            return ContentBlock::Citations(CitationsBlock::new(
                std::mem::take(&mut self.citations),
                content,
            ));
        }
        ContentBlock::Text(TextBlock::new(std::mem::take(&mut self.text)))
    }

    fn finish(self) -> Result<StreamAggregatedResult> {
        let (Some(stopped), Some(stop_reason)) = (self.stopped, self.stop_reason) else {
            let error = ModelError::new(INCOMPLETE_MESSAGE);
            return Err(match self.tool_input_error {
                Some(cause) => error.with_source(cause),
                None => error,
            }
            .into());
        };
        let message = with_metadata(stopped, self.metadata.as_ref());
        if stop_reason == StopReason::MaxTokens {
            return Err(MaxTokensError::new(MAX_TOKENS_MESSAGE, message).into());
        }
        if let Some(cause) = self.tool_input_error {
            return Err(ModelError::new(TOOL_INPUT_MESSAGE).with_source(cause).into());
        }
        let mut result = StreamAggregatedResult::new(message, stop_reason);
        result.metadata = self.metadata;
        result.redaction = self.redaction.map(Redaction::new);
        Ok(result)
    }
}

/// Puts the usage and metrics of `metadata` on the message, when there are any.
fn with_metadata(message: Message, metadata: Option<&ModelMetadataEvent>) -> Message {
    let usage = metadata.and_then(|metadata| metadata.usage);
    let metrics = metadata.and_then(|metadata| metadata.metrics);
    if usage.is_none() && metrics.is_none() {
        return message;
    }
    let mut message_metadata = MessageMetadata::new();
    message_metadata.usage = usage;
    message_metadata.metrics = metrics;
    Message::new(message.role, message.content).with_metadata(message_metadata)
}

/// Wraps an error that isn't in the `ModelError` family in a `ModelError` with it as the source.
fn as_model_error(error: Error) -> Error {
    if error.is_model_error() {
        return error;
    }
    ModelError::new(error.to_string()).with_source(error).into()
}

/// Passes every event of `events` through, adds each completed content block, and ends with the
/// aggregated result or the aggregation error.
pub(crate) fn aggregate(
    mut events: BoxStream<'static, Result<ModelStreamEvent>>,
) -> BoxStream<'static, Result<AggregatedEvent>> {
    Box::pin(async_stream::stream! {
        let mut aggregator = Aggregator::new();
        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    yield Err(as_model_error(error));
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
