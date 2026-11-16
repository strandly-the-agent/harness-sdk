//! An agent exposed as a tool for another agent (TS `agent/agent-as-tool.ts`).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, TakeSnapshotOptions};
use crate::error::Error;
use crate::interrupt::{Interrupt, InterruptResponse, InterruptSource};
use crate::tools::tool::error_result;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::types::agent::{AgentResult, AgentStreamEvent, InterruptState, InvokeArgs, InvokeOptions};
use crate::types::json::JsonValue;
use crate::types::messages::{JsonBlock, StopReason, ToolResultBlock, ToolResultContent, limit_stop_message};
use crate::types::snapshot::Snapshot;
use crate::types::tools::{ToolResultStatus, ToolSpec};

/// Appended to a delegation tool's description to guide the model (TS
/// `DELEGATION_DESCRIPTION_SUFFIX`).
/// Where a context-resetting child's paused turn is kept in the parent's interrupt context (TS
/// `INTERRUPTED_TURNS_KEY`).
const INTERRUPTED_TURNS_KEY: &str = "subAgentInterruptedTurns";

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
/// let tool = researcher.as_tool(AgentAsToolOptions::default())?;
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
    preserve_context: bool,
    /// What each call of a context-resetting tool starts from (TS `_initialSnapshot`).
    initial_snapshot: Option<Arc<Snapshot>>,
    /// Why the tool can't be used (TS throws it from the constructor).
    conflict: Option<String>,
}

impl fmt::Debug for AgentAsTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentAsTool")
            .field("name", &self.name)
            .field("delegate", &self.delegate)
            .field("preserve_context", &self.preserve_context)
            .finish_non_exhaustive()
    }
}

impl AgentAsTool {
    /// The tool without the session-manager check, which `check` makes.
    pub(crate) fn new(agent: Agent, options: AgentAsToolOptions) -> Self {
        let name = options.name.unwrap_or_else(|| agent.name().to_owned());
        let mut description = options
            .description
            .or_else(|| agent.description().map(str::to_owned))
            .unwrap_or_else(|| format!("Use the {name} agent by providing a natural language input"));
        if options.delegate {
            description.push_str(DELEGATION_DESCRIPTION_SUFFIX);
        }
        let conflict = (!options.preserve_context && agent.session_manager().is_some()).then(|| {
            format!(
                "Agent '{}' has a SessionManager, which conflicts with preserveContext=false. The SessionManager \
                 persists conversation history externally, but preserveContext=false resets state between \
                 invocations. Use preserveContext=true or remove the SessionManager.",
                agent.name()
            )
        });
        let initial_snapshot = if options.preserve_context {
            None
        } else {
            agent
                .take_snapshot(&TakeSnapshotOptions::session())
                .ok()
                .map(Arc::new)
        };
        Self {
            name,
            description,
            delegate: options.delegate,
            agent: Arc::new(Mutex::new(Some(agent))),
            preserve_context: options.preserve_context,
            initial_snapshot,
            conflict,
        }
    }

