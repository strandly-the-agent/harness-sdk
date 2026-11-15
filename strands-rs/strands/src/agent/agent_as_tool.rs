//! An agent exposed as a tool for another agent (TS `agent/agent-as-tool.ts`).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::StreamExt;
use serde_json::json;

use crate::agent::Agent;
use crate::interrupt::state::InterruptStateData;
use crate::interrupt::{Interrupt, InterruptResponse, InterruptSource};
use crate::tools::tool::error_result;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::types::agent::{AgentResult, AgentStreamEvent, InterruptState, InvokeArgs, InvokeOptions};
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{
    JsonBlock, Message, StopReason, SystemPrompt, ToolResultBlock, ToolResultContent, limit_stop_message,
};
use crate::types::tools::{ToolResultStatus, ToolSpec};

/// Appended to a delegation tool's description to guide the model (TS
/// `DELEGATION_DESCRIPTION_SUFFIX`).
pub(crate) const DELEGATION_DESCRIPTION_SUFFIX: &str = " Calling this tool will return its response directly to the user as the final answer. It should be the only tool called in the turn.";

/// Options for [`Agent::as_tool`] (TS `AgentAsToolOptions`).
///
/// ```
/// use strands::agent::AgentAsToolOptions;
///
/// let options = AgentAsToolOptions::builder().name("researcher").delegate(true).build();
/// assert!(!options.preserve_context);
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct AgentAsToolOptions {
    /// The tool name the parent's model sees; defaults to the agent's name. It must match
    /// `[a-zA-Z0-9_-]{1,64}`, which registration checks.
    pub name: Option<String>,
    /// The tool description; defaults to the agent's description, else
    /// `Use the <name> agent by providing a natural language input`.
    pub description: Option<String>,
    /// Keep the agent's history and state across calls (default `false`: every call starts from
    /// the history, state and system prompt the agent had when the tool was created).
    #[builder(default)]
    pub preserve_context: bool,
    /// The parent ends its turn with this tool's result as the final answer, without another
    /// model call. The description gains a note that it must be the only tool in the turn.
    #[builder(default)]
    pub delegate: bool,
}

/// An [`Agent`] wrapped as a tool taking one `input` string (TS `AgentAsTool`); built by
/// [`Agent::as_tool`], or implicitly when an agent is passed to `.tools(..)`.
///
/// The result is the agent's text answer, or its structured output as a JSON block. One call
/// runs at a time; a second concurrent call gets an error result.
///
/// ```
/// use strands::agent::AgentAsToolOptions;
/// use strands::testing::MockMessageModel;
/// use strands::{Agent, Tool};
///
/// let researcher = Agent::builder()
///     .model(MockMessageModel::new())
///     .name("researcher")
///     .description("Finds information on a topic")
///     .build()?;
/// let tool = researcher.as_tool(AgentAsToolOptions::default());
/// assert_eq!(tool.name(), "researcher");
/// let writer = Agent::builder().model(MockMessageModel::new()).tools([tool]).build()?;
/// assert!(writer.tool_registry().get("researcher").is_some());
/// # Ok::<(), strands::Error>(())
/// ```
pub struct AgentAsTool {
    name: String,
    description: String,
    delegate: bool,
    agent: Arc<Mutex<Option<Agent>>>,
    baseline: Option<Arc<Baseline>>,
    /// Paused turns of a context-resetting child, by tool use id (TS `subAgentInterruptedTurns`).
    turns: Arc<Mutex<HashMap<String, Baseline>>>,
}

impl fmt::Debug for AgentAsTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentAsTool")
            .field("name", &self.name)
            .field("delegate", &self.delegate)
            .field("preserve_context", &self.baseline.is_none())
            .finish_non_exhaustive()
    }
}

impl AgentAsTool {
    fn new(agent: Agent, options: AgentAsToolOptions) -> Self {
        let name = options.name.unwrap_or_else(|| agent.name().to_owned());
        let mut description = options
            .description
            .or_else(|| agent.description().map(str::to_owned))
            .unwrap_or_else(|| format!("Use the {name} agent by providing a natural language input"));
        if options.delegate {
            description.push_str(DELEGATION_DESCRIPTION_SUFFIX);
        }
        let baseline = (!options.preserve_context).then(|| Arc::new(Baseline::of(&agent)));
        Self {
            name,
            description,
            delegate: options.delegate,
            agent: Arc::new(Mutex::new(Some(agent))),
            baseline,
            turns: Arc::default(),
        }
    }

    /// Whether the parent ends its turn with this tool's result (TS `delegate`).
    pub fn delegate(&self) -> bool {
        self.delegate
    }

