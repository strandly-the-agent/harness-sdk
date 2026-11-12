//! Direct tool calls (TS `agent/tool-caller.ts`): `agent.tool("name").invoke(input)`.

use futures::StreamExt;

use crate::agent::{Agent, InvocationState};
use crate::error::{AgentError, Result};
use crate::hooks::{MessageAddedEvent, kinds};
use crate::tools::{ToolContext, ToolOutcome, ToolStreamItem, ToolUse};
use crate::types::{ContentBlock, JsonValue, Message, Role, ToolResultBlock, ToolUseBlock};

/// Options for a direct tool call (TS `DirectToolCallOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, bon::Builder)]
pub struct DirectToolCallOptions {
    /// Record the call in the history as a tool use, its result and a note. Defaults to `true`.
    #[builder(default = true)]
    pub record_direct_tool_call: bool,
}

impl Default for DirectToolCallOptions {
    fn default() -> Self {
        Self {
            record_direct_tool_call: true,
        }
    }
}

/// A registered tool, called directly rather than by the model (TS `ToolHandle`).
#[derive(Debug)]
pub struct ToolHandle<'a> {
    agent: &'a mut Agent,
    name: String,
}

impl Agent {
    /// A handle for calling the tool `name` directly (TS `agent.tool.<name>`). The name
    /// resolves as the registry does: exact, `_` for `-`, then case-insensitive.
    pub fn tool(&mut self, name: impl Into<String>) -> ToolHandle<'_> {
        ToolHandle {
            agent: self,
            name: name.into(),
        }
    }
}

impl ToolHandle<'_> {
    /// Calls the tool with `input` and records the call in the history.
    ///
    /// # Errors
    ///
    /// [`ToolNotFoundError`](crate::error::ToolNotFoundError) when no tool matches, and an
    /// [`AgentError`] when the tool interrupts (interrupts need an invocation to resume).
    pub async fn invoke(self, input: JsonValue) -> Result<ToolResultBlock> {
        self.invoke_with(input, DirectToolCallOptions::default()).await
    }

    /// [`invoke`](Self::invoke) with options.
    ///
    /// # Errors
    ///
    /// See [`invoke`](Self::invoke).
    pub async fn invoke_with(
        self,
        input: JsonValue,
        options: DirectToolCallOptions,
    ) -> Result<ToolResultBlock> {
        let agent = self.agent;
        let tool = std::sync::Arc::clone(agent.local.tool_registry.resolve(&self.name)?);
        let tool_use = ToolUse::new(tool.name(), format!("tooluse_{}", uuid::Uuid::new_v4()), input);
        let context = ToolContext {
            tool_use: tool_use.clone(),
            agent: agent.local.tool_agent(None),
            invocation_state: InvocationState::new(),
            cancel_signal: agent.local.cancel_signal.clone(),
            invocation_context: None,
        };
        let mut stream = tool.stream(context);
        let mut outcome = None;
        while let Some(item) = stream.next().await {
            if let ToolStreamItem::Outcome(found) = item {
                outcome = Some(found);
                break;
            }
        }
        let result = match outcome {
            Some(ToolOutcome::Result(result)) => result,
            Some(ToolOutcome::Interrupt(_)) => {
                return Err(AgentError::new("Interrupts are not supported in direct tool calls").into());
            }
            None => crate::tools::tool::error_result(
                &tool_use.tool_use_id,
                format!("Tool '{}' did not return a result", tool_use.name),
            ),
        };
        if options.record_direct_tool_call {
            record(agent, &tool_use, &result).await;
        }
        Ok(result)
    }
}

/// Appends the call to the history as TS does (`_recordToolExecution`).
async fn record(agent: &mut Agent, tool_use: &ToolUse, result: &ToolResultBlock) {
    let block = ToolUseBlock::new(&tool_use.name, &tool_use.tool_use_id, tool_use.input.clone());
    let note = format!("agent.tool.{} was called.", tool_use.name);
    let messages = [
        Message::new(Role::Assistant, vec![ContentBlock::ToolUse(block)]),
        Message::new(Role::User, vec![ContentBlock::ToolResult(result.clone())]),
        Message::new(Role::Assistant, vec![ContentBlock::from(note.as_str())]),
    ];
    let hooks = agent.local.hooks.clone();
    for message in messages {
        agent.local.messages.push(message.clone());
        let mut event = MessageAddedEvent::new(&mut agent.local, InvocationState::new(), None, message);
        hooks.fire::<kinds::MessageAdded>(&mut event).await;
    }
}

#[cfg(test)]
mod tests;
