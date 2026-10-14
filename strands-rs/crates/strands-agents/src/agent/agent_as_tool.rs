//! `Agent::as_tool`: expose an agent as a tool another agent can call
//! (`strands-ts/src/agent/agent-as-tool.ts`).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::json;

use crate::agent::{Agent, InvokeInput};
use crate::error::{Error, Result};
use crate::interrupt::{Interrupt, InterruptSource};
use crate::tools::{create_error_result, Tool, ToolContext, ToolRef};
use crate::types::{InterruptResponse, JsonValue, StopReason, ToolResultBlock, ToolResultContent, ToolSpec};

/// Namespace prefix for a sub-agent's interrupt ids: `agent_as_tool:{tool_use_id}:{inner_id}`.
const INTERRUPT_NAMESPACE: &str = "agent_as_tool";

/// Options for [`Agent::as_tool`]. Unset fields fall back to the agent's own name/description.
#[derive(Clone, Debug, Default)]
pub struct AgentAsToolOptions {
    /// Tool name; defaults to the agent's name.
    pub name: Option<String>,
    /// Tool description; defaults to the agent's description, else a generic sentence.
    pub description: Option<String>,
}

impl AgentAsToolOptions {
    /// Options overriding the tool name.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            description: None,
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl Agent {
    /// Wrap this agent as a tool: the caller sends a natural-language `prompt`, the child agent
    /// runs, and its final text (or structured output) becomes the tool result.
    ///
    /// Child stream events surface as tool progress; a child interrupt is re-raised in the
    /// calling agent with ids namespaced per tool call, and resumed by passing the answers back
    /// as [`InvokeInput::InterruptResponses`].
    pub fn as_tool(&self, options: AgentAsToolOptions) -> ToolRef {
        Arc::new(AgentAsTool::new(self.clone(), options))
    }
}

/// The [`Tool`] returned by [`Agent::as_tool`].
struct AgentAsTool {
    agent: Agent,
    name: String,
    description: String,
    spec: ToolSpec,
    /// Tool-use ids whose child turn interrupted and is waiting to be resumed.
    pending: Mutex<HashSet<String>>,
}

impl AgentAsTool {
    fn new(agent: Agent, options: AgentAsToolOptions) -> Self {
        let name = options.name.unwrap_or_else(|| agent.name().to_owned());
        let description = options
            .description
            .or_else(|| agent.description().map(str::to_owned));
        let description = description
            .unwrap_or_else(|| format!("Use the {name} agent by providing a natural language input"));
        let spec = ToolSpec::new(&name, &description).with_input_schema(input_schema());
        Self {
            agent,
            name,
            description,
            spec,
            pending: Mutex::new(HashSet::new()),
        }
    }

    /// The child agent's input for this call: fresh `prompt`, or the stripped interrupt answers
    /// when the parent is resuming an interrupted child turn.
    fn child_input(&self, ctx: &ToolContext, prefix: &str) -> Result<InvokeInput> {
        if self.pending.lock().contains(&ctx.tool_use.tool_use_id) {
            let responses = answered_responses(ctx, prefix);
            if !responses.is_empty() {
                return Ok(InvokeInput::InterruptResponses(
                    responses.into_iter().map(Into::into).collect(),
                ));
            }
        }
        #[derive(serde::Deserialize)]
        struct Input {
            prompt: String,
        }
        Ok(InvokeInput::Text(ctx.input::<Input>()?.prompt))
    }

    /// Register the child's interrupts in the parent under the namespaced ids and re-raise.
    fn raise_in_parent(&self, ctx: &ToolContext, prefix: &str, interrupts: Vec<Interrupt>) -> Error {
        let parent = ctx.agent.interrupt_state();
        let mut state = parent.lock();
        let raised = interrupts
            .into_iter()
            .map(|inner| {
                let outer = Interrupt {
                    id: format!("{prefix}{}", inner.id),
                    name: inner.name,
                    reason: inner.reason,
                    response: None,
                    source: InterruptSource::Tool,
                };
                state.register(&outer)
            })
            .collect();
        Error::Interrupt(raised)
    }
}

/// Verbatim tool input schema (`agent-as-tool.ts`), with `prompt` as the input key.
fn input_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "prompt": { "type": "string", "description": "The natural language input to send to the agent." }
        },
        "required": ["prompt"]
    })
}

