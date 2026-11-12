//! An agent wrapped as a tool of another agent (TS `agent/agent-as-tool.ts`).

use std::fmt;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;
use tokio::sync::Mutex;

use crate::agent::{Agent, AgentStreamEvent, InvokeArgs, InvokeOptions};
use crate::state_store::StateStore;
use crate::tools::tool::create_error_result;
use crate::tools::{
    Tool, ToolContext, ToolOutcome, ToolResultStatus, ToolSpec, ToolStream, ToolStreamEvent, ToolStreamItem,
};
use crate::types::messages::limit_stop_message;
use crate::types::{JsonBlock, Message, StopReason, ToolResultBlock, ToolResultContent};

/// Appended to a delegating tool's description (TS `DELEGATION_DESCRIPTION_SUFFIX`).
pub const DELEGATION_DESCRIPTION_SUFFIX: &str = " Calling this tool will return its response directly to the user as the final answer. It should be the only tool called in the turn.";

/// Options for [`Agent::as_tool`] (TS `AgentAsToolOptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
pub struct AgentAsToolOptions {
    /// The tool name. Defaults to the agent name.
    #[builder(into)]
    pub name: Option<String>,
    /// The tool description. Defaults to the agent description, else a generic one.
    #[builder(into)]
    pub description: Option<String>,
    /// Keep the agent's history between calls. Defaults to `false`: each call starts from the
    /// agent's state when it was wrapped.
    #[builder(default)]
    pub preserve_context: bool,
    /// Return the agent's answer to the user as the final answer.
    #[builder(default)]
    pub delegate: bool,
}

struct Initial {
    messages: Vec<Message>,
    app_state: StateStore,
}

/// An agent registered as a tool (TS `AgentAsTool`).
pub struct AgentAsTool {
    agent: Arc<Mutex<Agent>>,
    spec: ToolSpec,
    delegate: bool,
    initial: Option<Arc<Initial>>,
}

impl AgentAsTool {
    pub(crate) fn new(agent: Agent, options: AgentAsToolOptions) -> Self {
        let name = options.name.unwrap_or_else(|| agent.name().to_owned());
        let mut description = options
            .description
            .or_else(|| agent.description().map(str::to_owned))
            .unwrap_or_else(|| format!("Use the {name} agent by providing a natural language input"));
        if options.delegate {
            description.push_str(DELEGATION_DESCRIPTION_SUFFIX);
        }
        let initial = (!options.preserve_context).then(|| {
            Arc::new(Initial {
                messages: agent.messages().to_vec(),
                app_state: agent.app_state().snapshot(),
            })
        });
        let spec = ToolSpec::new(name, description).with_input_schema(json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "The natural language input to send to the agent." },
            },
            "required": ["input"],
        }));
        Self {
            agent: Arc::new(Mutex::new(agent)),
            spec,
            delegate: options.delegate,
            initial,
        }
    }

    /// Whether the agent's answer is returned to the user directly.
    pub fn delegate(&self) -> bool {
        self.delegate
    }

    /// The wrapped agent.
    pub fn agent(&self) -> &Arc<Mutex<Agent>> {
        &self.agent
    }
}

impl Tool for AgentAsTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let agent = Arc::clone(&self.agent);
        let initial = self.initial.clone();
        let name = self.spec.name.clone();
        ToolStream::new(async_stream::stream! {
            let tool_use_id = context.tool_use().tool_use_id.clone();
            let Ok(mut agent) = agent.try_lock() else {
                let message = format!("Agent '{name}' is already processing a request");
                yield ToolStreamItem::Outcome(ToolOutcome::Result(create_error_result(message, &tool_use_id)));
                return;
            };
            if let Some(initial) = &initial {
                *agent.messages_mut() = initial.messages.clone();
                agent.app_state().load_json(initial.app_state.to_json());
            }
            let input = context.tool_use().input.get("input").and_then(|input| input.as_str()).unwrap_or_default();
            let options = InvokeOptions {
                invocation_state: Some(context.invocation_state().clone()),
                cancel_signal: Some(context.cancel_signal().clone()),
                invocation_context: context.invocation_context().cloned(),
                ..Default::default()
            };
            let mut outcome = None;
            {
                let mut stream = agent.stream_with(InvokeArgs::from(input), options);
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(AgentStreamEvent::AgentResult(result)) => outcome = Some(Ok(result)),
                        Ok(AgentStreamEvent::ToolStreamUpdate(update)) => yield ToolStreamItem::Event(update.event),
                        Ok(event) => match serde_json::to_value(&event) {
                            Ok(data) => yield ToolStreamItem::Event(ToolStreamEvent::new(data)),
                            Err(error) => tracing::debug!(error = %error, "could not serialize sub-agent event"),
                        },
                        Err(error) => outcome = Some(Err(error)),
                    }
                }
            }
            let label = format!("Agent '{name}'");
            let result = match outcome {
                Some(Ok(result)) if result.stop_reason() == StopReason::Interrupt => {
                    create_error_result(format!("{label} interrupted without any interrupts"), &tool_use_id)
                }
                Some(Ok(result))
                    if result.stop_reason() == StopReason::Cancelled && agent.cancel_signal().is_cancelled() =>
                {
                    create_error_result(format!("{label} cancelled"), &tool_use_id)
                }
                Some(Ok(result)) if result.stop_reason().is_limit() => {
                    create_error_result(limit_stop_message(&name, &result.stop_reason()), &tool_use_id)
                }
                Some(Ok(result)) => {
                    let content = match result.structured_output_value() {
                        Some(output) => ToolResultContent::Json(JsonBlock::new(output.clone())),
                        None => ToolResultContent::from(result.to_string()),
                    };
                    ToolResultBlock::new(&tool_use_id, ToolResultStatus::Success, vec![content])
                }
                Some(Err(error)) => create_error_result(error, &tool_use_id),
                None => create_error_result(format!("Agent '{name}' produced no result"), &tool_use_id),
            };
            yield ToolStreamItem::Outcome(ToolOutcome::Result(result));
        })
    }
}

impl fmt::Debug for AgentAsTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentAsTool")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Wraps this agent as a tool for another agent (TS `agent.asTool`).
    pub fn as_tool(self, options: AgentAsToolOptions) -> AgentAsTool {
        AgentAsTool::new(self, options)
    }
}

#[cfg(test)]
mod tests;
