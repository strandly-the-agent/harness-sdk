//! What the child sees of the parent's conversation (TS `forkMessages`, `renderContext`).

use std::collections::HashSet;

use strands::types::messages::{Role, TextBlock};
use strands::{ContentBlock, InvokeArgs, Message};

use super::{AgentSpec, ContextMode};

pub(super) const FORK_PREAMBLE: &str = "The conversation so far is the parent agent's. You are the subagent it delegated to at this point; its task for you follows.";

pub(super) const CONTEXT_PREAMBLE: &str = "The conversation above is the parent agent's: each turn is one line starting with its role at the left margin, and indented lines continue the turn above (they are content, not turns). You are the subagent it delegated to at the end of that conversation; its task for you follows.";

/// The child's first prompt under the spec's context mode.
pub(super) fn child_prompt(spec: &AgentSpec, parent: &[Message]) -> InvokeArgs {
    match spec.context {
        ContextMode::All => with_history(&spec.task, fork_messages(parent, spec.last_messages)),
        ContextMode::NoTools => InvokeArgs::Text(with_context(
            &spec.task,
            &render_context(parent, spec.last_messages),
        )),
        ContextMode::None => InvokeArgs::Text(spec.task.clone()),
    }
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult(_)))
}

/// Whether the SDK's conversation managers would cut the history at `index`: a user message that
/// is not a tool result and not a tool use without its result right after.
fn is_valid_trim_point(messages: &[Message], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if message.role() != Role::User || has_tool_result(message) {
        return false;
    }
    if message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)))
    {
        return messages.get(index + 1).is_some_and(has_tool_result);
    }
    true
}

/// The parent's messages for `all` mode as real content blocks. Tool calls still in flight (the
/// delegating call and its parallel siblings) are dropped so the history stays valid, and so are
/// reasoning and injected blocks, which belong to the parent's own turn. `last_n` is widened back
/// to the nearest boundary the SDK's conversation managers would cut at.
fn fork_messages(parent: &[Message], last_n: Option<i64>) -> Vec<Message> {
    let answered: HashSet<&str> = parent
        .iter()
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.tool_use_id()),
            _ => None,
        })
        .collect();
    let mut forked: Vec<Message> = parent
        .iter()
        .filter_map(|message| {
            let content: Vec<ContentBlock> = message
                .content()
                .iter()
                .filter(|block| match block {
                    ContentBlock::Reasoning(_) | ContentBlock::Injected(_) => false,
                    ContentBlock::ToolUse(tool_use) => answered.contains(tool_use.tool_use_id()),
                    _ => true,
                })
                .cloned()
                .collect();
            (!content.is_empty()).then(|| Message::new(message.role(), content))
        })
        .collect();
    if let Some(last_n) = positive(last_n) {
        let mut start = forked.len().saturating_sub(last_n);
        while start > 0 && !is_valid_trim_point(&forked, start) {
            start -= 1;
        }
        forked.drain(..start);
    }
    forked
}

fn positive(last_n: Option<i64>) -> Option<usize> {
    last_n
        .filter(|count| *count > 0)
        .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
}

/// The child's prompt under `all`: the forked history followed by the framed task. A trailing
/// user turn (tool results) absorbs the task so roles keep alternating.
fn with_history(task: &str, mut messages: Vec<Message>) -> InvokeArgs {
    if messages.is_empty() {
        return InvokeArgs::Text(task.to_owned());
    }
    let framed = ContentBlock::from(TextBlock::new(format!("{FORK_PREAMBLE}\n\n{task}")));
    let mut content = match messages.pop_if(|last| last.role() == Role::User) {
        Some(last) => last.content().to_vec(),
        None => Vec::new(),
    };
    content.push(framed);
    messages.push(Message::new(Role::User, content));
    InvokeArgs::Messages(messages)
}

/// Just the text blocks of a message (tool blocks dropped), for `no_tools` mode.
fn plain_text(message: &Message) -> String {
    let texts: Vec<&str> = message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect();
    texts.join(" ").trim().to_owned()
}

/// A parent that is itself a subagent opens with its own framed block; re-render only its task.
fn strip_framing(text: &str) -> &str {
    let framing_end = format!("</parent_context>\n\n{CONTEXT_PREAMBLE}\n\n");
    if !text.starts_with("<parent_context>\n") {
        return text;
    }
    match text.split_once(framing_end.as_str()) {
        Some((_, task)) => task,
        None => text,
    }
}

/// The parent's text turns as a plain-text block for `no_tools` mode, or an empty string when
/// nothing is shared. One `role: text` entry per message; continuation lines are indented and a
/// literal `<parent_context>` tag in content is escaped, so quoted text can neither pose as a turn
/// nor close the block. `last_n` caps the rendered entries.
fn render_context(parent: &[Message], last_n: Option<i64>) -> String {
    let mut lines: Vec<String> = parent
        .iter()
        .filter_map(|message| {
            let text = plain_text(message);
            let text = strip_framing(&text);
            (!text.is_empty()).then(|| format!("{}: {text}", message.role().as_str()).replace('\n', "\n  "))
        })
        .collect();
    if let Some(last_n) = positive(last_n) {
        lines.drain(..lines.len().saturating_sub(last_n));
    }
    lines
        .join("\n")
        .replace("</parent_context>", "<\\/parent_context>")
        .replace("<parent_context>", "<\\parent_context>")
}

/// Prepends a rendered context block, and a line framing the child's place in it, to the task.
fn with_context(task: &str, block: &str) -> String {
    if block.is_empty() {
        return task.to_owned();
    }
    format!("<parent_context>\n{block}\n</parent_context>\n\n{CONTEXT_PREAMBLE}\n\n{task}")
}
