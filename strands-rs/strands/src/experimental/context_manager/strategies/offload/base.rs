//! The shared offload machinery (TS `context-manager/strategies/offload/base.ts`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;

use crate::agent::InvocationContext;
use crate::conversation_manager::compression::pin_message::is_pinned;
use crate::error::{Error, Result};
use crate::experimental::context_manager::ContextState;
use crate::experimental::context_manager::retrieval_tool::RETRIEVAL_TOOL_NAME;
use crate::experimental::context_manager::stash::Stash;
use crate::hooks::MessageAddedEvent;
use crate::models::model::CountTokensOptions;
use crate::types::agent::LocalAgent;
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{ContentBlock, Message, MessageMetadata, Role, TextBlock, ToolResultBlock};
use crate::types::tools::ToolResultStatus;

/// What an offload strategy acts on (TS `OffloadTarget`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OffloadTarget {
    /// Every block (TS `'*'`).
    All,
    /// Successful tool results (TS `'toolResults'`).
    ToolResults,
    /// Failed tool results (TS `'toolResultErrors'`).
    ToolResultErrors,
    /// Assistant text (TS `'assistantText'`).
    AssistantText,
    /// User text (TS `'userText'`).
    UserText,
    /// Results of the named tools (TS `string[]`): `name` or `tool::name` includes, `!name`
    /// excludes; includes win when both are given.
    Tools(Vec<String>),
}

/// When an offload strategy acts (TS `OffloadConditions`). Non-finite `utilization` and
/// `preserve_recent` values are ignored and negative ones count as zero, as in TS.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, bon::Builder)]
pub struct OffloadConditions {
    /// Offload only blocks above this many tokens.
    pub threshold: Option<u64>,
    /// Act only at or above this context utilization, removing whole messages.
    pub utilization: Option<f64>,
    /// Keep this many recent matching messages; a value in (0, 1) is a fraction of them.
    pub preserve_recent: Option<f64>,
}

fn finite(value: Option<f64>) -> Option<f64> {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.max(0.0))
}

pub(crate) fn build_tool_name_map(messages: &[Message]) -> HashMap<String, String> {
    messages
        .iter()
        .filter(|message| message.role() == Role::Assistant)
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => {
                Some((tool_use.tool_use_id().to_owned(), tool_use.name().to_owned()))
            }
            _ => None,
        })
        .collect()
}

fn has_text(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::Text(_)))
}

fn has_tool_use(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)))
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult(_)))
}

/// The tool-name filter from an array target (TS `resolveToolFilter`).
#[derive(Debug, Default)]
struct ToolFilter {
    include: Option<HashSet<String>>,
    exclude: Option<HashSet<String>>,
}

const TOOL_PREFIX: &str = "tool::";

fn resolve_tool_filter(target: &OffloadTarget) -> ToolFilter {
    let OffloadTarget::Tools(entries) = target else {
        return ToolFilter::default();
    };
    let strip = |name: &str| name.strip_prefix(TOOL_PREFIX).unwrap_or(name).to_owned();
    let mut includes = HashSet::new();
    let mut excludes = HashSet::new();
    for entry in entries {
        match entry.strip_prefix('!') {
            Some(name) => excludes.insert(strip(name)),
            None => includes.insert(strip(entry)),
        };
    }
    if !excludes.is_empty() && !includes.is_empty() {
        tracing::warn!("tool filter contains both includes and excludes, excludes will be ignored");
        return ToolFilter {
            include: Some(includes),
            exclude: None,
        };
    }
    ToolFilter {
        include: (!includes.is_empty()).then_some(includes),
        exclude: (!excludes.is_empty()).then_some(excludes),
    }
}

/// The target and conditions every offload strategy shares (TS `BaseOffloadStrategy` fields).
#[derive(Debug)]
pub(crate) struct OffloadBase {
    pub(crate) target: OffloadTarget,
    pub(crate) threshold: Option<u64>,
    pub(crate) utilization: Option<f64>,
    pub(crate) preserve_recent: f64,
    filter: ToolFilter,
    stash: Mutex<Option<Stash>>,
}