    /// Fails when the agent has a session manager and the tool resets its context between calls.
    pub(crate) fn check(&self) -> crate::Result<()> {
        self.conflict
            .as_ref()
            .map_or(Ok(()), |text| Err(Error::other(text.clone())))
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
        let preserve_context = self.preserve_context;
        let initial_snapshot = self.initial_snapshot.clone();
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
                    if let Some(snapshot) = &initial_snapshot
                        && let Err(error) = agent.load_snapshot(snapshot)
                    {
                        yield ToolOutcome::Result(error_result(&tool_use_id, error)).into();
                        return;
                    }
                    InvokeArgs::from(input)
                }
                Resume::Unanswered(interrupt) => {
                    yield ToolOutcome::Interrupt(interrupt).into();
                    return;
                }
                Resume::With(responses) => {
                    // A context-preserving child restores its own turn from its session on
                    // initialization (TS `ChildAgentAdapter.restore`).
                    let initialized = match stored_turn(&parent, &tool_use_id) {
                        Some(_) => Ok(()),
                        None => agent.initialize().await,
                    };
                    let restored = match initialized {
                        Ok(()) => restore(agent, &parent, &prefix, &tool_use_id),
                        Err(error) => {
                            tracing::error!(tool_use_id = %tool_use_id, error = %error, "failed to restore interrupted sub-agent turn");
                            reraise(&parent, &prefix, &tool_use_id, None)
                        }
                    };
                    match restored {
                        Restored::Ready => {}
                        Restored::Interrupt(interrupt) => {
                            yield ToolOutcome::Interrupt(*interrupt).into();
                            return;
                        }
                        Restored::Unavailable => {
                            let text = format!(
                                "Agent '{name}' did NOT run and the human's response was NOT applied: its interrupted turn \
                                 is not available. Do not report the requested action as completed; tell the user it failed and \
                                 ask them to respond again."
                            );
                            yield ToolOutcome::Result(error_result(&tool_use_id, text)).into();
                            return;
                        }
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
                cancel_signal: Some(context.cancel_signal().abort_only()),
                ..InvokeOptions::default()
            };
            let mut result = None;
            let mut failure = None;
            // Read on the result event, before the child's cancel state resets (TS `classify`).
            let child_cancel = agent.cancel_handle();
            let mut cancelled = false;
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
                                cancelled = child_cancel.is_cancelled();
                            }
                            yield ToolStreamItem::Event(ToolStreamEvent::wrapping(event, delegate));
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            let interrupted = result
                .as_ref()
                .filter(|result| failure.is_none() && result.stop_reason() == StopReason::Interrupt && !result.interrupts().is_empty());
            match (interrupted, &result) {
                (Some(_), _) if preserve_context => {
                    if agent.session_manager().is_none() {
                        tracing::warn!(
                            tool_name = %name,
                            tool_use_id = %tool_use_id,
                            "interrupted sub-agent has preserveContext=true and no session manager, so its interrupt cannot be resumed after a restart"
                        );
                    }
                }
                (Some(_), _) => store_turn(&parent, &tool_use_id, agent),
                (None, Some(_)) => remove_turn(&parent, &tool_use_id),
                (None, None) => {}
            }
            drop(lease);
            if let Some(interrupt) = interrupted.and_then(|result| register(&parent, &prefix, result.interrupts())) {
                yield ToolOutcome::Interrupt(interrupt).into();
                return;
            }
            let outcome = match (failure, result) {
                (Some(error), _) => error_result(&tool_use_id, error),
                (None, None) => error_result(&tool_use_id, format!("Agent '{name}' did not produce a result.")),
                (None, Some(result)) => match unfinished(&format!("Agent '{name}'"), &name, &result, cancelled) {
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

/// Why a finished child call doesn't count as an answer (TS `ChildAgentAdapter.classify`);
/// `label` names the child in the message, `name` in a limit-stop message.
pub(crate) fn unfinished(label: &str, name: &str, result: &AgentResult, cancelled: bool) -> Option<String> {
    let stop_reason = result.stop_reason();
    match stop_reason {
        StopReason::Interrupt => Some(format!("{label} interrupted without any interrupts")),
        StopReason::Cancelled => cancelled.then(|| format!("{label} cancelled")),
        _ if stop_reason.is_limit() => Some(limit_stop_message(name, &stop_reason)),
        _ => None,
    }
}

/// How a call relates to the parent's interrupt state (TS `AgentToolStreamer.isResuming`/`resume`).
pub(crate) enum Resume {
    /// Not resuming this call.
    Fresh,
    /// Resuming, but none of this call's interrupts were answered.
    Unanswered(Interrupt),
    /// This call's responses, with the scope prefix removed.
    With(Vec<InterruptResponse>),
}

pub(crate) fn resume_input(parent: &InterruptState, prefix: &str) -> Resume {
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
pub(crate) fn register(parent: &InterruptState, prefix: &str, interrupts: &[Interrupt]) -> Option<Interrupt> {
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
pub(crate) fn encode_uri_component(value: &str) -> String {
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

/// How restoring a paused child turn went (TS `AgentToolStreamer.resume`).
pub(crate) enum Restored {
    /// The child awaits the responses.
    Ready,
    /// The stored turn didn't load; its interrupts are raised again, unanswered.
    Interrupt(Box<Interrupt>),
    /// There is no turn to resume.
    Unavailable,
}

/// Loads the call's stored turn into the child (a context-preserving child keeps its own) and
/// reports whether the child awaits interrupt responses.
pub(crate) fn restore(
    agent: &mut Agent,
    parent: &InterruptState,
    prefix: &str,
    tool_use_id: &str,
) -> Restored {
    let turn = stored_turn(parent, tool_use_id);
    let loaded = turn.as_ref().map_or(Ok(()), |turn| {
        serde_json::from_value::<Snapshot>(turn.clone())
            .map_err(Error::other)
            .and_then(|snapshot| agent.load_snapshot(&snapshot))
    });
    if let Err(error) = loaded {
        tracing::error!(tool_use_id = %tool_use_id, error = %error, "failed to restore interrupted sub-agent turn");
        return reraise(parent, prefix, tool_use_id, turn.as_ref());
    }
    remove_turn(parent, tool_use_id);
    if !agent.local.interrupt_state().lock().activated {
        tracing::error!(tool_use_id = %tool_use_id, "interrupted sub-agent turn is not available to resume");
        return Restored::Unavailable;
    }
    Restored::Ready
}

/// TS `_handleFailedRestore`: the call's interrupts the stored turn still awaits become
/// unanswered again, and the turn stays stored.
pub(crate) fn reraise(
    parent: &InterruptState,
    prefix: &str,
    tool_use_id: &str,
    turn: Option<&JsonValue>,
) -> Restored {
    let pending = turn
        .and_then(|turn| turn.get("data")?.get("interrupts")?.get("interrupts"))
        .and_then(JsonValue::as_object);
    let Some(pending) = pending else {
        tracing::error!(tool_use_id = %tool_use_id, "stored turn has no readable interrupt state");
        return Restored::Unavailable;
    };
    let mut state = parent.lock();
    let mut raised = None;
    for (id, interrupt) in &mut state.interrupts {
        if id
            .strip_prefix(prefix)
            .is_some_and(|child_id| pending.contains_key(child_id))
        {
            interrupt.clear_response();
            raised.get_or_insert_with(|| interrupt.clone());
        }
    }
    raised.map_or(Restored::Unavailable, |interrupt| {
        Restored::Interrupt(Box::new(interrupt))
    })
}

pub(crate) fn stored_turn(parent: &InterruptState, tool_use_id: &str) -> Option<JsonValue> {
    parent
        .lock()
        .context
        .get(INTERRUPTED_TURNS_KEY)?
        .get(tool_use_id)
        .cloned()
}

/// Keeps the child's paused turn in the parent's interrupt context, so it survives a parent
/// snapshot (TS `turns[toolUseId] = takeSnapshot(child)`).
pub(crate) fn store_turn(parent: &InterruptState, tool_use_id: &str, agent: &Agent) {
    let Ok(turn) = agent
        .take_snapshot(&TakeSnapshotOptions::session())
        .and_then(|snapshot| serde_json::to_value(snapshot).map_err(Error::other))
    else {
        return;
    };
    let mut state = parent.lock();
    let turns = state
        .context
        .entry(INTERRUPTED_TURNS_KEY)
        .or_insert_with(|| json!({}));
    match turns.as_object_mut() {
        Some(turns) => {
            turns.insert(tool_use_id.to_owned(), turn);
        }
        None => *turns = json!({ tool_use_id: turn }),
    }
}

pub(crate) fn remove_turn(parent: &InterruptState, tool_use_id: &str) {
    if let Some(JsonValue::Object(turns)) = parent.lock().context.get_mut(INTERRUPTED_TURNS_KEY) {
        turns.shift_remove(tool_use_id);
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
    ///
    /// # Errors
    ///
    /// The agent has a session manager and `preserve_context` is `false`.
    pub fn as_tool(self, options: AgentAsToolOptions) -> crate::Result<AgentAsTool> {
        let tool = AgentAsTool::new(self, options);
        tool.check()?;
        Ok(tool)
    }
}

#[cfg(test)]
mod tests;
