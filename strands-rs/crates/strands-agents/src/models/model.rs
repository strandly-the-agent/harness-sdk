//! The `Model` trait and stream aggregation (`strands-ts/src/models/model.ts`).

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::error::{Error, Result};
use crate::types::{
    CitationsBlock, ContentBlock, Message, MessageMetadata, Metrics, ReasoningBlock, Role, StopReason,
    SystemContentBlock, SystemPrompt, ToolChoice, ToolResultContent, ToolSpec, ToolUseBlock, Usage,
};

/// Fallback context window used by `estimate_utilization` when a model has no known limit.
pub const DEFAULT_CONTEXT_WINDOW_LIMIT: u64 = 200_000;

/// Metadata about the invoking agent, passed to providers (`AgentMetadata` in TS).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentMetadata {
    /// Session id when a session manager is attached.
    pub session_id: Option<String>,
}

/// Per-call options for [`Model::stream`].
#[derive(Clone, Debug, Default)]
pub struct StreamOptions {
    /// Cancellation token for the request.
    pub cancel: Option<CancellationToken>,
    /// System prompt.
    pub system_prompt: Option<SystemPrompt>,
    /// Tools the model may call.
    pub tool_specs: Vec<ToolSpec>,
    /// Tool choice constraint.
    pub tool_choice: Option<ToolChoice>,
    /// Number of trailing content blocks that may still change (cache-point placement).
    pub dynamic_trailing_blocks: Option<usize>,
    /// Invoking-agent metadata.
    pub agent_metadata: Option<AgentMetadata>,
}

/// Options for [`Model::count_tokens`].
#[derive(Clone, Debug, Default)]
pub struct CountTokensOptions {
    /// System prompt.
    pub system_prompt: Option<SystemPrompt>,
    /// Tool specs.
    pub tool_specs: Vec<ToolSpec>,
}

/// Guardrail redaction of the user's input.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Redaction {
    /// Replacement text for the user message.
    pub user_message: String,
}

/// Final value of [`Model::stream_aggregated`].
#[derive(Clone, Debug, PartialEq)]
pub struct StreamAggregatedResult {
    /// The complete assistant message.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// Usage from the last metadata event, if any.
    pub usage: Option<Usage>,
    /// Metrics from the last metadata event, if any.
    pub metrics: Option<Metrics>,
    /// Input redaction, if the provider redacted the prompt.
    pub redaction: Option<Redaction>,
}

/// Items yielded by [`Model::stream_aggregated`]; `Result` is always the last item.
#[derive(Clone, Debug, PartialEq)]
pub enum AggregatedItem {
    /// A raw provider event, passed through.
    Event(ModelStreamEvent),
    /// A fully assembled content block.
    ContentBlock(ContentBlock),
    /// The final aggregated result.
    Result(StreamAggregatedResult),
}

/// A model provider. Object-safe: agents hold `Arc<dyn Model>`.
#[async_trait]
pub trait Model: Send + Sync {
    /// Provider model id, if configured.
    fn model_id(&self) -> Option<String>;

    /// Context window size in tokens, if known.
    fn context_window_limit(&self) -> Option<u64> {
        None
    }

    /// Whether the provider keeps conversation state server-side.
    fn stateful(&self) -> bool {
        false
    }

    /// Stream raw events for a request.
    fn stream<'a>(
        &'a self,
        messages: &'a [Message],
        options: &'a StreamOptions,
    ) -> BoxStream<'a, Result<ModelStreamEvent>>;

    /// Count prompt tokens; defaults to a character heuristic.
    async fn count_tokens(&self, messages: &[Message], options: &CountTokensOptions) -> Result<u64> {
        Ok(estimate_tokens_heuristic(messages, options))
    }

    /// `input_tokens / context_window_limit`, warning once when the limit is unknown.
    fn estimate_utilization(&self, input_tokens: u64) -> f64 {
        let limit = self.context_window_limit().unwrap_or_else(|| {
            crate::logging::warn_once(&format!(
                "contextWindowLimit is not set on the model, using default of {DEFAULT_CONTEXT_WINDOW_LIMIT} for utilization estimate | set contextWindowLimit in your model config for accurate results"
            ));
            DEFAULT_CONTEXT_WINDOW_LIMIT
        });
        input_tokens as f64 / limit as f64
    }

    /// Stream events and assembled content blocks, ending with [`AggregatedItem::Result`].
    fn stream_aggregated<'a>(
        &'a self,
        messages: &'a [Message],
        options: &'a StreamOptions,
    ) -> BoxStream<'a, Result<AggregatedItem>> {
        stream_aggregated(self.stream(messages, options))
    }
}