impl OffloadBase {
    pub(crate) fn new(target: OffloadTarget, conditions: OffloadConditions) -> Result<Self> {
        if matches!(&target, OffloadTarget::Tools(entries) if entries.is_empty()) {
            return Err(Error::other(
                "Empty array target matches nothing — provide at least one target",
            ));
        }
        Ok(Self::unchecked(target, conditions))
    }

    /// A base for a target known to be valid.
    pub(crate) fn unchecked(target: OffloadTarget, conditions: OffloadConditions) -> Self {
        let filter = resolve_tool_filter(&target);
        Self {
            target,
            threshold: conditions.threshold,
            utilization: finite(conditions.utilization),
            preserve_recent: finite(conditions.preserve_recent).unwrap_or(0.0),
            filter,
            stash: Mutex::new(None),
        }
    }

    pub(crate) fn stash(&self) -> Option<Stash> {
        self.stash.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn set_stash(&self, stash: Option<Stash>) {
        *self.stash.lock().unwrap_or_else(PoisonError::into_inner) = stash;
    }

    /// Whether the strategy removes whole messages (TS `_isMessageLevel`).
    pub(crate) fn is_message_level(&self) -> bool {
        self.utilization.is_some()
    }

    fn tool_matches(&self, block: &ToolResultBlock, tool_names: &HashMap<String, String>) -> bool {
        match &self.target {
            OffloadTarget::All => return true,
            OffloadTarget::ToolResults => return *block.status() == ToolResultStatus::Success,
            OffloadTarget::ToolResultErrors => return *block.status() == ToolResultStatus::Error,
            _ => {}
        }
        let Some(name) = tool_names.get(block.tool_use_id()) else {
            return false;
        };
        if let Some(exclude) = &self.filter.exclude {
            return !exclude.contains(name);
        }
        self.filter
            .include
            .as_ref()
            .is_some_and(|include| include.contains(name))
    }

    fn target_matches_message(&self, message: &Message) -> bool {
        match self.target {
            OffloadTarget::All => true,
            OffloadTarget::AssistantText => message.role() == Role::Assistant && has_text(message),
            OffloadTarget::UserText => message.role() == Role::User && has_text(message),
            _ => false,
        }
    }

    fn message_matches(&self, message: &Message, tool_names: &HashMap<String, String>) -> bool {
        if self.target_matches_message(message) {
            return true;
        }
        message.role() == Role::User
            && message.content().iter().any(|block| match block {
                ContentBlock::ToolResult(result) => self.tool_matches(result, tool_names),
                _ => false,
            })
    }

    /// The matching message indices except the most recent `preserve_recent` (TS
    /// `getOldestMatches`).
    pub(crate) fn oldest_matches(
        &self,
        messages: &[Message],
        tool_names: &HashMap<String, String>,
    ) -> Vec<usize> {
        let matching: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| self.message_matches(message, tool_names))
            .map(|(index, _)| index)
            .collect();
        let count = preserved_count(self.preserve_recent, matching.len());
        if count == 0 || count >= matching.len() {
            return Vec::new();
        }
        matching.into_iter().rev().skip(count).rev().collect()
    }

    /// Whether `block` is one this strategy may replace (TS `_blockMatchesTarget`).
    pub(crate) fn block_matches(
        &self,
        block: &ContentBlock,
        message: &Message,
        tool_names: &HashMap<String, String>,
    ) -> bool {
        match block {
            ContentBlock::ToolUse(_)
            | ContentBlock::CachePoint(_)
            | ContentBlock::Reasoning(_)
            | ContentBlock::Injected(_) => false,
            ContentBlock::Text(_) => self.target_matches_message(message),
            ContentBlock::ToolResult(result) => {
                if self.stash().is_some()
                    && tool_names.get(result.tool_use_id()).map(String::as_str) == Some(RETRIEVAL_TOOL_NAME)
                {
                    return false;
                }
                self.tool_matches(result, tool_names)
            }
            _ => self.target == OffloadTarget::All,
        }
    }

