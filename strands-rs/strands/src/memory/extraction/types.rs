//! Extraction types (TS `memory/extraction/types.ts`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::InvocationContext;
use crate::error::Result;
use crate::models::model::Model;
use crate::types::agent::LocalAgent;
use crate::types::json::JsonObject;
use crate::types::messages::{ContentBlock, Message};

/// The kind of a content block, by its wire key (TS `MemoryContentBlockType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MemoryContentBlockType {
    /// `text`
    Text,
    /// `toolUse`
    ToolUse,
    /// `toolResult`
    ToolResult,
    /// `reasoning`
    Reasoning,
    /// `cachePoint`
    CachePoint,
    /// `guardContent`
    GuardContent,
    /// `audio`
    Audio,
    /// `image`
    Image,
    /// `video`
    Video,
    /// `document`
    Document,
    /// `citations`
    Citations,
    /// `injected`
    Injected,
    /// `summary`
    Summary,
}

impl MemoryContentBlockType {
    /// The kind of `block`; `None` for a block type this SDK version doesn't know.
    pub(crate) fn of(block: &ContentBlock) -> Option<Self> {
        Some(match block {
            ContentBlock::Text(_) => Self::Text,
            ContentBlock::ToolUse(_) => Self::ToolUse,
            ContentBlock::ToolResult(_) => Self::ToolResult,
            ContentBlock::Reasoning(_) => Self::Reasoning,
            ContentBlock::CachePoint(_) => Self::CachePoint,
            ContentBlock::GuardContent(_) => Self::GuardContent,
            ContentBlock::Audio(_) => Self::Audio,
            ContentBlock::Image(_) => Self::Image,
            ContentBlock::Video(_) => Self::Video,
            ContentBlock::Document(_) => Self::Document,
            ContentBlock::Citations(_) => Self::Citations,
            ContentBlock::Injected(_) => Self::Injected,
            ContentBlock::Summary(_) => Self::Summary,
            _ => return None,
        })
    }
}

/// Which content blocks are dropped before messages reach a store (TS `MemoryMessageFilter`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct MemoryMessageFilter {
    /// Block kinds to drop; a message left with no content is dropped whole.
    pub exclude: Vec<MemoryContentBlockType>,
}

impl Default for MemoryMessageFilter {
    /// Drops tool use and tool result blocks (TS `DEFAULT_MEMORY_MESSAGE_FILTER`).
    fn default() -> Self {
        Self {
            exclude: vec![
                MemoryContentBlockType::ToolUse,
                MemoryContentBlockType::ToolResult,
            ],
        }
    }
}

/// One extracted memory (TS `ExtractionResult`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ExtractionResult {
    /// The content to store.
    pub content: String,
    /// Metadata stored with it.
    pub metadata: Option<JsonObject>,
}

impl ExtractionResult {
    /// A result holding `content`.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            metadata: None,
        }
    }

    /// Sets the metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: JsonObject) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// What the manager hands an [`Extractor`] (TS `ExtractorContext`).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ExtractorContext {
    /// The model to use when the extractor has none of its own: the agent's `aux_model`.
    pub default_model: Option<Arc<dyn Model>>,
    /// The request this extraction belongs to, for usage accounting.
    pub invocation_context: Option<InvocationContext>,
    /// Wraps the extractor's model call in a `chat` span under the extraction span (TS `tracer`).
    pub(crate) tracer: Option<crate::telemetry::tracer::memory::MemoryTracer>,
}

impl ExtractorContext {
    /// A context with `default_model`.
    pub fn new(default_model: Option<Arc<dyn Model>>) -> Self {
        Self {
            default_model,
            invocation_context: None,
            tracer: None,
        }
    }
}

impl fmt::Debug for ExtractorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractorContext")
            .field("default_model", &self.default_model.as_ref().map(|_| ".."))
            .field("invocation_context", &self.invocation_context.is_some())
            .finish()
    }
}

/// Distills messages into discrete entries, each written through `MemoryStore::add` (TS
/// `Extractor`).
#[async_trait]
pub trait Extractor: Send + Sync + 'static {
    /// Extracts entries from `messages`.
    async fn extract(
        &self,
        messages: &[Message],
        context: &ExtractorContext,
    ) -> Result<Vec<ExtractionResult>>;
}

/// Starts an extraction in the background (TS `ExtractionTriggerContext.fire`); it never blocks.
#[derive(Clone)]
pub struct ExtractionFire(Arc<dyn Fn(Option<InvocationContext>) + Send + Sync>);

impl ExtractionFire {
    pub(crate) fn new(fire: impl Fn(Option<InvocationContext>) + Send + Sync + 'static) -> Self {
        Self(Arc::new(fire))
    }

    /// Dispatches an extraction for the store this trigger is attached for.
    pub fn fire(&self, invocation_context: Option<InvocationContext>) {
        (self.0)(invocation_context);
    }
}

impl fmt::Debug for ExtractionFire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExtractionFire(..)")
    }
}

/// What a trigger gets when it attaches to an agent (TS `ExtractionTriggerContext`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ExtractionTriggerContext<'a> {
    /// The agent to register hooks on.
    pub agent: &'a LocalAgent,
    /// Starts an extraction.
    pub fire: ExtractionFire,
}

/// Decides when extraction runs (TS `ExtractionTrigger`).
pub trait ExtractionTrigger: Send + Sync + 'static {
    /// The trigger's name.
    fn name(&self) -> &str;

    /// Registers the hooks that call `context.fire`.
    fn attach(&self, context: ExtractionTriggerContext<'_>);
}

impl fmt::Debug for dyn ExtractionTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractionTrigger")
            .field("name", &self.name())
            .finish()
    }
}

/// Extraction settings of a store (TS `ExtractionConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct ExtractionConfig {
    /// When extraction runs (default: an [`IntervalTrigger`](super::IntervalTrigger) every 5
    /// turns).
    pub trigger: Option<Vec<Arc<dyn ExtractionTrigger>>>,
    /// How entries are extracted. Default: none when the store implements `add_messages` (raw
    /// messages go to the backend), else a [`ModelExtractor`](super::ModelExtractor).
    pub extractor: Option<Arc<dyn Extractor>>,
    /// Which content blocks are dropped first (default: tool use and tool results).
    pub filter: Option<MemoryMessageFilter>,
}

impl fmt::Debug for ExtractionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractionConfig")
            .field("trigger", &self.trigger)
            .field("extractor", &self.extractor.as_ref().map(|_| ".."))
            .field("filter", &self.filter)
            .finish()
    }
}
