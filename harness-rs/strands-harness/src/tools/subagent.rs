//! The `subagent` tool (TS `tools/subagent.ts`).
//!
//! Each axis (instructions, tools, MCP servers, model, context) takes an authority mode that
//! decides whether it becomes a model-facing parameter: [`Fixed`] and [`Inherit`] add none,
//! [`Open`] adds a free string and [`Choice`] an enum of the offered names. Named presets add an
//! `agent_type` enum. Children are built through the caller's [`AgentBuilder`].

mod context;
mod resolve;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use futures::StreamExt;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use strands::agent::ModelSource;
use strands::interrupt::{Interrupt, InterruptParams};
use strands::tools::{ToolStream, ToolStreamEvent, ToolStreamItem};
use strands::types::messages::{ToolResultBlock, ToolResultContent};
use strands::types::tools::{ToolResultStatus, ToolSpec};
pub use strands::vended_tools::subagent::{Choice, ChoiceOption};
use strands::{
    Agent, AgentResult, AgentStreamEvent, InterruptResponse, InvokeArgs, InvokeOptions, JsonValue, Message,
    StopReason, Tool, ToolContext, ToolOutcome,
};

use crate::defaults::DEFAULT_SUBAGENT_MAX_DEPTH;
use crate::error::{HarnessError, Result};
use resolve::Axes;

/// The developer pins the value; the axis contributes no model-facing parameter.
#[expect(clippy::exhaustive_structs, reason = "TS `Fixed(value)`: a one-field wrapper")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fixed<T>(pub T);

/// The child takes the parent's value; the axis contributes no model-facing parameter.
#[expect(clippy::exhaustive_structs, reason = "TS `Inherit()`: a marker")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inherit;

/// The model writes the value freely; the axis contributes a string parameter.
#[expect(clippy::exhaustive_structs, reason = "TS `Open()`: a marker")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Open;

/// How much of the parent conversation the child sees.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextMode {
    /// A fresh start.
    #[default]
    None,
    /// The parent's history as real turns, tool calls and results included.
    All,
    /// The parent's text turns only, rendered as a block before the task.
    NoTools,
}

impl ContextMode {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
            Self::NoTools => "no_tools",
        }
    }
}

/// The context modes, in TS order.
pub(crate) const CONTEXT_MODES: [ContextMode; 3] =
    [ContextMode::None, ContextMode::All, ContextMode::NoTools];

/// An axis mode for the `instructions` parameter.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum InstructionsMode {
    /// The model writes the system prompt.
    #[default]
    Open,
    /// The model picks one of the offered prompts by name.
    Choice(Choice<String>),
    /// The developer's prompt, or none.
    Fixed(Option<String>),
}

/// An axis mode for the `tools` parameter.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum ToolsMode {
    /// The parent's tools.
    #[default]
    Inherit,
    /// The model picks a subset of the offered tools by name.
    Choice(Choice<String>),
    /// The developer's tool names; `None` inherits the parent's.
    Fixed(Option<Vec<String>>),
}

/// An axis mode for the `mcp_servers` parameter.
pub type McpServersMode = ToolsMode;

/// An axis mode for the `model` parameter.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum ModelMode {
    /// The parent's model.
    #[default]
    Inherit,
    /// The model picks one of the offered models by name.
    Choice(Choice<ModelSource>),
    /// The developer's model; `None` inherits the parent's.
    Fixed(Option<ModelSource>),
}

/// An axis mode for the `context` parameter.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ContextAxis {
    /// The developer's context mode.
    Fixed(ContextMode),
    /// The model picks one of the offered modes by name.
    Choice(Choice<ContextMode>),
}

impl Default for ContextAxis {
    fn default() -> Self {
        Self::Fixed(ContextMode::None)
    }
}

macro_rules! axis_from {
    ($axis:ident: $($from:ty => $to:expr),+ $(,)?) => {
        $(impl From<$from> for $axis {
            fn from(mode: $from) -> Self {
                let convert: fn($from) -> Self = $to;
                convert(mode)
            }
        })+
    };
}