/// `encodeURIComponent` semantics, so a tool-use id containing `:` cannot forge the delimiter.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn interrupt_prefix(tool_use_id: &str) -> String {
    format!("{INTERRUPT_NAMESPACE}:{}:", percent_encode(tool_use_id))
}

/// The parent's answered interrupts for this call, with the namespace prefix stripped back off.
fn answered_responses(ctx: &ToolContext, prefix: &str) -> Vec<InterruptResponse> {
    let parent = ctx.agent.interrupt_state();
    let state = parent.lock();
    state
        .list()
        .iter()
        .filter(|i| i.id.starts_with(prefix))
        .filter_map(|i| {
            i.response.clone().map(|response| InterruptResponse {
                interrupt_id: i.id[prefix.len()..].to_owned(),
                response,
            })
        })
        .collect()
}

#[async_trait]
impl Tool for AgentAsTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let tool_use_id = ctx.tool_use.tool_use_id.clone();
        let prefix = interrupt_prefix(&tool_use_id);

        let input = match self.child_input(&ctx, &prefix) {
            Ok(input) => input,
            Err(error) => return Ok(create_error_result(error, &tool_use_id)),
        };

        let mut stream = self.agent.stream(input);
        let mut result = None;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    if let Some(agent_result) = event.result() {
                        result = Some(agent_result.clone());
                    }
                    ctx.progress(json!({ "agent": self.name, "event": event.type_name() }))
                        .await;
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => return Ok(create_error_result(error, &tool_use_id)),
            }
        }
        let Some(result) = result else {
            return Ok(create_error_result(
                format!("Agent '{}' produced no result", self.name),
                &tool_use_id,
            ));
        };

        match result.stop_reason {
            StopReason::Interrupt => {
                if result.interrupts.is_empty() {
                    return Ok(create_error_result(
                        format!("Agent '{}' interrupted without any interrupts", self.name),
                        &tool_use_id,
                    ));
                }
                self.pending.lock().insert(tool_use_id.clone());
                Err(self.raise_in_parent(&ctx, &prefix, result.interrupts))
            }
            StopReason::Cancelled => {
                self.pending.lock().remove(&tool_use_id);
                Ok(create_error_result(
                    format!("Agent '{}' cancelled", self.name),
                    &tool_use_id,
                ))
            }
            _ => {
                self.pending.lock().remove(&tool_use_id);
                let content = match &result.structured_output {
                    Some(output) => ToolResultContent::Json(output.clone()),
                    None => ToolResultContent::text(result.text()),
                };
                Ok(ToolResultBlock::success(&tool_use_id, vec![content]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child() -> Agent {
        crate::agent::test_agent()
    }

    #[test]
    fn defaults_name_and_description_from_the_agent() {
        let tool = child().as_tool(AgentAsToolOptions::default());
        assert_eq!(tool.name(), "test-agent");
        assert_eq!(
            tool.description(),
            "Use the test-agent agent by providing a natural language input"
        );
    }

    #[test]
    fn options_override_name_and_description() {
        let tool = child().as_tool(AgentAsToolOptions::named("researcher").with_description("Researches"));
        assert_eq!(tool.name(), "researcher");
        assert_eq!(tool.description(), "Researches");
        assert_eq!(tool.spec().name, "researcher");
        assert_eq!(tool.spec().description, "Researches");
    }

    #[test]
    fn input_schema_is_a_required_prompt_string() {
        let tool = child().as_tool(AgentAsToolOptions::default());
        assert_eq!(
            tool.spec().input_schema.clone().unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The natural language input to send to the agent."
                    }
                },
                "required": ["prompt"]
            })
        );
    }

    #[test]
    fn interrupt_ids_are_namespaced_per_tool_call_with_the_id_escaped() {
        assert_eq!(interrupt_prefix("tool:1"), "agent_as_tool:tool%3A1:");
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
        assert_eq!(percent_encode("keep-_.!~*'()"), "keep-_.!~*'()");
    }
}
