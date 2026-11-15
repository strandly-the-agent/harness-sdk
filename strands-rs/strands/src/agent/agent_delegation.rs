//! Delegation semantics for agent tools with `delegate` set (TS `agent/agent-delegation.ts`).
//!
//! A delegation tool must be the only tool of its turn; when it succeeds, the parent ends the
//! turn with the tool's content as the final message instead of calling the model again. It is
//! the `strands:agent-delegation` plugin every agent registers unless a user plugin takes that
//! name. TS refuses a lifted batch cancel and unwraps child events in an `ExecuteToolStage`
//! middleware; until middleware lands those jobs are a `BeforeToolCallEvent` hook and an
//! executor event (P-82).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::hooks::{
    AfterToolCallEvent, AfterToolsEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent,
    HookCallbackOptions, HookOrder, HookRegistry,
};
use crate::plugins::Plugin;
use crate::tools::executors::executor::error_text_result;
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, Tool, ToolContext, ToolOutcome, ToolRegistry, ToolStream};
use crate::types::agent::LocalAgent;
use crate::types::json::to_js_string;
use crate::types::messages::{ContentBlock, TextBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::{ToolResultStatus, ToolSpec};

const BATCH_CANCEL: &str = "This tool call was not executed. A delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools.";
const EXECUTION_REFUSED: &str = "Delegation failed: a delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools.";
const STATEFUL_UNSUPPORTED: &str = "Delegation tools (delegate: true) are not supported with stateful models. Stateful models manage conversation state server-side, and delegation's early loop exit would leave unclosed function calls on the server.";

/// The delegation state of one tool batch (TS `DelegationState`).
#[derive(Debug, Default)]
struct Batch {
    tool_use_count: usize,
    /// The tool use id of the delegation tool that succeeded.
    tool_use_id: Option<String>,
}

type Shared = Arc<Mutex<Option<Batch>>>;

fn lock(state: &Shared) -> MutexGuard<'_, Option<Batch>> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn is_delegation_tool(registry: &ToolRegistry, name: &str) -> bool {
    registry.get(name).is_some_and(|tool| tool.is_delegate())
}

/// The name of the built-in delegation plugin (TS `AgentDelegation.name`).
pub(crate) const AGENT_DELEGATION_NAME: &str = "strands:agent-delegation";

/// The built-in delegation plugin (TS `AgentDelegation`).
pub(crate) struct AgentDelegation;

#[async_trait]
impl Plugin for AgentDelegation {
    fn name(&self) -> &str {
        AGENT_DELEGATION_NAME
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        check(agent)?;
        install(agent.hooks());
        Ok(())
    }
}

/// TS `initAgent`'s fail-fast check.
fn check(agent: &LocalAgent) -> Result<()> {
    let has_delegation = agent.tool_registry().list().any(|tool| tool.is_delegate());
    if agent.model().stateful() && has_delegation {
        return Err(Error::other(STATEFUL_UNSUPPORTED));
    }
    Ok(())
}

/// Registers the delegation hooks on `hooks`.
fn install(hooks: &HookRegistry) {
    let state = Shared::default();

    let batch = Arc::clone(&state);
    hooks.add_hook(move |event: &mut BeforeToolsEvent<'_>| {
        if event.agent().model().stateful() {
            return;
        }
        let names: Vec<&str> = event
            .message()
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => Some(tool_use.name()),
                _ => None,
            })
            .collect();
        let has_delegation = names
            .iter()
            .any(|name| is_delegation_tool(event.agent().tool_registry(), name));
        let tool_use_count = names.len();
        *lock(&batch) = Some(Batch {
            tool_use_count,
            tool_use_id: None,
        });
        if has_delegation && tool_use_count > 1 {
            event.set_cancel(BATCH_CANCEL);
        }
    });

    // TS refuses in its `ExecuteToolStage` middleware when a later hook cleared the batch cancel.
    let batch = Arc::clone(&state);
    hooks.add_hook_with(
        HookCallbackOptions::builder().order(HookOrder::SDK_LAST).build(),
        move |event: &mut BeforeToolCallEvent| {
            let delegates = event
                .selected_tool()
                .or(event.tool())
                .is_some_and(|tool| tool.is_delegate());
            let crowded = lock(&batch)
                .as_ref()
                .is_some_and(|batch| batch.tool_use_count > 1);
            if delegates && crowded && !event.agent().model().stateful() {
                event.set_selected_tool(Arc::new(RefusedDelegation));
            }
        },
    );

    let batch = Arc::clone(&state);
    hooks.add_hook(move |event: &mut AfterToolCallEvent| {
        let mut guard = lock(&batch);
        let delegates = event.tool().is_some_and(|tool| tool.is_delegate());
        if event.agent().model().stateful() || !delegates {
            // A retry may have swapped the delegation tool for a regular one.
            if let Some(batch) = guard.as_mut()
                && batch.tool_use_id.as_deref() == Some(event.tool_use().tool_use_id())
            {
                batch.tool_use_id = None;
            }
            return;
        }
        let Some(batch) = guard.as_mut() else {
            return;
        };
        batch.tool_use_id = (*event.result().status() != ToolResultStatus::Error)
            .then(|| event.tool_use().tool_use_id().to_owned());
    });

    let batch = Arc::clone(&state);
    hooks.add_hook_with(
        HookCallbackOptions::builder().order(HookOrder::SDK_LAST).build(),
        move |event: &mut AfterToolsEvent<'_>| {
            if event.agent().model().stateful() {
                return;
            }
            let mut guard = lock(&batch);
            let Some(batch) = guard.as_mut() else {
                return;
            };
            let Some(tool_use_id) = batch.tool_use_id.clone() else {
                return;
            };
            let result = event.message().content().iter().find_map(|block| match block {
                ContentBlock::ToolResult(result) if result.tool_use_id() == tool_use_id => Some(result),
                _ => None,
            });
            let Some(result) = result.filter(|result| *result.status() != ToolResultStatus::Error) else {
                batch.tool_use_id = None;
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
            let content = to_content_blocks(result);
            let blank = content
                .iter()
                .all(|block| matches!(block, ContentBlock::Text(text) if text.text().is_empty()));
            if blank {
                tracing::debug!(
                    "tool_use_id=<{tool_use_id}> | delegation produced blank content, skipping delegation"
                );
                return;
            }
            drop(guard);
            event.set_end_turn(content);
        },
    );

    hooks.add_hook(move |_: &mut BeforeModelCallEvent<'_>| {
        *lock(&state) = None;
    });
}

/// The result's content as assistant content; JSON becomes its text (TS `toContentBlocks`).
fn to_content_blocks(result: &ToolResultBlock) -> Vec<ContentBlock> {
    result
        .content()
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text(text) => Some(text.clone().into()),
            ToolResultContent::Json(json) => {
                Some(TextBlock::new(to_js_string(json.json()).unwrap_or_default()).into())
            }
            ToolResultContent::Image(image) => Some(image.clone().into()),
            ToolResultContent::Video(video) => Some(video.clone().into()),
            ToolResultContent::Document(document) => Some(document.clone().into()),
            _ => None,
        })
        .collect()
}

/// Runs in place of a delegation tool that shares its turn with other tools.
struct RefusedDelegation;

impl Tool for RefusedDelegation {
    fn name(&self) -> &str {
        "refused_delegation"
    }

    fn description(&self) -> &str {
        EXECUTION_REFUSED
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(self.name(), self.description())
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let result = error_text_result(context.tool_use().tool_use_id(), EXECUTION_REFUSED);
        ToolStream::from_future(async move { ToolOutcome::Result(result) })
    }
}

#[cfg(test)]
mod tests;