axis_from!(InstructionsMode: Open => |_| Self::Open, Choice<String> => Self::Choice, Fixed<Option<String>> => |fixed| Self::Fixed(fixed.0));
axis_from!(ToolsMode: Inherit => |_| Self::Inherit, Choice<String> => Self::Choice, Fixed<Option<Vec<String>>> => |fixed| Self::Fixed(fixed.0));
axis_from!(ModelMode: Inherit => |_| Self::Inherit, Choice<ModelSource> => Self::Choice, Fixed<Option<ModelSource>> => |fixed| Self::Fixed(fixed.0));
axis_from!(ContextAxis: Choice<ContextMode> => Self::Choice, Fixed<ContextMode> => |fixed| Self::Fixed(fixed.0));

/// A named role: a partially applied child configuration selected through `agent_type`.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct Preset {
    /// The child's system prompt.
    pub instructions: Option<String>,
    /// The child's tool names; `None` inherits the parent's tools.
    pub tools: Option<Vec<String>>,
    /// The child's model; `None` inherits the parent's.
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// How much of the parent conversation the child sees.
    #[builder(default)]
    pub context: ContextMode,
    /// Caps the shared context to the last N messages; `None` shares all of it.
    pub last_messages: Option<i64>,
    /// What the role is for, shown to the model.
    #[builder(default)]
    pub description: String,
}

/// The options of a [`Preset`] (TS `PresetOptions`): the preset itself, built with
/// [`Preset::builder`].
pub type PresetOptions = Preset;

/// The system prompt of the [`GENERALIST`] preset.
pub(crate) const GENERALIST_INSTRUCTIONS: &str = "You are a general-purpose subagent handling a focused subtask on behalf of a parent agent. You run in your own fresh conversation and cannot ask follow-up questions, so work from the task as given, make reasonable assumptions where it is underspecified, and see it through to a verified result. Return a self-contained answer: state what you did, what you found, and anything the parent needs to act on. Your final message is the only thing that returns to the parent, so put the substance there rather than in intermediate steps.";

/// A general-purpose role for a focused subtask.
pub(crate) static GENERALIST: LazyLock<Preset> = LazyLock::new(|| Preset {
    instructions: Some(GENERALIST_INSTRUCTIONS.to_owned()),
    description: "a general-purpose agent for a focused subtask that runs in its own context".to_owned(),
    ..Preset::default()
});

/// The resolved child configuration handed to the builder: the config and the model's arguments.
#[expect(
    clippy::exhaustive_structs,
    reason = "data struct the builder reads field by field"
)]
#[derive(Debug, Clone)]
pub struct AgentSpec {
    /// The task the child runs.
    pub task: String,
    /// The preset the spec was resolved from.
    pub agent_type: Option<String>,
    /// The child's system prompt.
    pub instructions: Option<String>,
    /// The child's tool names; `None` inherits the parent's tools.
    pub tools: Option<Vec<String>>,
    /// The child's MCP server names; `None` inherits all the parent's servers.
    pub mcp_servers: Option<Vec<String>>,
    /// The child's model; `None` inherits the parent's.
    pub model: Option<ModelSource>,
    /// How much of the parent conversation the child sees.
    pub context: ContextMode,
    /// Caps the shared context to the last N messages; `None` shares all of it.
    pub last_messages: Option<i64>,
}

impl AgentSpec {
    /// A spec for `task` that inherits everything and shares no context.
    pub fn new(task: impl Into<String>, agent_type: Option<String>) -> Self {
        Self {
            task: task.into(),
            agent_type,
            instructions: None,
            tools: None,
            mcp_servers: None,
            model: None,
            context: ContextMode::None,
            last_messages: None,
        }
    }
}

/// Turns a resolved spec into a child agent, built the way the parent was.
pub type AgentBuilder = Arc<dyn Fn(AgentSpec) -> BoxFuture<'static, Result<Agent>> + Send + Sync>;

