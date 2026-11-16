//! The subagent tool (TS `vended-tools/subagent/`): delegates a self-contained task to a fresh
//! child agent and returns its final answer.
//!
//! The tool's schema comes from axis policies (see [`InstructionsAxis`], [`ToolsAxis`],
//! [`ModelAxis`], [`ContextAxis`]) and named [`Preset`]s. Each call resolves the model's input
//! into an [`AgentSpec`], builds the child with an [`AgentBuilder`] (by default one inheriting
//! from the calling agent), streams it, and propagates its interrupts to the parent for resume.

mod spec;

pub use spec::{
    AgentBuilder, AgentSpec, Choice, ChoiceOption, ContextAxis, InstructionsAxis, ModelAxis, Preset,
    PresetBuilder, ToolsAxis, preset_builder,
};

use std::fmt;
use std::sync::{Arc, LazyLock};

use futures::StreamExt;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::json;

use crate::JsonValue;
use crate::agent::Agent;
use crate::agent::agent_as_tool::{
    Restored, Resume, encode_uri_component, register, remove_turn, reraise, restore, resume_input,
    store_turn, stored_turn, unfinished,
};
use crate::error::{Error, Result};
use crate::interrupt::InterruptResponse;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::types::agent::{AgentStreamEvent, InvokeArgs, InvokeOptions, ToolAgent};
use crate::types::json::JsonObject;
use crate::types::messages::{ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock};
use crate::types::tools::{ToolResultStatus, ToolSpec};

use spec::{Axes, default_build, resolve, schema_properties, with_option_lines};

/// The default description (TS `DEFAULT_SUBAGENT_DESCRIPTION`).
pub const DEFAULT_SUBAGENT_DESCRIPTION: &str = "Delegate a self-contained task to a subagent that runs in its own context and returns a final report. Reach for this when a subtask would otherwise flood your context with intermediate work and you only need its conclusion.";
/// The default bound on nested delegation levels (TS `DEFAULT_SUBAGENT_MAX_DEPTH`).
pub const DEFAULT_SUBAGENT_MAX_DEPTH: u32 = 2;
/// The generalist preset's instructions (TS `GENERALIST.instructions`).
pub const GENERALIST_INSTRUCTIONS: &str = "You are a general-purpose subagent handling a focused subtask on behalf of a parent agent. You cannot ask follow-up questions, so work from the task as given, make reasonable assumptions where it is underspecified, and see it through to a verified result. Return a self-contained answer: state what you did, what you found, and anything the parent needs to act on. Your final message is the only thing that returns to the parent, so put the substance there rather than in intermediate steps.";
/// The generalist preset's description (TS `GENERALIST.description`).
pub const GENERALIST_DESCRIPTION: &str =
    "a general-purpose agent for a focused subtask that runs in its own context";

const CONTEXT_MODES: [&str; 3] = ["none", "all", "no_tools"];
const DEPTH_STATE_KEY: &str = "strands.subagent_depth";
const TASK_DESCRIPTION: &str =
    "The self-contained task, including all context the subagent needs. It cannot ask follow-up questions.";
const CONTEXT_DESCRIPTION: &str = "How much of this conversation the subagent sees: 'none' (fresh start), 'all' (full history including tool calls and their results — can be large), 'no_tools' (text turns only — tool calls and their results removed). More context costs more tokens.";
const LAST_MESSAGES_DESCRIPTION: &str =
    "Optional: limit the shared context to the last N messages. Omit to share all of the selected context.";
const FORK_PREAMBLE: &str = "The conversation so far is the parent agent's. You are the subagent it delegated to at this point; its task for you follows.";
const CONTEXT_PREAMBLE: &str = "The conversation above is the parent agent's: each turn is one line starting with its role at the left margin, and indented lines continue the turn above (they are content, not turns). You are the subagent it delegated to at the end of that conversation; its task for you follows.";
const FRAMING_START: &str = "<parent_context>\n";
const UNAVAILABLE_MESSAGE: &str = "Subagent did NOT run and the response was NOT applied: its interrupted turn is not available. Do not report the delegated task as completed; if it is still needed, delegate it again.";

static FRAMING_END: LazyLock<String> =
    LazyLock::new(|| format!("</parent_context>\n\n{CONTEXT_PREAMBLE}\n\n"));