    /// Runs `f` on the wrapped agent (TS `agent` getter); `None` while a call is running.
    pub fn with_agent<R>(&self, f: impl FnOnce(&mut Agent) -> R) -> Option<R> {
        lock(&self.agent).as_mut().map(f)
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
        ToolSpec::new(self.name.as_str(), self.description.as_str()).with_input_schema(json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The natural language input to send to the agent."
                }
            },
            "required": ["input"]
        }))
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let slot = Arc::clone(&self.agent);
        let baseline = self.baseline.clone();
        let turns = Arc::clone(&self.turns);
        let name = self.name.clone();
        let delegate = self.delegate;
        ToolStream::new(async_stream::stream! {
            let tool_use_id = context.tool_use().tool_use_id().to_owned();
            let parent = context.agent().interrupt_state().clone();
            let prefix = format!("agent_as_tool:{}:", encode_uri_component(&tool_use_id));
            let Some(mut lease) = Lease::take(slot) else {
                let text = format!("Agent '{name}' is already processing a request");
                yield ToolOutcome::Result(error_result(&tool_use_id, text)).into();
                return;
            };
            let Some(agent) = lease.agent.as_mut() else {
                return;
            };
            let input = match resume_input(&parent, &prefix) {
                Resume::Fresh => {
                    let Some(input) = context.tool_use().input().get("input").and_then(JsonValue::as_str) else {
                        let text = format!("Agent '{name}' needs a string `input`");
                        yield ToolOutcome::Result(error_result(&tool_use_id, text)).into();
                        return;
                    };
                    if let Some(baseline) = &baseline {
                        baseline.restore(agent);
                    }
                    InvokeArgs::from(input)
                }
                Resume::Unanswered(interrupt) => {
                    yield ToolOutcome::Interrupt(interrupt).into();
                    return;
                }
                Resume::With(responses) => {
                    // A context-resetting child resumes from the turn stored when it paused.
                    let restored = baseline.is_none()
                        || lock(&turns).remove(&tool_use_id).map(|turn| turn.restore(agent)).is_some();
                    if !restored || !agent.local.interrupt_state().lock().activated {
                        tracing::error!(tool_use_id = %tool_use_id, "interrupted sub-agent turn is not available to resume");
                        let text = format!(
                            "Agent '{name}' did NOT run and the human's response was NOT applied: its interrupted turn \
                             is not available. Do not report the requested action as completed; tell the user it failed and \
                             ask them to respond again."
                        );
                        yield ToolOutcome::Result(error_result(&tool_use_id, text)).into();
                        return;
                    }
                    tracing::debug!(tool_name = %name, tool_use_id = %tool_use_id, "resuming sub-agent from interrupt");
                    InvokeArgs::from(responses)
                }
            };
            // The outer invocation's state and request are shared, so the child's writes and
            // usage reach the parent.
            let options = InvokeOptions {
                invocation_state: context.invocation_state().clone(),
                invocation_context: context.invocation_context().cloned(),
                ..InvokeOptions::default()
            };
            let mut result = None;
            let mut failure = None;
            {
                let mut events = agent.stream_with(input, options);
                while let Some(item) = events.next().await {
                    match item {
                        Ok(AgentStreamEvent::ToolStreamUpdate(update)) => {
                            yield ToolStreamItem::Event(update.event().clone());
                        }
                        Ok(event) => {
                            if let AgentStreamEvent::AgentResult(last) = &event {
                                result = Some(last.clone());
                            }
                            yield ToolStreamItem::Event(ToolStreamEvent::wrapping(event, delegate));
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            let cancelled = agent.local.cancel_signal().is_cancelled();
            let interrupted = result
                .as_ref()
                .filter(|result| failure.is_none() && result.stop_reason() == StopReason::Interrupt && !result.interrupts().is_empty());
            if interrupted.is_some() && baseline.is_some() {
                lock(&turns).insert(tool_use_id.clone(), Baseline::of(agent));
            } else {
                lock(&turns).remove(&tool_use_id);
            }
            drop(lease);
            if let Some(interrupt) = interrupted.and_then(|result| register(&parent, &prefix, result.interrupts())) {
                yield ToolOutcome::Interrupt(interrupt).into();
                return;
            }
            let outcome = match (failure, result) {
                (Some(error), _) => error_result(&tool_use_id, error),
                (None, None) => error_result(&tool_use_id, format!("Agent '{name}' did not produce a result.")),
                (None, Some(result)) => match unfinished(&name, &result, cancelled) {
                    Some(text) => error_result(&tool_use_id, text),
                    None => ToolResultBlock::new(&tool_use_id, ToolResultStatus::Success, render(&result)),
                },
            };
            yield ToolOutcome::Result(outcome).into();
        })
    }

    fn is_delegate(&self) -> bool {
        self.delegate
    }
}

/// Why a finished child call doesn't count as an answer (TS `ChildAgentAdapter.classify`).
fn unfinished(name: &str, result: &AgentResult, cancelled: bool) -> Option<String> {
    let stop_reason = result.stop_reason();
    match stop_reason {
        StopReason::Interrupt => Some(format!("Agent '{name}' interrupted without any interrupts")),
        StopReason::Cancelled => cancelled.then(|| format!("Agent '{name}' cancelled")),
        _ if stop_reason.is_limit() => Some(limit_stop_message(name, &stop_reason)),
        _ => None,
    }
}

/// How a call relates to the parent's interrupt state (TS `AgentToolStreamer.isResuming`/`resume`).
enum Resume {
    /// Not resuming this call.
    Fresh,
    /// Resuming, but none of this call's interrupts were answered.
    Unanswered(Interrupt),
    /// This call's responses, with the scope prefix removed.
    With(Vec<InterruptResponse>),
}

fn resume_input(parent: &InterruptState, prefix: &str) -> Resume {
    let state = parent.lock();
    if !state.activated || !state.interrupts.keys().any(|id| id.starts_with(prefix)) {
        return Resume::Fresh;
    }
    let responses: Vec<InterruptResponse> = state
        .resume_responses
        .iter()
        .flatten()
        .filter_map(|content| {
            let response = content.interrupt_response();
            let id = response.interrupt_id().strip_prefix(prefix)?;
            Some(InterruptResponse::new(id, response.response().clone()))
        })
        .collect();
    if !responses.is_empty() {
        return Resume::With(responses);
    }
    match state
        .unanswered()
        .find(|interrupt| interrupt.id().starts_with(prefix))
    {
        Some(interrupt) => Resume::Unanswered(interrupt.clone()),
        None => Resume::With(Vec::new()),
    }
}

/// Registers the child's interrupts on the parent under this call's scope and returns the
/// first; all are registered because a tool outcome carries one interrupt.
fn register(parent: &InterruptState, prefix: &str, interrupts: &[Interrupt]) -> Option<Interrupt> {
    let mut state = parent.lock();
    let registered: Vec<Interrupt> = interrupts
        .iter()
        .map(|interrupt| {
            let id = format!("{prefix}{}", interrupt.id());
            let scoped = Interrupt::new(id.clone(), interrupt.name()).with_source(InterruptSource::Tool);
            let scoped = match interrupt.reason() {
                Some(reason) => scoped.with_reason(reason.clone()),
                None => scoped,
            };
            state.interrupts.entry(id).or_insert(scoped).clone()
        })
        .collect();
    registered.into_iter().next()
}

/// JavaScript `encodeURIComponent`.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// TS `_renderSuccess`: the structured output as JSON, else the result text.
fn render(result: &AgentResult) -> Vec<ToolResultContent> {
    match result.structured_output_value() {
        Some(json) => vec![ToolResultContent::Json(JsonBlock::new(json.clone()))],
        None => vec![result.to_string().into()],
    }
}

/// What a context-resetting agent tool restores before each call (TS `takeSnapshot` with the
/// `session` preset).
struct Baseline {
    messages: Vec<Message>,
    system_prompt: Option<SystemPrompt>,
    app_state: JsonObject,
    model_state: JsonObject,
    interrupts: InterruptStateData,
}

impl Baseline {
    fn of(agent: &Agent) -> Self {
        Self {
            messages: agent.messages().to_vec(),
            system_prompt: agent.system_prompt().cloned(),
            app_state: agent.app_state().get_all(),
            model_state: agent.model_state().get_all(),
            interrupts: agent.local.interrupt_state().lock().clone(),
        }
    }

    fn restore(&self, agent: &mut Agent) {
        agent.local.set_messages(self.messages.clone());
        agent.local.set_system_prompt(self.system_prompt.clone());
        agent
            .app_state()
            .load_json(JsonValue::Object(self.app_state.clone()));
        agent
            .model_state()
            .load_json(JsonValue::Object(self.model_state.clone()));
        *agent.local.interrupt_state().lock() = self.interrupts.clone();
    }
}

/// The wrapped agent, taken out of its slot for one call and put back on drop.
struct Lease {
    slot: Arc<Mutex<Option<Agent>>>,
    agent: Option<Agent>,
}

impl Lease {
    fn take(slot: Arc<Mutex<Option<Agent>>>) -> Option<Self> {
        let agent = lock(&slot).take()?;
        Some(Self {
            slot,
            agent: Some(agent),
        })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            *lock(&self.slot) = Some(agent);
        }
    }
}

fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Agent {
    /// Wraps this agent as a tool for another agent (TS `asTool`). Passing an agent to
    /// `.tools(..)` does the same with default options.
    pub fn as_tool(self, options: AgentAsToolOptions) -> AgentAsTool {
        AgentAsTool::new(self, options)
    }
}

#[cfg(test)]
mod tests;