/// Options for [`make_subagent`].
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct MakeSubagentOptions {
    /// Turns a resolved spec into a child agent, built the way the parent was.
    pub builder: AgentBuilder,
    /// Named roles selectable through `agent_type`, in the order the model sees them.
    #[builder(default)]
    pub presets: IndexMap<String, Preset>,
    /// The preset used when the model omits `agent_type`. Defaults to the first preset.
    pub default_preset: Option<String>,
    /// Authority mode for `instructions`. Defaults to [`Open`].
    #[builder(into)]
    pub instructions: Option<InstructionsMode>,
    /// Authority mode for `tools`. Defaults to a multiple [`Choice`] over `inherited_tools`.
    #[builder(into)]
    pub tools: Option<ToolsMode>,
    /// Authority mode for `mcp_servers`. Defaults to a multiple [`Choice`] over
    /// `inherited_mcp_servers`, or [`Inherit`] when there are none.
    #[builder(into)]
    pub mcp_servers: Option<McpServersMode>,
    /// Authority mode for `model`. Defaults to [`Inherit`].
    #[builder(into)]
    pub model: Option<ModelMode>,
    /// Authority mode for `context`. Defaults to `Fixed(ContextMode::None)`.
    #[builder(into)]
    pub context: Option<ContextAxis>,
    /// The parent's tool names, used to build the default `tools` choice.
    #[builder(default)]
    pub inherited_tools: Vec<String>,
    /// The parent's MCP server names, used to build the default `mcp_servers` choice.
    #[builder(default)]
    pub inherited_mcp_servers: Vec<String>,
    /// Bounds delegation depth; the tool refuses at 0. Defaults to 2.
    pub max_depth: Option<u32>,
    /// The tool name. Defaults to `subagent`.
    pub name: Option<String>,
}

impl fmt::Debug for MakeSubagentOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MakeSubagentOptions")
            .field("presets", &self.presets)
            .field("default_preset", &self.default_preset)
            .field("instructions", &self.instructions)
            .field("tools", &self.tools)
            .field("mcp_servers", &self.mcp_servers)
            .field("model", &self.model)
            .field("context", &self.context)
            .field("max_depth", &self.max_depth)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

// Depth travels on each child's own state, not a build-time parameter.
const DEPTH_STATE_KEY: &str = "subagentDepth";

const BASE_DESCRIPTION: &str = "Delegate a self-contained task to a subagent that runs in its own context and returns a final report. Reach for this when a subtask would otherwise flood your context with intermediate work and you only need its conclusion. Do not poll for progress or redo its work.";

/// Builds a `subagent` tool whose schema follows the axis modes and the preset map.
///
/// `max_depth` bounds delegation depth: each child's remaining budget is stored on its own app
/// state right after it is built and read back the next time the tool runs on that child. At 0
/// the tool refuses with an error result instead of building another child.
///
/// ```
/// use std::sync::Arc;
///
/// use strands::Tool;
/// use strands::testing::{MockMessageModel, test_agent};
/// use strands_harness::{AgentBuilder, MakeSubagentOptions, Preset, make_subagent};
///
/// let builder: AgentBuilder =
///     Arc::new(|_spec| Box::pin(async { Ok(test_agent(Arc::new(MockMessageModel::new()))) }));
/// let researcher = Preset {
///     description: "a read-only agent that gathers facts for a focused question".to_owned(),
///     instructions: Some("You research one question and report what you found.".to_owned()),
///     ..Preset::default()
/// };
/// let options = MakeSubagentOptions::builder()
///     .builder(builder)
///     .presets([("researcher".to_owned(), researcher)].into())
///     .build();
/// let tool = make_subagent(options)?;
/// assert_eq!(tool.name(), "subagent");
/// # Ok::<(), strands_harness::HarnessError>(())
/// ```
///
/// # Errors
///
/// Returns an error when a `tools` or `mcp_servers` [`Choice`] is empty or not multiple.
pub fn make_subagent(options: MakeSubagentOptions) -> Result<Arc<dyn Tool>> {
    Ok(Arc::new(SubagentTool::new(options)?))
}

