//! An agent used as another agent's tool (TS `agent/agent-as-tool.ts`).

use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::Mutex;

use crate::agent::agent::Agent;
use crate::agent::invoke::InvokeOptions;
use crate::agent::stream_event::AgentStreamEvent;
use crate::error::Error;
use crate::state_store::StateStore;
use crate::tools::context::ToolContext;
use crate::tools::tool::{Tool, ToolError, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::tools::tool_source::ToolSource;
use crate::types::agent::AgentResult;
use crate::types::json::JsonValue;
use crate::types::messages::{
    JsonBlock, Message, StopReason, SystemPrompt, ToolResultBlock, ToolResultContent,
};
use crate::types::tools::{ToolResultStatus, ToolSpec};

/// What a delegating agent tool appends to its description (TS `DELEGATION_DESCRIPTION_SUFFIX`).
pub const DELEGATION_DESCRIPTION_SUFFIX: &str = " Calling this tool will return its response directly to the user as the final answer. It should be the only tool called in the turn.";

/// How [`Agent::as_tool`] wraps an agent (TS `AgentAsToolOptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
pub struct AgentAsToolOptions {
    /// The tool name. Defaults to the agent's name.
    #[builder(into)]
    pub name: Option<String>,
    /// The tool description. Defaults to the agent's description, else `Use the <name> agent by
    /// providing a natural language input`.
    #[builder(into)]
    pub description: Option<String>,
    /// Keep the agent's conversation between calls. By default every call starts from the
    /// agent's state when it was wrapped.
    #[builder(default)]
    pub preserve_context: bool,
    /// Return the agent's answer directly to the user as the caller's final answer, ending the
    /// caller's turn (TS `delegate`).
    #[builder(default)]
    pub delegate: bool,
}

/// An agent wrapped as a tool (TS `AgentAsTool`). Build it with [`Agent::as_tool`], or pass an
/// agent to `.tools(..)` directly.
///
/// The tool takes `{ "input": "<natural language input>" }`, runs the agent with the caller's
/// invocation state, cancel signal and request, streams the agent's events as tool progress, and
/// returns the agent's answer: its structured output as JSON when it has one, else its text. Only
/// one call runs at a time; a concurrent call gets an error result.
///
/// ```
/// use strands::agent::AgentAsToolOptions;
/// use strands::testing::{MockModel, test_agent};
/// use strands::Tool;
///
/// let researcher = test_agent(MockModel::new()).name("researcher").build()?;
/// let tool = researcher.as_tool(AgentAsToolOptions::default());
/// assert_eq!(tool.name(), "researcher");
/// assert_eq!(tool.description(), "Use the researcher agent by providing a natural language input");
/// # Ok::<(), strands::Error>(())
/// ```
pub struct AgentAsTool {
    name: String,
    description: String,
    delegate: bool,
    agent: Arc<Mutex<Agent>>,
    initial: Option<Arc<InitialState>>,
}

/// The state a call restores when context isn't preserved (TS the `session` snapshot preset).
struct InitialState {
    messages: Vec<Message>,
    system_prompt: Option<SystemPrompt>,
    app_state: serde_json::Map<String, JsonValue>,
    model_state: serde_json::Map<String, JsonValue>,
}

impl InitialState {
    fn take(agent: &Agent) -> Self {
        Self {
            messages: agent.local.messages().to_vec(),
            system_prompt: agent.local.system_prompt().cloned(),
            app_state: agent.local.app_state().get_all(),
            model_state: agent.local.model_state().get_all(),
        }
    }

    fn restore(&self, agent: &mut Agent) {
        agent.local.set_messages(self.messages.clone());
        agent.local.set_system_prompt(self.system_prompt.clone());
        replace_state(agent.local.app_state(), &self.app_state);
        replace_state(agent.local.model_state(), &self.model_state);
    }
}

fn replace_state(store: &StateStore, values: &serde_json::Map<String, JsonValue>) {
    store.load_state_from_json(JsonValue::Object(values.clone()));
}

impl Agent {
    /// Wraps the agent as a tool another agent can call (TS `asTool`). The tool owns the agent.
    #[must_use]
    pub fn as_tool(self, options: AgentAsToolOptions) -> AgentAsTool {
        let name = options.name.unwrap_or_else(|| self.local.name().to_owned());
        let mut description = options
            .description
            .or_else(|| self.local.description().map(str::to_owned))
            .unwrap_or_else(|| format!("Use the {name} agent by providing a natural language input"));
        if options.delegate {
            description.push_str(DELEGATION_DESCRIPTION_SUFFIX);
        }
        let initial = (!options.preserve_context).then(|| Arc::new(InitialState::take(&self)));
        AgentAsTool {
            name,
            description,
            delegate: options.delegate,
            agent: Arc::new(Mutex::new(self)),
            initial,
        }
    }
}

impl From<Agent> for ToolSource {
    fn from(agent: Agent) -> Self {
        Self::from(agent.as_tool(AgentAsToolOptions::default()))
    }
}

impl AgentAsTool {
    /// Whether the tool returns the agent's answer as the caller's final answer.
    #[must_use]
    pub fn delegate(&self) -> bool {
        self.delegate
    }
}

impl Tool for AgentAsTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(self.name.clone(), self.description.clone()).with_input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The natural language input to send to the agent.",
                },
            },
            "required": ["input"],
        }))
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let name = self.name.clone();
        let agent = Arc::clone(&self.agent);
        let initial = self.initial.clone();
        ToolStream::new(async_stream::stream! {
            let tool_use_id = context.tool_use().tool_use_id.clone();
            let Ok(mut agent) = agent.try_lock_owned() else {
                let error = ToolError::msg(format!("Agent '{name}' is already processing a request"));
                yield ToolStreamItem::Outcome(ToolOutcome::from_error(tool_use_id, error));
                return;
            };
            if let Some(initial) = &initial {
                initial.restore(&mut agent);
            }
            let input = context
                .tool_use()
                .input
                .get("input")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_owned();
            let options = InvokeOptions {
                invocation_state: Some(context.invocation_state().clone()),
                cancel_signal: Some(context.cancel_signal().clone()),
                invocation_context: context.invocation_context().cloned(),
                ..InvokeOptions::default()
            };
            let mut outcome: Result<Option<AgentResult>, Error> = Ok(None);
            {
                let mut events = agent.stream_with(input, options);
                while let Some(event) = events.next().await {
                    match event {
                        Ok(AgentStreamEvent::ToolStreamUpdate(update)) => yield ToolStreamItem::Event(update.event),
                        Ok(AgentStreamEvent::AgentResult(result)) => {
                            let data = serde_json::to_value(AgentStreamEvent::AgentResult(result.clone())).unwrap_or_default();
                            yield ToolStreamItem::Event(ToolStreamEvent::new().with_data(data));
                            outcome = Ok(Some(result));
                        }
                        Ok(event) => {
                            let data = serde_json::to_value(&event).unwrap_or_default();
                            yield ToolStreamItem::Event(ToolStreamEvent::new().with_data(data));
                        }
                        Err(error) => outcome = Err(error),
                    }
                }
            }
            drop(agent);
            let cancelled = context.cancel_signal().is_cancelled();
            yield ToolStreamItem::Outcome(match outcome {
                Ok(Some(result)) => terminal_outcome(&name, &tool_use_id, &result, cancelled),
                Ok(None) => error_outcome(&tool_use_id, format!("Agent '{name}' did not produce a result.")),
                Err(error) => ToolOutcome::from_error(tool_use_id, ToolError::from(error)),
            });
        })
    }

    fn delegates(&self) -> bool {
        self.delegate
    }
}

