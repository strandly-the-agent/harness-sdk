//! Agent-level test helpers: a hook recorder, a test agent and a tool context (TS
//! `__fixtures__/agent-helpers.ts` and `__fixtures__/tool-helpers.ts`).

use std::sync::{Arc, Mutex, PoisonError};

use crate::Agent;
use crate::agent::invocation_state::InvocationState;
use crate::agent::tool_agent::{ToolAgent, ToolAgentParts};
use crate::agent::{AgentBuilder, agent_builder};
use crate::cancel::CancelSignal;
use crate::hooks::events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, ContentBlockEvent,
    InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent, ModelStreamUpdateEvent,
    ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::hooks::registry::HookRegistry;
use crate::models::model::{Model, ModelSource};
use crate::state_store::StateStore;
use crate::testing::MockModel;
use crate::tools::Tool;
use crate::tools::context::ToolContext;
use crate::tools::executors::executor::PipelineToolCaller;
use crate::tools::registry::ToolRegistry;
use crate::types::tools::ToolUse;

/// Records the order hook events fire in, by their TS `type` names (for example
/// `beforeInvocationEvent`).
///
/// ```
/// use strands::ContentBlock;
/// use strands::testing::{HookRecorder, MockModel, test_agent};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), strands::Error> {
/// let model = MockModel::new();
/// model.add_turn(vec![ContentBlock::from("Hi")]);
/// let mut agent = test_agent(model).build()?;
/// let recorder = HookRecorder::new();
/// recorder.attach(agent.hooks());
/// agent.invoke("Hello").await?;
/// assert_eq!(recorder.events().first().map(String::as_str), Some("initializedEvent"));
/// assert_eq!(recorder.events().last().map(String::as_str), Some("agentResultEvent"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct HookRecorder {
    events: Arc<Mutex<Vec<String>>>,
}

macro_rules! record_events {
    ($recorder:ident, $hooks:ident, { $($event:ty => $name:literal),* $(,)? }) => {
        $(
            let events = Arc::clone(&$recorder.events);
            $hooks.add_hook(move |_: &mut $event| {
                events.lock().unwrap_or_else(PoisonError::into_inner).push($name.to_owned());
            });
        )*
    };
}

impl HookRecorder {
    /// An empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a recording hook for each of the 17 events.
    pub fn attach(&self, hooks: &HookRegistry) {
        record_events!(self, hooks, {
            InitializedEvent<'_> => "initializedEvent",
            BeforeInvocationEvent<'_> => "beforeInvocationEvent",
            AfterInvocationEvent<'_> => "afterInvocationEvent",
            MessageAddedEvent<'_> => "messageAddedEvent",
            BeforeModelCallEvent<'_> => "beforeModelCallEvent",
            AfterModelCallEvent<'_> => "afterModelCallEvent",
            ModelStreamUpdateEvent<'_> => "modelStreamUpdateEvent",
            ContentBlockEvent<'_> => "contentBlockEvent",
            ModelMessageEvent<'_> => "modelMessageEvent",
            BeforeToolsEvent<'_> => "beforeToolsEvent",
            AfterToolsEvent<'_> => "afterToolsEvent",
            BeforeToolCallEvent => "beforeToolCallEvent",
            AfterToolCallEvent => "afterToolCallEvent",
            ToolStreamUpdateEvent => "toolStreamUpdateEvent",
            ToolResultEvent => "toolResultEvent",
            InterruptEvent<'_> => "interruptEvent",
            AgentResultEvent<'_> => "agentResultEvent",
        });
    }

    /// The events fired so far, in order.
    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// An agent builder for tests: the given model and no printer.
///
/// ```
/// use strands::testing::{MockModel, test_agent};
///
/// let agent = test_agent(MockModel::new()).name("tester").build()?;
/// assert_eq!(agent.name(), "tester");
/// # Ok::<(), strands::Error>(())
/// ```
pub fn test_agent(
    model: impl Into<ModelSource>,
) -> AgentBuilder<agent_builder::SetPrinter<agent_builder::SetModel>> {
    Agent::builder().model(model).printer(false)
}

/// Builds a [`ToolContext`] for unit-testing a tool outside an agent (TS `createMockContext`).
///
/// The context's agent has id `agent`, a [`MockModel`], fresh app state and invocation state,
/// and a registry with the given tools; its
/// [`call_tool`](crate::agent::tool_agent::ToolAgent::call_tool) runs those tools through the
/// per-tool pipeline without hooks. Tools with an invalid name or description are left out.
///
/// ```
/// use strands::testing::tool_context;
/// use strands::types::tools::ToolUse;
///
/// let context = tool_context(ToolUse::new("echo", "tool-1", serde_json::json!({}))).build();
/// context.agent().app_state().set("region", "eu")?;
/// assert_eq!(context.tool_use().tool_use_id, "tool-1");
/// # Ok::<(), strands::Error>(())
/// ```
#[bon::builder(finish_fn = build)]
pub fn tool_context(
    /// The call the tool receives.
    #[builder(start_fn)]
    tool_use: ToolUse,
    /// The agent's app state.
    #[builder(default)]
    app_state: StateStore,
    /// The invocation state.
    #[builder(default)]
    invocation_state: InvocationState,
    /// The cancel signal; a signal that never fires when unset.
    cancel_signal: Option<CancelSignal>,
    /// The tools in the agent's registry.
    #[builder(default)]
    tools: Vec<Arc<dyn Tool>>,
    /// The agent's model; a [`MockModel`] when unset.
    model: Option<Arc<dyn Model>>,
) -> ToolContext {
    let cancel_signal = cancel_signal.unwrap_or_else(|| CancelSignal::new().0);
    let model = model.unwrap_or_else(|| Arc::new(MockModel::new()));
    let mut registry = ToolRegistry::new();
    for tool in tools {
        let name = tool.name().to_owned();
        if let Err(error) = registry.add(tool) {
            tracing::warn!("tool_name=<{name}>, error=<{error}> | tool left out of the test context");
        }
    }
    let hooks = Arc::new(HookRegistry::new());
    let agent = ToolAgent::new(ToolAgentParts {
        id: crate::agent::DEFAULT_AGENT_ID.to_owned(),
        name: crate::agent::DEFAULT_AGENT_NAME.to_owned(),
        app_state,
        aux_model: Arc::clone(&model),
        model,
        cancel_handle: cancel_signal.handle(),
        messages: Arc::from(Vec::new()),
        tool_registry: Arc::new(registry),
        invocation_context: None,
        tool_caller: PipelineToolCaller::new(hooks, invocation_state.clone(), cancel_signal.clone()),
    });
    ToolContext::new(tool_use, agent, invocation_state, cancel_signal)
}