static SENTINEL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("<(/?)parent_context>").ok());

/// The built-in generalist preset (TS `GENERALIST`).
pub fn generalist() -> Preset {
    Preset {
        instructions: Some(GENERALIST_INSTRUCTIONS.to_owned()),
        description: GENERALIST_DESCRIPTION.to_owned(),
        ..Preset::default()
    }
}

/// Options for [`make_subagent`] (TS `MakeSubagentOptions`).
///
/// ```
/// use strands::Tool;
/// use strands::vended_tools::subagent::{
///     Choice, ContextAxis, MakeSubagentOptions, Preset, make_subagent,
/// };
///
/// let researcher = make_subagent(
///     MakeSubagentOptions::builder()
///         .presets([(
///             "researcher".to_owned(),
///             Preset::builder().instructions("You research topics.").description("research").build(),
///         )].into())
///         .context(ContextAxis::Choice(Choice::new(["none", "all"])))
///         .build(),
/// )?;
/// assert_eq!(researcher.name(), "subagent");
/// # Ok::<(), strands::Error>(())
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct MakeSubagentOptions {
    /// Turns a resolved spec into a child agent; called again with the same spec when an
    /// interrupted child resumes. Defaults to a builder inheriting the calling agent's model,
    /// tools and sandbox.
    pub builder: Option<AgentBuilder>,
    /// Named roles the model selects with `agent_type`, in order (default `generalist`).
    pub presets: Option<IndexMap<String, Preset>>,
    /// The preset used when the model omits `agent_type` (default the first preset).
    pub default_preset: Option<String>,
    /// Policy for the child's system prompt (default [`InstructionsAxis::Open`]).
    #[builder(default)]
    pub instructions: InstructionsAxis,
    /// Policy for the child's tools (default [`ToolsAxis::Inherit`]).
    #[builder(default)]
    pub tools: ToolsAxis,
    /// Policy for the child's model (default [`ModelAxis::Inherit`]).
    #[builder(default)]
    pub model: ModelAxis,
    /// Policy for how much parent conversation the child sees (default `Fixed("none")`).
    #[builder(default)]
    pub context: ContextAxis,
    /// The bound on nested delegation levels, at least 1 (default
    /// [`DEFAULT_SUBAGENT_MAX_DEPTH`]).
    pub max_depth: Option<u32>,
    /// The tool name (default `subagent`).
    pub name: Option<String>,
    /// The description (default [`DEFAULT_SUBAGENT_DESCRIPTION`]).
    pub description: Option<String>,
}

impl fmt::Debug for MakeSubagentOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MakeSubagentOptions")
            .field("builder", &self.builder.as_ref().map(|_| "<builder>"))
            .field("presets", &self.presets)
            .field("default_preset", &self.default_preset)
            .field("instructions", &self.instructions)
            .field("tools", &self.tools)
            .field("model", &self.model)
            .field("context", &self.context)
            .field("max_depth", &self.max_depth)
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}

/// The subagent tool (TS `SubagentTool`): each call runs a freshly built child; an interrupted
/// child's turn is stored on the parent and resumes once the parent gets the responses.
pub struct SubagentTool {
    spec: ToolSpec,
    inner: Arc<Inner>,
}

struct Inner {
    name: String,
    axes: Axes,
    context: ContextAxis,
    builder: Option<AgentBuilder>,
    max_depth: u32,
}

impl fmt::Debug for SubagentTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubagentTool")
            .field("name", &self.inner.name)
            .field("max_depth", &self.inner.max_depth)
            .finish_non_exhaustive()
    }
}