    /// The candidate message indices for message-level removal (TS `_getEligibleMessages`).
    pub(crate) async fn eligible_messages(&self, agent: &LocalAgent) -> Result<Vec<usize>> {
        let messages = agent.messages();
        let tool_names = build_tool_name_map(messages);
        let candidates: Vec<usize> = if self.preserve_recent > 0.0 {
            self.oldest_matches(messages, &tool_names)
                .into_iter()
                .filter(|&index| index > 0 && !is_pinned(messages, index))
                .collect()
        } else {
            messages
                .iter()
                .enumerate()
                .skip(1)
                .filter(|&(index, message)| {
                    !is_pinned(messages, index) && self.message_matches(message, &tool_names)
                })
                .map(|(index, _)| index)
                .collect()
        };
        let Some(threshold) = self.threshold else {
            return Ok(candidates);
        };
        let mut eligible = Vec::new();
        for index in candidates {
            let Some(message) = messages.get(index) else {
                continue;
            };
            for block in message.content() {
                if !self.block_matches(block, message, &tool_names) {
                    continue;
                }
                if exceeds(block_tokens(agent, message.role(), block).await?, threshold) {
                    eligible.push(index);
                    break;
                }
            }
        }
        Ok(eligible)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "TS `Math.ceil(length * ratio)` / `Math.floor(count)` on non-negative values"
)]
fn preserved_count(preserve_recent: f64, matching: usize) -> usize {
    if preserve_recent > 0.0 && preserve_recent < 1.0 {
        (matching as f64 * preserve_recent).ceil() as usize
    } else {
        preserve_recent.floor() as usize
    }
}

pub(crate) fn exceeds(tokens: u64, threshold: u64) -> bool {
    tokens > threshold
}

/// The token count of one block as its own message.
pub(crate) async fn block_tokens(agent: &LocalAgent, role: Role, block: &ContentBlock) -> Result<u64> {
    let message = Message::new(role, vec![block.clone()]);
    agent
        .model()
        .count_tokens(std::slice::from_ref(&message), CountTokensOptions::default())
        .await
}

/// `message` with new content, keeping its tracking id and metadata.
pub(crate) fn with_content(message: &Message, content: Vec<ContentBlock>) -> Message {
    let rebuilt = Message::new(message.role(), content).with_tracking_id(message.tracking_id());
    match message.metadata() {
        Some(metadata) => rebuilt.with_metadata(metadata.clone()),
        None => rebuilt,
    }
}

/// The message at `index` plus its tool-pair partner, or nothing when either is the first or
/// last message (TS `collectRemovableWithPair`).
pub(crate) fn collect_removable_with_pair(messages: &[Message], index: usize) -> Vec<usize> {
    let Some(message) = messages.get(index) else {
        return Vec::new();
    };
    let last = messages.len() - 1;
    if index == 0 || index == last {
        return Vec::new();
    }
    let mut result = vec![index];
    if has_tool_result(message) && messages.get(index - 1).is_some_and(has_tool_use) {
        if index - 1 > 0 {
            result.push(index - 1);
        } else {
            return Vec::new();
        }
    }
    if has_tool_use(message) && messages.get(index + 1).is_some_and(has_tool_result) {
        if index + 1 < last {
            result.push(index + 1);
        } else {
            return Vec::new();
        }
    }
    result
}

/// Removes `to_remove` with their tool-pair partners; returns the count removed and the lowest
/// removed index (TS `spliceWithPairs`).
pub(crate) fn splice_with_pairs(messages: &mut Vec<Message>, to_remove: &[usize]) -> (usize, usize) {
    let mut doomed: Vec<usize> = to_remove
        .iter()
        .flat_map(|&index| collect_removable_with_pair(messages, index))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    doomed.sort_unstable();
    let lowest = doomed.first().copied().unwrap_or(messages.len());
    for &index in doomed.iter().rev() {
        messages.remove(index);
    }
    (doomed.len(), lowest)
}