impl std::fmt::Debug for AgentAsTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentAsTool")
            .field("name", &self.name)
            .field("delegate", &self.delegate)
            .finish_non_exhaustive()
    }
}

/// How the child's result ends the call (TS `ChildAgentAdapter.classify` and
/// `AgentToolStreamer.terminalResult`): an interrupt passes up, a cancel through the caller's
/// signal or a reached limit is an error result, anything else renders as success.
fn terminal_outcome(name: &str, tool_use_id: &str, result: &AgentResult, cancelled: bool) -> ToolOutcome {
    match result.stop_reason() {
        StopReason::Interrupt => match result.interrupts().first() {
            Some(interrupt) => ToolOutcome::Interrupt(interrupt.clone()),
            None => error_outcome(
                tool_use_id,
                format!("Agent '{name}' interrupted without any interrupts"),
            ),
        },
        StopReason::Cancelled if cancelled => error_outcome(tool_use_id, format!("Agent '{name}' cancelled")),
        stop_reason if is_limit(&stop_reason) => error_outcome(
            tool_use_id,
            format!(
                "Agent '{name}' stopped early ({}): a limit on this call was reached, so its answer is incomplete",
                stop_reason.as_str()
            ),
        ),
        _ => ToolOutcome::Result(render_success(tool_use_id, result)),
    }
}

fn is_limit(stop_reason: &StopReason) -> bool {
    matches!(
        stop_reason,
        StopReason::LimitTurns
            | StopReason::LimitTotalTokens
            | StopReason::LimitOutputTokens
            | StopReason::LimitStructuredOutputAttempts
    )
}

/// TS `createErrorResult`: the text as `Error: <text>`.
fn error_outcome(tool_use_id: &str, text: String) -> ToolOutcome {
    ToolOutcome::from_error(tool_use_id, ToolError::msg(text))
}

/// The agent's answer: structured output as JSON, else its text (TS `_renderSuccess`).
fn render_success(tool_use_id: &str, result: &AgentResult) -> ToolResultBlock {
    let content = match result.structured_output::<JsonValue>() {
        Ok(output) => ToolResultContent::from(JsonBlock::new(output)),
        Err(_) => ToolResultContent::from(result.to_string()),
    };
    ToolResultBlock::new(tool_use_id, ToolResultStatus::Success, vec![content])
}