/// A subagent tool whose schema is derived from the axis policies and presets (TS
/// `makeSubagent`).
///
/// # Errors
///
/// `name` or `description` is empty, `max_depth` is zero, `default_preset` is not a preset, a
/// context mode is not `none`, `all` or `no_tools`, or a `Choice` axis has no options (or, for
/// tools, is not multiple).
pub fn make_subagent(options: MakeSubagentOptions) -> Result<SubagentTool> {
    let name = options.name.unwrap_or_else(|| "subagent".to_owned());
    let description = options
        .description
        .unwrap_or_else(|| DEFAULT_SUBAGENT_DESCRIPTION.to_owned());
    let max_depth = options.max_depth.unwrap_or(DEFAULT_SUBAGENT_MAX_DEPTH);
    if name.is_empty() {
        return Err(Error::other("name must be a non-empty string."));
    }
    if description.is_empty() {
        return Err(Error::other("description must be a non-empty string."));
    }
    if max_depth == 0 {
        return Err(Error::other("maxDepth must be a positive integer (>= 1)."));
    }
    let presets = options
        .presets
        .unwrap_or_else(|| IndexMap::from([("generalist".to_owned(), generalist())]));
    let default_preset = options.default_preset.or_else(|| presets.keys().next().cloned());
    if let Some(default) = &default_preset
        && !presets.contains_key(default)
    {
        let names: Vec<&str> = presets.keys().map(String::as_str).collect();
        return Err(Error::other(format!(
            "defaultPreset '{default}' is not one of the presets: {}.",
            names.join(", ")
        )));
    }
    for mode in context_modes(&options.context) {
        if !mode.is_some_and(|mode| CONTEXT_MODES.contains(&mode)) {
            let shown = mode.map_or_else(|| "undefined".to_owned(), |mode| json!(mode).to_string());
            return Err(Error::other(format!(
                "context mode {shown} must be one of: {}.",
                CONTEXT_MODES.join(", ")
            )));
        }
    }
    let axes = Axes {
        presets,
        default_preset,
        instructions: options.instructions,
        tools: options.tools,
        model: options.model,
    };
    let properties = schema_properties(&axes).map_err(Error::other)?;
    Ok(SubagentTool::new(
        name,
        &description,
        axes,
        options.context,
        options.builder,
        max_depth,
        properties,
    ))
}

/// The default subagent tool (TS `subagent`): the generalist preset and every axis at its
/// default.
pub fn subagent() -> SubagentTool {
    let axes = Axes {
        presets: IndexMap::from([("generalist".to_owned(), generalist())]),
        default_preset: Some("generalist".to_owned()),
        instructions: InstructionsAxis::Open,
        tools: ToolsAxis::Inherit,
        model: ModelAxis::Inherit,
    };
    let properties = schema_properties(&axes).unwrap_or_default();
    SubagentTool::new(
        "subagent".to_owned(),
        DEFAULT_SUBAGENT_DESCRIPTION,
        axes,
        ContextAxis::default(),
        None,
        DEFAULT_SUBAGENT_MAX_DEPTH,
        properties,
    )
}

/// The modes a context axis can produce (`None` for an unset `Fixed`).
fn context_modes(context: &ContextAxis) -> Vec<Option<&str>> {
    match context {
        ContextAxis::Fixed(mode) => vec![mode.as_deref()],
        ContextAxis::Choice(choice) => choice
            .options
            .iter()
            .map(|option| Some(option.value.as_deref().unwrap_or(&option.name)))
            .collect(),
    }
}

/// What a call runs, or the outcome it ends with before running a child.
enum Prepared {
    Run(Box<Agent>, InvokeArgs),
    Done(ToolOutcome),
}

impl SubagentTool {
    fn new(
        name: String,
        description: &str,
        axes: Axes,
        context: ContextAxis,
        builder: Option<AgentBuilder>,
        max_depth: u32,
        axis_properties: JsonObject,
    ) -> Self {
        let mut properties = JsonObject::new();
        properties.insert(
            "task".into(),
            json!({ "type": "string", "description": TASK_DESCRIPTION }),
        );
        properties.extend(axis_properties);
        if let ContextAxis::Choice(choice) = &context {
            let names: Vec<&str> = choice.options.iter().map(|option| option.name.as_str()).collect();
            properties.insert(
                "context".into(),
                json!({
                    "type": "string",
                    "enum": names,
                    "description": with_option_lines(CONTEXT_DESCRIPTION.to_owned(), choice),
                }),
            );
            if context_modes(&context)
                .into_iter()
                .any(|mode| mode != Some("none"))
            {
                properties.insert(
                    "last_messages".into(),
                    json!({ "type": "integer", "description": LAST_MESSAGES_DESCRIPTION }),
                );
            }
        }
        let spec = ToolSpec::new(name.as_str(), description).with_input_schema(json!({
            "type": "object",
            "properties": properties,
            "required": ["task"],
        }));
        Self {
            spec,
            inner: Arc::new(Inner {
                name,
                axes,
                context,
                builder,
                max_depth,
            }),
        }
    }
}