/// Adds `text` to an assistant neighbour of `index`, or inserts an assistant message there
/// (TS `insertMarker`).
pub(crate) fn insert_marker(messages: &mut Vec<Message>, index: usize, text: &str) {
    let index = index.min(messages.len()).max(1);
    for neighbor in [index - 1, index] {
        let Some(carrier) = messages.get(neighbor) else {
            continue;
        };
        if neighbor > 0 && carrier.role() == Role::Assistant {
            let mut content = carrier.content().to_vec();
            content.push(TextBlock::new(text).into());
            let marked = with_content(carrier, content);
            if let Some(slot) = messages.get_mut(neighbor) {
                *slot = marked;
            }
            return;
        }
    }
    let marker = Message::new(Role::Assistant, vec![TextBlock::new(text).into()]);
    messages.insert(index.min(messages.len()), marker);
}

fn pin_flag(message: &Message) -> bool {
    message
        .metadata()
        .and_then(MessageMetadata::custom)
        .and_then(|custom| custom.get("pinned"))
        == Some(&JsonValue::Bool(true))
}

/// Merges consecutive same-role messages (TS `repairAlternation`); a merge keeps only the pin
/// flag of the metadata.
pub(crate) fn repair_alternation(messages: &mut Vec<Message>) {
    let mut repaired: Vec<Message> = Vec::with_capacity(messages.len());
    for current in messages.drain(..) {
        let Some(prev) = repaired.last_mut().filter(|prev| prev.role() == current.role()) else {
            repaired.push(current);
            continue;
        };
        let pinned = pin_flag(prev) || pin_flag(&current);
        let content = prev.content().iter().chain(current.content()).cloned().collect();
        let mut merged = Message::new(prev.role(), content).with_tracking_id(prev.tracking_id());
        if pinned {
            let custom = JsonObject::from_iter([("pinned".to_owned(), JsonValue::Bool(true))]);
            merged = merged.with_metadata(MessageMetadata::new().with_custom(custom));
        }
        *prev = merged;
    }
    *messages = repaired;
}

/// What a concrete offload strategy contributes (TS `BaseOffloadStrategy` abstract members).
#[async_trait]
pub(crate) trait Offloader: Send + Sync + 'static {
    fn base(&self) -> &OffloadBase;

    /// The marker left where messages were removed (TS `_makeRemovalMarker`).
    fn removal_marker(&self, _count: usize) -> Option<String> {
        None
    }

    /// The replacement for a block over the threshold, or `None` to keep it (TS `_replaceBlock`).
    async fn replace_block(
        &self,
        block: &ContentBlock,
        tokens: u64,
        message: &Message,
        agent: &LocalAgent,
        stash_refs: &[String],
        invocation_context: Option<&InvocationContext>,
    ) -> Option<ContentBlock>;
}

/// Registers the eager per-block hook when the strategy has no conditions that need the whole
/// context (TS `BaseOffloadStrategy.init`).
pub(crate) fn init_offload<O: Offloader>(offloader: Arc<O>, agent: &mut LocalAgent, stash: Option<Stash>) {
    let base = offloader.base();
    base.set_stash(stash);
    if base.is_message_level() || base.preserve_recent > 0.0 {
        return;
    }
    agent
        .hooks()
        .add_async_hook::<MessageAddedEvent<'static>>(move |event| {
            let offloader = Arc::clone(&offloader);
            Box::pin(async move {
                let tracking_id = event.message().tracking_id().to_owned();
                let agent = event.agent_mut();
                let Some(index) = agent
                    .messages()
                    .iter()
                    .rposition(|message| message.tracking_id() == tracking_id)
                else {
                    return;
                };
                if is_pinned(agent.messages(), index) {
                    return;
                }
                let tool_names = build_tool_name_map(agent.messages());
                if let Err(error) = transform_blocks(&*offloader, agent, index, &tool_names, None).await {
                    tracing::warn!("error=<{error}> | eager offload failed");
                }
            })
        });
}