#[derive(Default)]
struct Aggregator {
    role: Option<Role>,
    blocks: Vec<ContentBlock>,
    text: String,
    tool_input: String,
    tool_name: String,
    tool_use_id: String,
    tool_signature: String,
    reasoning: Option<ReasoningBlock>,
    citations: CitationsBlock,
    stopped: Option<Message>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    metrics: Option<Metrics>,
    redaction: Option<String>,
    parse_error: Option<serde_json::Error>,
}

impl Aggregator {
    fn reset_block(&mut self) {
        self.tool_input.clear();
        self.text.clear();
        self.reasoning = None;
        self.citations = CitationsBlock::default();
    }

    /// Returns a finished block for a stop event, or `None` when tool input failed to parse.
    fn finish_block(&mut self) -> Option<ContentBlock> {
        if !self.tool_use_id.is_empty() {
            let input = if self.tool_input.is_empty() {
                Ok(serde_json::json!({}))
            } else {
                serde_json::from_str(&self.tool_input)
            };
            let block = match input {
                Ok(input) => {
                    let mut tu = ToolUseBlock::new(self.tool_name.clone(), self.tool_use_id.clone(), input);
                    if !self.tool_signature.is_empty() {
                        tu.reasoning_signature = Some(self.tool_signature.clone());
                    }
                    Some(ContentBlock::ToolUse(tu))
                }
                Err(e) => {
                    tracing::error!(error = %e, "unable to parse tool input JSON");
                    self.parse_error = Some(e);
                    None
                }
            };
            self.tool_name.clear();
            self.tool_use_id.clear();
            self.tool_signature.clear();
            return block;
        }
        if let Some(r) = self.reasoning.take() {
            return Some(ContentBlock::Reasoning(r));
        }
        if !self.citations.citations.is_empty() {
            let mut cb = std::mem::take(&mut self.citations);
            if cb.content.is_empty() && !self.text.is_empty() {
                cb.content = vec![crate::types::CitationGeneratedContent {
                    text: self.text.clone(),
                }];
            }
            return Some(ContentBlock::Citations(cb));
        }
        Some(ContentBlock::Text(self.text.clone()))
    }

    fn apply(&mut self, event: &ModelStreamEvent) -> Option<ContentBlock> {
        match event {
            ModelStreamEvent::MessageStart { role } => {
                self.role = Some(*role);
                self.blocks.clear();
            }
            ModelStreamEvent::ContentBlockStart { start } => {
                if let Some(ContentBlockStart::ToolUse {
                    name,
                    tool_use_id,
                    reasoning_signature,
                }) = start
                {
                    self.tool_name = name.clone();
                    self.tool_use_id = tool_use_id.clone();
                    self.tool_signature = reasoning_signature.clone().unwrap_or_default();
                }
                self.reset_block();
            }
            ModelStreamEvent::ContentBlockDelta { delta } => match delta {
                ContentBlockDelta::Text { text } => self.text.push_str(text),
                ContentBlockDelta::ToolUseInput { input } => self.tool_input.push_str(input),
                ContentBlockDelta::ReasoningContent {
                    text,
                    signature,
                    redacted_content,
                } => {
                    let r = self.reasoning.get_or_insert_with(ReasoningBlock::default);
                    if let Some(t) = text.as_deref().filter(|t| !t.is_empty()) {
                        r.text.get_or_insert_with(String::new).push_str(t);
                    }
                    if let Some(s) = signature.as_deref().filter(|s| !s.is_empty()) {
                        r.signature = Some(s.to_owned());
                    }
                    if let Some(b) = redacted_content.as_ref().filter(|b| !b.is_empty()) {
                        r.redacted_content = Some(b.clone());
                    }
                    if r.text.is_none() && r.signature.is_none() && r.redacted_content.is_none() {
                        self.reasoning = None;
                    }
                }
                ContentBlockDelta::Citations { citations, content } => {
                    self.citations.citations.extend(citations.iter().cloned());
                    self.citations.content.extend(content.iter().cloned());
                }
            },
            ModelStreamEvent::ContentBlockStop {} => {
                let block = self.finish_block()?;
                self.blocks.push(block.clone());
                return Some(block);
            }
            ModelStreamEvent::MessageStop { stop_reason, .. } => {
                if let Some(role) = self.role {
                    let content = self
                        .blocks
                        .iter()
                        .filter(|b| !matches!(b, ContentBlock::Text(t) if t.trim().is_empty()))
                        .cloned()
                        .collect();
                    self.stopped = Some(Message::new(role, content));
                    self.stop_reason = Some(stop_reason.clone());
                }
            }
            ModelStreamEvent::Metadata { usage, metrics, .. } => {
                self.usage = usage.clone();
                self.metrics = metrics.clone();
            }
            ModelStreamEvent::Redaction {
                input_redaction,
                output_redaction,
            } => {
                if let Some(i) = input_redaction {
                    self.redaction = Some(i.replace_content.clone());
                }
                if let Some(o) = output_redaction {
                    self.stopped = Some(Message::assistant(o.replace_content.clone()));
                }
            }
        }
        None
    }