impl Tool for SubagentTool {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn description(&self) -> &str {
        self.spec.description()
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let inner = Arc::clone(&self.inner);
        ToolStream::new(async_stream::stream! {
            let tool_use_id = context.tool_use().tool_use_id().to_owned();
            let raw = context.tool_use().input().as_object().cloned().unwrap_or_default();
            let parent = context.agent().interrupt_state().clone();
            let prefix = format!("subagent:{}:", encode_uri_component(&tool_use_id));
            let prepared = match resume_input(&parent, &prefix) {
                Resume::Fresh => inner.prepare(&tool_use_id, &raw, context.agent()),
                Resume::Unanswered(interrupt) => Prepared::Done(ToolOutcome::Interrupt(interrupt)),
                Resume::With(responses) => inner.prepare_resume(&tool_use_id, &raw, context.agent(), &prefix, responses),
            };
            let (mut child, prompt) = match prepared {
                Prepared::Run(child, prompt) => (child, prompt),
                Prepared::Done(outcome) => {
                    yield outcome.into();
                    return;
                }
            };
            let options = InvokeOptions {
                invocation_state: context.invocation_state().copy(),
                invocation_context: context.invocation_context().cloned(),
                cancel_signal: Some(context.cancel_signal().abort_only()),
                ..InvokeOptions::default()
            };
            let child_cancel = child.cancel_handle();
            let mut result = None;
            let mut cancelled = false;
            let mut failure = None;
            {
                let mut events = child.stream_with(prompt, options);
                while let Some(item) = events.next().await {
                    match item {
                        Ok(AgentStreamEvent::ToolStreamUpdate(update)) => {
                            yield ToolStreamItem::Event(update.event().clone());
                        }
                        Ok(event) => {
                            // Read on the result event, before the child's cancel state resets.
                            if let AgentStreamEvent::AgentResult(last) = &event && result.is_none() {
                                result = Some(last.clone());
                                cancelled = child_cancel.is_cancelled();
                            }
                            yield ToolStreamItem::Event(ToolStreamEvent::wrapping(event, false));
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            if let Some(error) = failure {
                yield ToolOutcome::Result(inner.failed(&tool_use_id, &error.to_string())).into();
                return;
            }
            let Some(result) = result else {
                yield ToolOutcome::Result(error_block(&tool_use_id, "Subagent did not produce a result.")).into();
                return;
            };
            if result.stop_reason() == StopReason::Interrupt && !result.interrupts().is_empty() {
                store_turn(&parent, &tool_use_id, &child);
                if let Some(interrupt) = register(&parent, &prefix, result.interrupts()) {
                    yield ToolOutcome::Interrupt(interrupt).into();
                    return;
                }
            }
            remove_turn(&parent, &tool_use_id);
            let outcome = match unfinished("Subagent", &inner.name, &result, cancelled) {
                Some(text) => error_block(&tool_use_id, &text),
                None => ToolResultBlock::new(&tool_use_id, ToolResultStatus::Success, vec![result.to_string().into()]),
            };
            yield ToolOutcome::Result(outcome).into();
        })
    }
}

impl Inner {
    fn build(&self, parent: &ToolAgent, spec: &AgentSpec) -> Result<Agent> {
        match &self.builder {
            Some(builder) => builder(spec),
            None => default_build(parent, spec),
        }
    }

    /// An error result for a call that failed, logged (TS `Subagent error: <message>`).
    fn failed(&self, tool_use_id: &str, message: &str) -> ToolResultBlock {
        tracing::warn!(tool_name = %self.name, tool_use_id = %tool_use_id, error = %message, "subagent failed");
        error_block(tool_use_id, &format!("Subagent error: {message}"))
    }

    /// Resolves the child and its prompt for a fresh call (TS `_prepareChild`).
    fn prepare(&self, tool_use_id: &str, raw: &JsonObject, parent: &ToolAgent) -> Prepared {
        let done = |text: &str| Prepared::Done(ToolOutcome::Result(error_block(tool_use_id, text)));
        let depth = parent
            .app_state()
            .get_value(DEPTH_STATE_KEY)
            .and_then(|depth| depth.as_i64())
            .unwrap_or_else(|| i64::from(self.max_depth));
        if depth <= 0 {
            return done(&format!(
                "Delegation depth limit reached ({} levels); you cannot delegate further. Complete this task yourself instead of calling {} again.",
                self.max_depth, self.name
            ));
        }
        let Some(task) = raw
            .get("task")
            .and_then(JsonValue::as_str)
            .filter(|task| !task.trim().is_empty())
        else {
            return done("Missing required parameter 'task': describe the task to delegate.");
        };
        let resolved = match resolve(&self.name, raw, &self.axes) {
            Ok(resolved) => resolved,
            Err(message) => return Prepared::Done(ToolOutcome::Result(self.failed(tool_use_id, &message))),
        };
        if resolved
            .instructions
            .as_ref()
            .is_some_and(|value| !value.is_string())
        {
            return done("Parameter 'instructions' must be a string.");
        }
        let child = self
            .build(parent, &resolved.spec)
            .and_then(|child| child.app_state().set(DEPTH_STATE_KEY, depth - 1).map(|()| child));
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                return Prepared::Done(ToolOutcome::Result(self.failed(tool_use_id, &error.to_string())));
            }
        };
        let (mode, last_messages) = self.resolve_context(raw);
        let prompt = match mode.as_str() {
            "all" => fork_prompt(parent.messages(), last_messages, task),
            "no_tools" => {
                let block = render_context(parent.messages(), last_messages);
                if block.is_empty() {
                    InvokeArgs::from(task)
                } else {
                    InvokeArgs::from(format!("{FRAMING_START}{block}\n{}{task}", *FRAMING_END))
                }
            }
            _ => InvokeArgs::from(task),
        };
        Prepared::Run(Box::new(child), prompt)
    }

    /// Rebuilds the interrupted child from the same input and restores its stored turn (TS
    /// `_prepareResume`).
    fn prepare_resume(
        &self,
        tool_use_id: &str,
        raw: &JsonObject,
        parent: &ToolAgent,
        prefix: &str,
        responses: Vec<InterruptResponse>,
    ) -> Prepared {
        let state = parent.interrupt_state();
        let unavailable =
            || Prepared::Done(ToolOutcome::Result(error_block(tool_use_id, UNAVAILABLE_MESSAGE)));
        let resolved = match resolve(&self.name, raw, &self.axes) {
            Ok(resolved) => resolved,
            Err(message) => return Prepared::Done(ToolOutcome::Result(self.failed(tool_use_id, &message))),
        };
        let child = self.build(parent, &resolved.spec).inspect_err(|error| {
            tracing::warn!(
                tool_name = %self.name,
                tool_use_id = %tool_use_id,
                error = %error,
                "failed to rebuild interrupted subagent"
            );
        });
        let restored = match child {
            Ok(mut child) => match restore(&mut child, state, prefix, tool_use_id) {
                Restored::Ready => return Prepared::Run(Box::new(child), InvokeArgs::from(responses)),
                other => other,
            },
            Err(_) => reraise(
                state,
                prefix,
                tool_use_id,
                stored_turn(state, tool_use_id).as_ref(),
            ),
        };
        match restored {
            Restored::Interrupt(interrupt) => Prepared::Done(ToolOutcome::Interrupt(*interrupt)),
            _ => unavailable(),
        }
    }

    /// The context mode and the optional positive `last_messages` limit (TS `_resolveContext`).
    fn resolve_context(&self, raw: &JsonObject) -> (String, Option<usize>) {
        let mode = match &self.context {
            ContextAxis::Choice(choice) => raw
                .get("context")
                .filter(|value| !value.is_null())
                .map(js_string)
                .and_then(|name| choice.options.iter().find(|option| option.name == name))
                .map(|option| option.value.clone().unwrap_or_else(|| option.name.clone())),
            ContextAxis::Fixed(mode) => mode.clone(),
        };
        let last = raw.get("last_messages").and_then(|value| match value {
            JsonValue::Number(number) => number.as_f64(),
            JsonValue::String(text) => Some(js_number_of(text)),
            _ => None,
        });
        let last_messages = last
            .map(f64::trunc)
            .filter(|count| count.is_finite() && *count >= 1.0)
            .map(saturating_count);
        (mode.unwrap_or_else(|| "none".to_owned()), last_messages)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a finite whole count of at least 1; `as` saturates"
)]
fn saturating_count(count: f64) -> usize {
    count as usize
}

/// JS `String(value)` for the scalar values a model sends.
fn js_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// JS `Number(text)`: blank is 0, otherwise a decimal number or NaN.
fn js_number_of(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// The parent's messages for `all` mode with the framed task appended to the last user turn
/// (TS `_forkMessages`): reasoning, injected blocks and unanswered tool uses are dropped,
/// same-role neighbours merged, and `last_n` widened so a tool pair is never split.
fn fork_prompt(messages: &[Message], last_n: Option<usize>, task: &str) -> InvokeArgs {
    let answered: Vec<&str> = messages
        .iter()
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.tool_use_id()),
            _ => None,
        })
        .collect();
    let mut forked: Vec<(Role, Vec<ContentBlock>)> = Vec::new();
    for message in messages {
        let content: Vec<ContentBlock> = message
            .content()
            .iter()
            .filter(|block| match block {
                ContentBlock::Reasoning(_) | ContentBlock::Injected(_) => false,
                ContentBlock::ToolUse(tool_use) => answered.contains(&tool_use.tool_use_id()),
                _ => true,
            })
            .cloned()
            .collect();
        if content.is_empty() {
            continue;
        }
        match forked.last_mut() {
            Some((role, previous)) if *role == message.role() => previous.extend(content),
            _ => forked.push((message.role(), content)),
        }
    }
    if let Some(last_n) = last_n {
        let mut start = forked.len().saturating_sub(last_n);
        while start > 0 && !is_valid_start(forked.get(start..).unwrap_or_default()) {
            start -= 1;
        }
        forked.drain(..start);
    }
    let framed = ContentBlock::from(TextBlock::new(format!("{FORK_PREAMBLE}\n\n{task}")));
    match forked.last_mut() {
        None => return InvokeArgs::from(task),
        Some((Role::User, content)) => content.push(framed),
        Some(_) => forked.push((Role::User, vec![framed])),
    }
    InvokeArgs::from(
        forked
            .into_iter()
            .map(|(role, content)| Message::new(role, content))
            .collect::<Vec<_>>(),
    )
}

