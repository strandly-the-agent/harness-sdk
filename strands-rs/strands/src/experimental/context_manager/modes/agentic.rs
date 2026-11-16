//! The agentic context tools and the token usage note (TS
//! `context-manager/modes/agentic/agentic-context.ts`).

use serde::Deserialize;

use crate::JsonValue;
use crate::agent::InvocationContext;
use crate::conversation_manager::compression::context_compression::{
    MessageTypeFilter, adjust_split_point_for_tool_pairs, find_valid_trim_point, generate_summary,
    matches_message_type,
};
use crate::conversation_manager::compression::pin_message::{is_pinned, pin_message, unpin_message};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};
use crate::types::messages::{ContentBlock, Message, Role};
use crate::vended_tools::schema_tool;

const DEFAULT_KEEP_RECENT_MESSAGES: usize = 10;
const DEFAULT_SUMMARY_RATIO: f64 = 0.3;
const MIN_SUMMARY_RATIO: f64 = 0.1;
const MAX_SUMMARY_RATIO: f64 = 0.8;
const MIN_MESSAGES_FOR_COMPRESSION: usize = 2;

const SUMMARIZE_DESCRIPTION: &str = "Compress the oldest messages in your conversation into a concise summary to free up context space. \
The summary preserves key information while reducing token usage. \
Recent messages are kept verbatim. Pinned messages are never summarized away. \
Often most useful with messageType \"messages\" to preserve tool results verbatim while condensing discussion.";

const TRUNCATE_DESCRIPTION: &str = "Drop the oldest messages from your conversation history entirely to free up context space. \
Use this when older messages are no longer relevant and do not need to be preserved in any form. \
Pinned messages are always kept. Tool-call pairs are preserved together. \
Often most useful with messageType \"tools\" since tool results tend to be large and lose relevance quickly.";

const PIN_DESCRIPTION: &str = "Pin or unpin messages in the conversation history. \
Pinned messages are protected from eviction during context reduction (summarize or truncate). \
Best for critical context like user-established constraints or key facts that must survive compression. \
Pin sparingly — too many pinned messages limit what can be compressed. \
Select messages using relative references: pin the current exchange, the last N messages, or specific indices.";

/// The `summarize_context` input schema, as `z.toJSONSchema` writes it in TS.
pub(crate) fn summarize_schema() -> JsonValue {
    serde_json::json!({"type":"object","properties":{"keepRecent":{"description":"Minimum number of recent messages to preserve verbatim. Defaults to 10.","type":"integer","minimum":2,"maximum":9_007_199_254_740_991_u64},"summaryRatio":{"description":"Fraction of the oldest messages to fold into the summary (0.1–0.8). Defaults to 0.3.","type":"number","minimum":0.1,"maximum":0.8},"messageType":{"description":"Filter which messages to target. \"tools\" targets only tool use/result messages, \"messages\" targets only non-tool messages, \"all\" (default) targets everything.","type":"string","enum":["tools","messages","all"]}},"additionalProperties":false})
}

/// The `truncate_context` input schema, as `z.toJSONSchema` writes it in TS.
pub(crate) fn truncate_schema() -> JsonValue {
    serde_json::json!({"type":"object","properties":{"keepRecent":{"description":"Number of most recent messages to keep. Everything older (and unpinned) is dropped. Defaults to 10.","type":"integer","minimum":2,"maximum":9_007_199_254_740_991_u64},"messageType":{"description":"Filter which messages to target. \"tools\" targets only tool use/result messages, \"messages\" targets only non-tool messages, \"all\" (default) targets everything.","type":"string","enum":["tools","messages","all"]}},"additionalProperties":false})
}