    fn finish(self) -> Result<StreamAggregatedResult> {
        let (Some(mut message), Some(stop_reason)) = (self.stopped, self.stop_reason) else {
            return Err(Error::Model {
                message: "Stream ended without completing a message".into(),
                source: self.parse_error.map(|e| Box::new(e) as _),
            });
        };
        if self.usage.is_some() || self.metrics.is_some() {
            message = Message::new(message.role, message.content).with_metadata(MessageMetadata {
                usage: self.usage.clone(),
                metrics: self.metrics.clone(),
                custom: None,
            });
        }
        if stop_reason == StopReason::MaxTokens {
            return Err(Error::MaxTokens {
                message: "Model reached maximum token limit. This is an unrecoverable state that requires intervention.".into(),
                partial_message: Box::new(message),
            });
        }
        if let Some(e) = self.parse_error {
            return Err(Error::Model {
                message: "unable to parse tool input JSON".into(),
                source: Some(Box::new(e)),
            });
        }
        Ok(StreamAggregatedResult {
            message,
            stop_reason,
            usage: self.usage,
            metrics: self.metrics,
            redaction: self.redaction.map(|user_message| Redaction { user_message }),
        })
    }
}

/// Aggregate a raw event stream into content blocks and a final message.
pub fn stream_aggregated<'a>(
    mut events: BoxStream<'a, Result<ModelStreamEvent>>,
) -> BoxStream<'a, Result<AggregatedItem>> {
    Box::pin(async_stream::stream! {
        let mut agg = Aggregator::default();
        while let Some(event) = events.next().await {
            let event = match event {
                Ok(e) => e,
                Err(e) if e.is_model_error() => { yield Err(e); return; }
                Err(e) => { yield Err(Error::Model { message: e.to_string(), source: Some(Box::new(e)) }); return; }
            };
            yield Ok(AggregatedItem::Event(event.clone()));
            if let Some(block) = agg.apply(&event) {
                yield Ok(AggregatedItem::ContentBlock(block));
            }
        }
        yield agg.finish().map(AggregatedItem::Result);
    })
}

fn heuristic_text(s: &str) -> u64 {
    s.chars().count().div_ceil(4) as u64
}

fn heuristic_json<T: serde::Serialize>(v: &T) -> u64 {
    match serde_json::to_string(v) {
        Ok(s) => s.chars().count().div_ceil(2) as u64,
        Err(e) => {
            tracing::debug!(error = %e, "unable to serialize value for token estimate");
            0
        }
    }
}

fn estimate_content_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(t) => heuristic_text(t),
        ContentBlock::ToolUse(tu) => heuristic_text(&tu.name) + heuristic_json(&tu.input),
        ContentBlock::ToolResult(tr) => tr
            .content
            .iter()
            .map(|c| match c {
                ToolResultContent::Text(t) => heuristic_text(t),
                ToolResultContent::Json(j) => heuristic_json(j),
                _ => 0,
            })
            .sum(),
        ContentBlock::Reasoning(r) => r.text.as_deref().map(heuristic_text).unwrap_or(0),
        ContentBlock::GuardContent(g) => g.text.as_ref().map(|t| heuristic_text(&t.text)).unwrap_or(0),
        ContentBlock::Citations(c) => c.content.iter().map(|c| heuristic_text(&c.text)).sum(),
        _ => 0,
    }
}