/// Whether the conversation can start at `window[0]` (TS `findValidTrimPoint(window, 0) === 0`).
fn is_valid_start(window: &[(Role, Vec<ContentBlock>)]) -> bool {
    let Some((role, content)) = window.first() else {
        return true;
    };
    let has_result = |blocks: &[ContentBlock]| {
        blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult(_)))
    };
    if *role != Role::User || has_result(content) {
        return false;
    }
    let has_use = content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)));
    !has_use || window.get(1).is_some_and(|(_, next)| has_result(next))
}

/// The parent's text turns as plain lines for `no_tools` mode (TS `_renderContext`).
fn render_context(messages: &[Message], last_n: Option<usize>) -> String {
    let messages = match last_n {
        Some(count) => messages
            .get(messages.len().saturating_sub(count)..)
            .unwrap_or(messages),
        None => messages,
    };
    let lines: Vec<String> = messages
        .iter()
        .filter_map(|message| {
            let joined = message
                .content()
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            let mut text = joined.trim();
            if text.starts_with(FRAMING_START)
                && let Some(end) = text.find(FRAMING_END.as_str())
            {
                text = text.get(end + FRAMING_END.len()..).unwrap_or_default();
            }
            (!text.is_empty()).then(|| format!("{}: {text}", message.role()).replace('\n', "\n  "))
        })
        .collect();
    let block = lines.join("\n");
    match SENTINEL.as_ref() {
        Some(sentinel) => sentinel
            .replace_all(&block, "<\\${1}parent_context>")
            .into_owned(),
        None => block,
    }
}

fn error_block(tool_use_id: &str, text: &str) -> ToolResultBlock {
    ToolResultBlock::new(tool_use_id, ToolResultStatus::Error, vec![text.into()])
}

#[cfg(test)]
mod tests;