impl SubagentTool {
    fn new(options: MakeSubagentOptions) -> Result<Self> {
        let default_preset = options
            .default_preset
            .or_else(|| options.presets.keys().next().cloned());
        let tools = match options.tools {
            None => ToolsMode::Choice(Choice::multiple(options.inherited_tools)),
            Some(ToolsMode::Choice(choice)) if choice.options.is_empty() => {
                return Err(HarnessError::SubagentToolsChoiceEmpty);
            }
            Some(ToolsMode::Choice(choice)) if !choice.multiple => {
                return Err(HarnessError::SubagentToolsChoiceSingle);
            }
            Some(tools) => tools,
        };
        let mcp_servers = match options.mcp_servers {
            None if options.inherited_mcp_servers.is_empty() => McpServersMode::Inherit,
            None => McpServersMode::Choice(Choice::multiple(options.inherited_mcp_servers)),
            Some(McpServersMode::Choice(choice)) if choice.options.is_empty() => {
                return Err(HarnessError::SubagentMcpServersChoiceEmpty);
            }
            Some(McpServersMode::Choice(choice)) if !choice.multiple => {
                return Err(HarnessError::SubagentMcpServersChoiceSingle);
            }
            Some(mcp_servers) => mcp_servers,
        };
        let axes = Axes {
            presets: options.presets,
            default_preset,
            instructions: options.instructions.unwrap_or_default(),
            tools,
            mcp_servers,
            model: options.model.unwrap_or_default(),
            context: options.context.unwrap_or_default(),
        };
        Ok(SubagentTool {
            name: options.name.unwrap_or_else(|| "subagent".to_owned()),
            description: axes.describe_roles(BASE_DESCRIPTION),
            input_schema: axes.schema(),
            shared: Arc::new(Shared {
                axes,
                builder: options.builder,
                max_depth: options.max_depth.unwrap_or(DEFAULT_SUBAGENT_MAX_DEPTH),
                pending: Mutex::new(HashMap::new()),
            }),
        })
    }
}

/// A child that stopped on interrupts, kept until the parent's tool call is re-run with answers.
struct Pending {
    child: Agent,
    interrupts: Vec<Interrupt>,
}

struct Shared {
    axes: Axes,
    builder: AgentBuilder,
    max_depth: u32,
    // Children awaiting a resume, keyed by the tool-use id that interrupted.
    pending: Mutex<HashMap<String, Pending>>,
}

impl Shared {
    fn pending(&self) -> MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Resolves the spec, builds the child and stores its depth budget.
    async fn start(&self, input: &JsonValue, parent: &[Message], depth: f64) -> Result<(Agent, InvokeArgs)> {
        let spec = self.axes.resolve(input)?;
        let prompt = context::child_prompt(&spec, parent);
        let child = (self.builder)(spec).await?;
        child
            .app_state()
            .set(DEPTH_STATE_KEY, resolve::json_number(depth - 1.0))?;
        Ok((child, prompt))
    }
}

/// Streams a fresh child per call and passes its interrupts to the parent for resume.
struct SubagentTool {
    name: String,
    description: String,
    input_schema: JsonValue,
    shared: Arc<Shared>,
}