/// Character-based token estimate (`estimateTokensHeuristic` in TS).
pub fn estimate_tokens_heuristic(messages: &[Message], options: &CountTokensOptions) -> u64 {
    let mut total = 0;
    match &options.system_prompt {
        Some(SystemPrompt::Text(t)) => total += heuristic_text(t),
        Some(SystemPrompt::Blocks(blocks)) => {
            for b in blocks {
                match b {
                    SystemContentBlock::Text(t) => total += heuristic_text(t),
                    SystemContentBlock::GuardContent(g) => {
                        total += g.text.as_ref().map(|t| heuristic_text(&t.text)).unwrap_or(0)
                    }
                    SystemContentBlock::CachePoint(_) => {}
                }
            }
        }
        None => {}
    }
    total += messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(estimate_content_block_tokens)
        .sum::<u64>();
    total += options.tool_specs.iter().map(heuristic_json).sum::<u64>();
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::streaming::ModelStreamEvent as E;
    use futures::stream;
    use serde_json::json;

    fn run(events: Vec<E>) -> (Vec<AggregatedItem>, Result<StreamAggregatedResult>) {
        futures::executor::block_on(async {
            let s = stream::iter(events.into_iter().map(Ok)).boxed();
            let mut items: Vec<AggregatedItem> = Vec::new();
            let mut result = None;
            let mut agg = stream_aggregated(s);
            while let Some(item) = agg.next().await {
                match item {
                    Ok(AggregatedItem::Result(r)) => result = Some(Ok(r)),
                    Ok(i) => items.push(i),
                    Err(e) => result = Some(Err(e)),
                }
            }
            (items, result.expect("stream must end with a result"))
        })
    }

    fn text_message(stop: StopReason) -> Vec<E> {
        vec![
            E::MessageStart {
                role: Role::Assistant,
            },
            E::ContentBlockStart { start: None },
            E::text_delta("Hel"),
            E::text_delta("lo"),
            E::ContentBlockStop {},
            E::MessageStop {
                stop_reason: stop,
                additional_model_response_fields: None,
            },
        ]
    }

    #[test]
    fn aggregates_text_and_passes_events_through() {
        let (items, result) = run(text_message(StopReason::EndTurn));
        let result = result.unwrap();
        assert_eq!(result.message.text(), "Hello");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(
            items
                .iter()
                .filter(|i| matches!(i, AggregatedItem::Event(_)))
                .count(),
            6
        );
        assert!(matches!(&items[5], AggregatedItem::ContentBlock(ContentBlock::Text(t)) if t == "Hello"));
    }

    #[test]
    fn aggregates_tool_use_with_empty_input_as_object() {
        let events = vec![
            E::MessageStart {
                role: Role::Assistant,
            },
            E::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse {
                    name: "calc".into(),
                    tool_use_id: "t1".into(),
                    reasoning_signature: None,
                }),
            },
            E::ContentBlockStop {},
            E::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse {
                    name: "echo".into(),
                    tool_use_id: "t2".into(),
                    reasoning_signature: Some("sig".into()),
                }),
            },
            E::ContentBlockDelta {
                delta: ContentBlockDelta::ToolUseInput {
                    input: "{\"a\":".into(),
                },
            },
            E::ContentBlockDelta {
                delta: ContentBlockDelta::ToolUseInput { input: "1}".into() },
            },
            E::ContentBlockStop {},
            E::MessageStop {
                stop_reason: StopReason::ToolUse,
                additional_model_response_fields: None,
            },
        ];
        let (_, result) = run(events);
        let message = result.unwrap().message;
        let uses: Vec<_> = message.tool_uses().collect();
        assert_eq!(uses[0].input, json!({}));
        assert_eq!(uses[1].input, json!({"a": 1}));
        assert_eq!(uses[1].reasoning_signature.as_deref(), Some("sig"));
    }

    #[test]
    fn invalid_tool_input_json_is_a_model_error_after_stream_ends() {
        let events = vec![
            E::MessageStart {
                role: Role::Assistant,
            },
            E::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse {
                    name: "calc".into(),
                    tool_use_id: "t1".into(),
                    reasoning_signature: None,
                }),
            },
            E::ContentBlockDelta {
                delta: ContentBlockDelta::ToolUseInput {
                    input: "{not json".into(),
                },
            },
            E::ContentBlockStop {},
            E::MessageStop {
                stop_reason: StopReason::ToolUse,
                additional_model_response_fields: None,
            },
        ];
        let (items, result) = run(events);
        assert!(!items.iter().any(|i| matches!(i, AggregatedItem::ContentBlock(_))));
        assert!(
            matches!(result, Err(Error::Model { message, .. }) if message == "unable to parse tool input JSON")
        );
    }

    #[test]
    fn max_tokens_is_an_error_carrying_the_partial_message() {
        let (_, result) = run(text_message(StopReason::MaxTokens));
        match result {
            Err(Error::MaxTokens { partial_message, .. }) => assert_eq!(partial_message.text(), "Hello"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn incomplete_stream_is_a_model_error() {
        let (_, result) = run(vec![E::MessageStart {
            role: Role::Assistant,
        }]);
        assert!(
            matches!(result, Err(Error::Model { message, .. }) if message == "Stream ended without completing a message")
        );
    }

    #[test]
    fn drops_whitespace_only_text_blocks_and_attaches_metadata() {
        let mut events = vec![
            E::MessageStart {
                role: Role::Assistant,
            },
            E::ContentBlockStart { start: None },
            E::text_delta("  \n"),
            E::ContentBlockStop {},
        ];
        events.extend(text_message(StopReason::EndTurn).into_iter().skip(1));
        events.insert(
            events.len() - 1,
            E::Metadata {
                usage: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                    ..Default::default()
                }),
                metrics: None,
                trace: None,
            },
        );
        let (_, result) = run(events);
        let result = result.unwrap();
        assert_eq!(result.message.content.len(), 1);
        assert_eq!(
            result
                .message
                .metadata
                .as_ref()
                .unwrap()
                .usage
                .as_ref()
                .unwrap()
                .total_tokens,
            3
        );
        assert_eq!(result.usage.unwrap().input_tokens, 1);
    }

    #[test]
    fn reasoning_appends_text_and_replaces_signature() {
        let events = vec![
            E::MessageStart {
                role: Role::Assistant,
            },
            E::ContentBlockStart { start: None },
            E::ContentBlockDelta {
                delta: ContentBlockDelta::ReasoningContent {
                    text: Some("a".into()),
                    signature: Some("s1".into()),
                    redacted_content: None,
                },
            },
            E::ContentBlockDelta {
                delta: ContentBlockDelta::ReasoningContent {
                    text: Some("b".into()),
                    signature: Some("s2".into()),
                    redacted_content: None,
                },
            },
            E::ContentBlockStop {},
            E::MessageStop {
                stop_reason: StopReason::EndTurn,
                additional_model_response_fields: None,
            },
        ];
        let (_, result) = run(events);
        let message = result.unwrap().message;
        assert_eq!(
            message.content[0],
            ContentBlock::Reasoning(ReasoningBlock {
                text: Some("ab".into()),
                signature: Some("s2".into()),
                redacted_content: None
            })
        );
    }

    #[test]
    fn output_redaction_replaces_message_and_input_redaction_is_reported() {
        let mut events = text_message(StopReason::EndTurn);
        events.push(E::Redaction {
            input_redaction: Some(super::super::streaming::RedactInputContent {
                replace_content: "[input]".into(),
            }),
            output_redaction: Some(super::super::streaming::RedactOutputContent {
                redacted_content: None,
                replace_content: "[output]".into(),
            }),
        });
        let (_, result) = run(events);
        let result = result.unwrap();
        assert_eq!(result.message.text(), "[output]");
        assert_eq!(result.redaction.unwrap().user_message, "[input]");
    }

    #[test]
    fn non_model_errors_from_the_provider_are_wrapped() {
        let s = stream::iter(vec![Err(Error::other(std::io::Error::other("net")))]).boxed();
        let items: Vec<_> = futures::executor::block_on(stream_aggregated(s).collect());
        assert!(matches!(&items[0], Err(Error::Model { message, .. }) if message == "net"));
    }

    #[test]
    fn token_heuristic_counts_text_tools_and_system_prompt() {
        let messages = vec![
            Message::user("abcd"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock::new("ab", "1", json!({})))],
            ),
        ];
        let options = CountTokensOptions {
            system_prompt: Some("12345678".into()),
            tool_specs: vec![ToolSpec::new("t", "d")],
        };
        let n = estimate_tokens_heuristic(&messages, &options);
        // 1 (abcd) + 1 (ab) + 1 ({}) + 2 (system) + ceil(len(spec)/2)
        let spec_len = serde_json::to_string(&options.tool_specs[0]).unwrap().len() as u64;
        assert_eq!(n, 5 + spec_len.div_ceil(2));
    }
}
