//! Delegation: a delegating agent tool's answer becomes the caller's final answer (TS
//! `agent/agent-delegation.ts`, the `strands:agent-delegation` plugin).

use std::sync::{Arc, Mutex, PoisonError};

use crate::agent::local_agent::LocalAgent;
use crate::error::{DataError, Result};
use crate::hooks::events::{
    AfterToolCallEvent, AfterToolsEvent, BeforeModelCallEvent, BeforeToolsEvent, EndTurn,
};
use crate::hooks::types::{HookOptions, HookOrder};
use crate::tools::registry::ToolRegistry;
use crate::tools::structured_output::STRUCTURED_OUTPUT_TOOL_NAME;
use crate::types::json::to_string;
use crate::types::messages::{ContentBlock, TextBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;

const STATEFUL_DELEGATION: &str = "Delegation tools (delegate: true) are not supported with stateful models. Stateful models manage conversation state server-side, and delegation's early loop exit would leave unclosed function calls on the server.";
const DELEGATION_NOT_ALONE: &str = "This tool call was not executed. A delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools.";

#[derive(Default)]
struct DelegationState {
    tool_use_id: Option<String>,
}

type Shared = Arc<Mutex<Option<DelegationState>>>;

fn with_state<R>(state: &Shared, f: impl FnOnce(&mut Option<DelegationState>) -> R) -> R {
    f(&mut state.lock().unwrap_or_else(PoisonError::into_inner))
}

fn is_delegation_tool(registry: &ToolRegistry, name: &str) -> bool {
    registry.get(name).is_some_and(|tool| tool.delegates())
}

/// Registers the delegation hooks on the agent (TS `AgentDelegation.initAgent`).
pub(crate) fn register(agent: &LocalAgent) -> Result<()> {
    if agent.model().stateful() && agent.tool_registry().list().iter().any(|tool| tool.delegates()) {
        return Err(DataError::new(STATEFUL_DELEGATION).into());
    }
    let state: Shared = Arc::default();
    let before_tools = Arc::clone(&state);
    agent.add_hook(move |event: &mut BeforeToolsEvent<'_>| {
        if event.agent().model().stateful() {
            return;
        }
        let names: Vec<&str> = event
            .message()
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => Some(tool_use.name.as_str()),
                _ => None,
            })
            .collect();
        let has_delegation = names
            .iter()
            .any(|name| is_delegation_tool(event.agent().tool_registry(), name));
        with_state(&before_tools, |state| *state = Some(DelegationState::default()));
        if has_delegation && names.len() > 1 {
            event.set_cancel(Some(DELEGATION_NOT_ALONE.into()));
        }
    });
    let after_tool_call = Arc::clone(&state);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        let delegating =
            !event.agent().model().stateful() && event.tool().is_some_and(|tool| tool.delegates());
        let tool_use_id = event.tool_use().tool_use_id.clone();
        let failed = event.result().status == ToolResultStatus::Error;
        with_state(&after_tool_call, |state| {
            let Some(state) = state else {
                return;
            };
            if !delegating {
                if state.tool_use_id.as_deref() == Some(tool_use_id.as_str()) {
                    state.tool_use_id = None;
                }
                return;
            }
            state.tool_use_id = (!failed).then_some(tool_use_id);
        });
    });
    let after_tools = Arc::clone(&state);
    let options = HookOptions {
        order: HookOrder::SDK_LAST,
    };
    agent.add_hook_with(options, move |event: &mut AfterToolsEvent<'_>| {
        if event.agent().model().stateful() {
            return;
        }
        let Some(tool_use_id) = with_state(&after_tools, |state| {
            state.as_ref().and_then(|s| s.tool_use_id.clone())
        }) else {
            return;
        };
        let result = event.message().content.iter().find_map(|block| match block {
            ContentBlock::ToolResult(result) if result.tool_use_id == tool_use_id => Some(result.clone()),
            _ => None,
        });
        let Some(result) = result.filter(|result| result.status != ToolResultStatus::Error) else {
            with_state(&after_tools, |state| {
                if let Some(state) = state {
                    state.tool_use_id = None;
                }
            });
            return;
        };
        if event
            .agent()
            .tool_registry()
            .get(STRUCTURED_OUTPUT_TOOL_NAME)
            .is_some()
        {
            tracing::debug!(
                "tool_use_id=<{tool_use_id}> | parent requires structured output, skipping delegation"
            );
            return;
        }
        let content = to_content_blocks(&result);
        let blank = content
            .iter()
            .all(|block| matches!(block, ContentBlock::Text(text) if text.text.is_empty()));
        if blank {
            tracing::debug!(
                "tool_use_id=<{tool_use_id}> | delegation produced blank content, skipping delegation"
            );
            return;
        }
        event.set_end_turn(Some(EndTurn::Content(content)));
    });
    agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| with_state(&state, |state| *state = None));
    Ok(())
}

/// The caller's final answer from the delegate's result (TS `toContentBlocks`).
fn to_content_blocks(result: &ToolResultBlock) -> Vec<ContentBlock> {
    result
        .content
        .iter()
        .map(|content| match content {
            ToolResultContent::Json(json) => TextBlock::new(to_string(&json.json).unwrap_or_default()).into(),
            ToolResultContent::Text(text) => text.clone().into(),
            ToolResultContent::Image(image) => image.clone().into(),
            ToolResultContent::Video(video) => video.clone().into(),
            ToolResultContent::Document(document) => document.clone().into(),
            ToolResultContent::Unknown(unknown) => unknown.clone().into(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