impl Tool for SubagentTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(self.name.as_str(), self.description.as_str())
            .with_input_schema(self.input_schema.clone())
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let shared = Arc::clone(&self.shared);
        ToolStream::new(async_stream::stream! {
            let tool_use_id = context.tool_use().tool_use_id().to_owned();
            let retained = shared.pending().remove(&tool_use_id);
            let (mut child, args) = match retained {
                // Resume: feed the parent's answers to the retained child.
                Some(Pending { child, interrupts }) => match answers(&context, &interrupts) {
                    Ok(responses) => (child, InvokeArgs::from(responses)),
                    Err(unanswered) => {
                        shared.pending().insert(tool_use_id, Pending { child, interrupts });
                        yield ToolOutcome::Interrupt(unanswered).into();
                        return;
                    }
                },
                None => {
                    let stored = context.agent().app_state().get_value(DEPTH_STATE_KEY);
                    let depth = resolve::remaining_depth(stored.as_ref(), shared.max_depth);
                    if depth <= 0.0 {
                        let text = format!(
                            "Delegation depth limit reached ({} levels); you cannot delegate further. Complete this task yourself instead of calling subagent again.",
                            shared.max_depth
                        );
                        yield ToolOutcome::Result(error_result(&tool_use_id, text)).into();
                        return;
                    }
                    let parent = Arc::clone(context.agent().messages());
                    match shared.start(context.tool_use().input(), &parent, depth).await {
                        Ok(started) => started,
                        Err(error) => {
                            yield ToolOutcome::Result(error_result(&tool_use_id, format!("Subagent error: {error}"))).into();
                            return;
                        }
                    }
                }
            };

            // The child joins the parent's abort (not its cancel message or deferral) and shares its
            // invocation state. As in TS it runs as a request of its own.
            let options = InvokeOptions {
                invocation_state: context.invocation_state().clone(),
                cancel_signal: Some(context.cancel_signal().abort_only()),
                ..InvokeOptions::default()
            };
            let mut result = None;
            let mut failure = None;
            {
                let mut events = child.stream_with(args, options);
                while let Some(item) = events.next().await {
                    match item {
                        Ok(event) => {
                            if let AgentStreamEvent::AgentResult(last) = &event {
                                result = Some(last.clone());
                            }
                            let data = serde_json::to_value(&event).unwrap_or_default();
                            yield ToolStreamItem::Event(ToolStreamEvent::new(data));
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            if let Some(error) = failure {
                yield ToolOutcome::Result(error_result(&tool_use_id, format!("Subagent error: {error}"))).into();
                return;
            }
            let Some(result) = result else {
                yield ToolOutcome::Result(error_result(&tool_use_id, "Subagent produced no result.".to_owned())).into();
                return;
            };
            if result.stop_reason() == StopReason::Interrupt && !result.interrupts().is_empty() {
                // Keep the child for the resume and raise its first unanswered interrupt on the parent.
                let interrupts = result.interrupts().to_vec();
                if let Err(unanswered) = answers(&context, &interrupts) {
                    shared.pending().insert(tool_use_id, Pending { child, interrupts });
                    yield ToolOutcome::Interrupt(unanswered).into();
                    return;
                }
            }
            yield ToolOutcome::Result(final_tool_result(&tool_use_id, &result)).into();
        })
    }
}

/// Re-raises each child interrupt on the parent under the child's interrupt id; the parent's
/// stored responses, or the first interrupt that has none.
#[expect(
    clippy::result_large_err,
    reason = "the interrupt is the value the tool ends with"
)]
fn answers(context: &ToolContext, interrupts: &[Interrupt]) -> Result<Vec<InterruptResponse>, Interrupt> {
    interrupts
        .iter()
        .map(|interrupt| {
            let response = context.interrupt(InterruptParams {
                name: interrupt.id().to_owned(),
                reason: interrupt.reason().cloned(),
                response: None,
            })?;
            Ok(InterruptResponse::new(interrupt.id(), response))
        })
        .collect()
}

fn error_result(tool_use_id: &str, message: String) -> ToolResultBlock {
    ToolResultBlock::new(
        tool_use_id,
        ToolResultStatus::Error,
        vec![ToolResultContent::from(message)],
    )
}

/// Maps the child's stop reason onto the parent's tool result. A cancelled, filtered or refused
/// child never produced an answer, so the parent sees a failure rather than its partial text.
fn final_tool_result(tool_use_id: &str, result: &AgentResult) -> ToolResultBlock {
    let stop_reason = result.stop_reason();
    let text = result.to_string();
    if !matches!(
        stop_reason,
        StopReason::Cancelled | StopReason::ContentFiltered | StopReason::Refusal
    ) {
        return ToolResultBlock::new(
            tool_use_id,
            ToolResultStatus::Success,
            vec![ToolResultContent::from(text)],
        );
    }
    let mut message = format!("Subagent stopped before finishing (stopReason={stop_reason}).");
    let partial = text.trim_end_matches('\n');
    if !partial.is_empty() {
        message.push_str("\n\nPartial output:\n");
        message.push_str(partial);
    }
    error_result(tool_use_id, message)
}