/// Runs `offloader` on `context` (TS `BaseOffloadStrategy.apply` with the default per-message
/// step).
pub(crate) async fn apply_offload<O: Offloader>(
    offloader: &O,
    context: &mut ContextState<'_>,
) -> Result<bool> {
    let base = offloader.base();
    if context.stash.is_some() {
        base.set_stash(context.stash.clone());
    }
    if let Some(utilization) = base.utilization {
        if !context.overflow && context.utilization < utilization {
            return Ok(false);
        }
        return apply_per_message(offloader, context).await;
    }
    apply_per_block(offloader, context).await
}

async fn apply_per_block<O: Offloader>(offloader: &O, context: &mut ContextState<'_>) -> Result<bool> {
    let base = offloader.base();
    let invocation_context = context.invocation_context.clone();
    let agent = &mut *context.agent;
    let tool_names = build_tool_name_map(agent.messages());
    let eligible: Vec<usize> = if base.preserve_recent > 0.0 {
        base.oldest_matches(agent.messages(), &tool_names)
    } else {
        (0..agent.messages().len()).collect()
    };
    let mut acted = false;
    for index in eligible {
        if is_pinned(agent.messages(), index) {
            continue;
        }
        if transform_blocks(offloader, agent, index, &tool_names, invocation_context.as_ref()).await? {
            acted = true;
        }
    }
    Ok(acted)
}

/// Removes eligible messages with their pairs and leaves the strategy's marker (TS
/// `_applyPerMessage`).
pub(crate) async fn apply_per_message<O: Offloader>(
    offloader: &O,
    context: &mut ContextState<'_>,
) -> Result<bool> {
    if context.agent.messages().len() <= 1 {
        return Ok(false);
    }
    let eligible = offloader.base().eligible_messages(context.agent).await?;
    if eligible.is_empty() {
        return Ok(false);
    }
    let messages = context.agent.messages_mut();
    let (removed, lowest) = splice_with_pairs(messages, &eligible);
    if removed == 0 {
        return Ok(false);
    }
    if let Some(marker) = offloader.removal_marker(removed) {
        insert_marker(messages, lowest, &marker);
    }
    repair_alternation(messages);
    Ok(true)
}

/// Replaces the blocks of the message at `index` that are over the threshold (TS
/// `_transformBlocks`).
pub(crate) async fn transform_blocks<O: Offloader + ?Sized>(
    offloader: &O,
    agent: &mut LocalAgent,
    index: usize,
    tool_names: &HashMap<String, String>,
    invocation_context: Option<&InvocationContext>,
) -> Result<bool> {
    let base = offloader.base();
    let threshold = base.threshold.unwrap_or(0);
    let Some(message) = agent.messages().get(index).cloned() else {
        return Ok(false);
    };
    let stash = base.stash();
    let mut content = message.content().to_vec();
    let mut acted = false;
    for (block_index, block) in message.content().iter().enumerate() {
        if !base.block_matches(block, &message, tool_names) {
            continue;
        }
        let tokens = block_tokens(agent, message.role(), block).await?;
        if !exceeds(tokens, threshold) {
            continue;
        }
        let refs = stash
            .as_ref()
            .map(|stash| stash.refs_for(block, &message, block_index))
            .unwrap_or_default();
        let replacement = offloader
            .replace_block(block, tokens, &message, agent, &refs, invocation_context)
            .await;
        if let Some(replacement) = replacement.filter(|replacement| replacement != block) {
            if let Some(slot) = content.get_mut(block_index) {
                *slot = replacement;
            }
            acted = true;
        }
    }
    if acted && let Some(slot) = agent.messages_mut().get_mut(index) {
        *slot = with_content(&message, content);
    }
    Ok(acted)
}

/// `N message` or `N messages`.
pub(crate) fn messages_text(count: usize) -> String {
    crate::experimental::context_manager::methods::truncate::plural(count, "message", "messages")
}