/// The `pin_context` input schema, as `z.toJSONSchema` writes it in TS.
pub(crate) fn pin_schema() -> JsonValue {
    serde_json::json!({"type":"object","properties":{"select":{"anyOf":[{"type":"string","const":"last_turn","description":"Select messages from the current turn (everything since the last user request)."},{"type":"integer","minimum":1,"maximum":9_007_199_254_740_991_u64,"description":"Select the last N messages from the conversation."},{"minItems":1,"type":"array","items":{"type":"integer","minimum":0,"maximum":9_007_199_254_740_991_u64},"description":"Select messages at specific zero-based indices."}],"description":"Which messages to target. \"last_turn\" for the current exchange, a number for the last N messages, or an array of indices."},"filter":{"description":"Narrow the selection to only messages matching this filter. \"user\" matches user text messages, \"assistant\" matches assistant text responses, \"tools\" matches tool call and tool result messages (pairs are always kept together).","type":"string","enum":["user","assistant","tools"]},"action":{"default":"pin","description":"Whether to pin or unpin the selected messages.","type":"string","enum":["pin","unpin"]}},"required":["select","action"],"additionalProperties":false})
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MessageType {
    Tools,
    Messages,
    #[default]
    All,
}

impl MessageType {
    fn filter(self) -> MessageTypeFilter {
        match self {
            Self::Tools => MessageTypeFilter::Tools,
            Self::Messages => MessageTypeFilter::Messages,
            Self::All => MessageTypeFilter::All,
        }
    }

    /// `"tools" ` for a named filter, empty for `all`, as the TS result texts quote it.
    fn quoted(self) -> &'static str {
        match self {
            Self::Tools => "\"tools\" ",
            Self::Messages => "\"messages\" ",
            Self::All => "",
        }
    }

    fn eligible_label(self) -> &'static str {
        match self {
            Self::Tools => "\"tools\"",
            Self::Messages => "\"messages\"",
            Self::All => "eligible",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummarizeInput {
    keep_recent: Option<usize>,
    summary_ratio: Option<f64>,
    #[serde(default)]
    message_type: MessageType,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TruncateInput {
    keep_recent: Option<usize>,
    #[serde(default)]
    message_type: MessageType,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Select {
    LastTurn(LastTurn),
    Last(usize),
    Indices(Vec<usize>),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LastTurn {
    LastTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PinFilter {
    User,
    Assistant,
    Tools,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PinAction {
    #[default]
    Pin,
    Unpin,
}

#[derive(Deserialize)]
struct PinInput {
    select: Select,
    filter: Option<PinFilter>,
    #[serde(default)]
    action: PinAction,
}

/// Splits `messages[..range_end]` into what a reduction may fold away and what it keeps: the
/// first user message, pinned messages and messages outside the filter (TS `collectPreserved`).
fn collect_preserved(
    messages: &[Message],
    range_end: usize,
    filter: MessageTypeFilter,
) -> (Vec<Message>, Vec<Message>) {
    let mut eligible = Vec::new();
    let mut preserved = Vec::new();
    let mut found_first_user = false;
    for (index, message) in messages.iter().enumerate().take(range_end) {
        let is_first_user = !found_first_user && message.role() == Role::User;
        found_first_user |= is_first_user;
        if is_first_user || is_pinned(messages, index) || !matches_message_type(message, filter) {
            preserved.push(message.clone());
        } else {
            eligible.push(message.clone());
        }
    }
    (eligible, preserved)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "TS `Math.floor(length * ratio)` on a message count"
)]
fn ratio_split(length: usize, ratio: f64) -> usize {
    ((length as f64 * ratio).floor() as usize).max(1)
}

async fn summarize_context(input: SummarizeInput, context: ToolContext) -> String {
    let agent = context.agent();
    let mut messages = agent.current_messages();
    let original = messages.len();
    let preserve_recent = input.keep_recent.unwrap_or(DEFAULT_KEEP_RECENT_MESSAGES);
    let ratio = input
        .summary_ratio
        .unwrap_or(DEFAULT_SUMMARY_RATIO)
        .clamp(MIN_SUMMARY_RATIO, MAX_SUMMARY_RATIO);
    let split_point = ratio_split(original, ratio).min(original.saturating_sub(preserve_recent));
    if split_point == 0 {
        return format!(
            "No summarization performed: not enough eligible messages to compress (conversation has {original} messages, preserving recent {preserve_recent})."
        );
    }
    let Ok(split_point) = adjust_split_point_for_tool_pairs(&messages, split_point) else {
        return format!(
            "No summarization performed: no valid split boundary found from index {split_point} onward (requires a message that isn't mid-tool-call). Try a smaller keepRecent, a larger summaryRatio, or use truncate_context with messageType=\"tools\" instead."
        );
    };
    let (eligible, preserved) = collect_preserved(&messages, split_point, input.message_type.filter());
    if eligible.is_empty() {
        return format!(
            "No summarization performed: no {} messages found in range (conversation has {original} messages).",
            input.message_type.eligible_label()
        );
    }
    let invocation_context = InvocationContext::auxiliary(context.invocation_context());
    let summary = match generate_summary(&eligible, agent.aux_model(), None, invocation_context).await {
        Ok(summary) => summary,
        Err(error) => return format!("Summarization failed: {error}"),
    };
    messages.splice(..split_point, preserved.into_iter().chain([summary]));
    agent.replace_messages(messages.clone());
    format!(
        "Summarized {} {}message(s). Removed {} message(s), {} remaining.",
        eligible.len(),
        input.message_type.quoted(),
        original - messages.len(),
        messages.len()
    )
}

fn truncate_context(input: TruncateInput, context: &ToolContext) -> String {
    let agent = context.agent();
    let mut messages = agent.current_messages();
    let original = messages.len();
    let window = input.keep_recent.unwrap_or(DEFAULT_KEEP_RECENT_MESSAGES);
    if original <= MIN_MESSAGES_FOR_COMPRESSION || original <= window {
        return format!("No messages dropped: conversation only has {original} messages.");
    }
    let start = original - window;
    let trim_point = find_valid_trim_point(&messages, start);
    if trim_point >= original {
        return format!(
            "No messages dropped: no valid trim boundary exists between index {start} and {} (requires a plain user text message). Try a larger keepRecent or use summarize_context instead.",
            original - 1
        );
    }
    let (eligible, preserved) = collect_preserved(&messages, trim_point, input.message_type.filter());
    if eligible.is_empty() {
        return format!(
            "No messages dropped: no {} messages found in range (conversation has {original} messages).",
            input.message_type.eligible_label()
        );
    }
    messages.splice(..trim_point, preserved);
    agent.replace_messages(messages.clone());
    format!(
        "Dropped {} {}message(s). {} remaining.",
        original - messages.len(),
        input.message_type.quoted(),
        messages.len()
    )
}

fn has_text(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::Text(_)))
}

fn matches_pin_filter(message: &Message, filter: PinFilter) -> bool {
    match filter {
        PinFilter::User => message.role() == Role::User && has_text(message),
        PinFilter::Assistant => message.role() == Role::Assistant && has_text(message),
        PinFilter::Tools => message
            .content()
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse(_) | ContentBlock::ToolResult(_))),
    }
}

fn pin_context(input: PinInput, context: &ToolContext) -> String {
    let agent = context.agent();
    let mut messages = agent.current_messages();
    if messages.is_empty() {
        return "No messages in the conversation.".to_owned();
    }
    let len = messages.len();
    let candidates: Vec<usize> = match input.select {
        Select::LastTurn(LastTurn::LastTurn) => {
            let mut indices = Vec::new();
            for (index, message) in messages.iter().enumerate().rev() {
                indices.push(index);
                if message.role() == Role::User && has_text(message) {
                    break;
                }
            }
            indices
        }
        Select::Last(count) => (0..count.min(len)).map(|offset| len - 1 - offset).collect(),
        Select::Indices(indices) => {
            let indices: Vec<usize> = indices.into_iter().filter(|&index| index < len).collect();
            if indices.is_empty() {
                return format!("All indices out of range (conversation has {len} messages).");
            }
            indices
        }
    };
    let targets: Vec<usize> = match input.filter {
        Some(filter) => candidates
            .into_iter()
            .filter(|&index| {
                messages
                    .get(index)
                    .is_some_and(|message| matches_pin_filter(message, filter))
            })
            .collect(),
        None => candidates,
    };
    if targets.is_empty() {
        return "No matching messages found.".to_owned();
    }
    for &index in &targets {
        match input.action {
            PinAction::Pin => pin_message(&mut messages, index),
            PinAction::Unpin => unpin_message(&mut messages, index),
        }
    }
    agent.replace_messages(messages);
    let verb = match input.action {
        PinAction::Pin => "Pinned",
        PinAction::Unpin => "Unpinned",
    };
    format!("{verb} {} message(s).", targets.len())
}

/// The `summarize_context` tool (TS `summarizeContextTool`).
pub(crate) fn summarize_context_tool() -> FunctionTool {
    let schema = summarize_schema();
    schema_tool(
        "summarize_context".to_owned(),
        SUMMARIZE_DESCRIPTION.to_owned(),
        schema.clone(),
        InputValidator::new(&schema),
        |_| Ok(()),
        |input: SummarizeInput, context: ToolContext| async move {
            Ok::<_, ToolError>(summarize_context(input, context).await)
        },
    )
}

/// The `truncate_context` tool (TS `truncateContextTool`).
pub(crate) fn truncate_context_tool() -> FunctionTool {
    let schema = truncate_schema();
    schema_tool(
        "truncate_context".to_owned(),
        TRUNCATE_DESCRIPTION.to_owned(),
        schema.clone(),
        InputValidator::new(&schema),
        |_| Ok(()),
        |input: TruncateInput, context: ToolContext| async move {
            Ok::<_, ToolError>(truncate_context(input, &context))
        },
    )
}

/// The `pin_context` tool (TS `pinContextTool`).
pub(crate) fn pin_context_tool() -> FunctionTool {
    let schema = pin_schema();
    // Zod fills the `action` default before validating, so input may omit it.
    let mut validation = schema.clone();
    if let Some(required) = validation.get_mut("required") {
        *required = serde_json::json!(["select"]);
    }
    schema_tool(
        "pin_context".to_owned(),
        PIN_DESCRIPTION.to_owned(),
        schema,
        InputValidator::new(&validation),
        |_| Ok(()),
        |input: PinInput, context: ToolContext| async move { Ok::<_, ToolError>(pin_context(input, &context)) },
    )
}

/// The three agentic context tools, in TS order.
pub(crate) fn agentic_tools() -> Vec<FunctionTool> {
    vec![
        summarize_context_tool(),
        truncate_context_tool(),
        pin_context_tool(),
    ]
}

#[cfg(test)]
mod tests;
