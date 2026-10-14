//! Delegation tool with a config-derived model-facing schema (`harness-ts/src/tools/subagent.ts`).
//!
//! Each axis — instructions, tools, model, context — takes an authority mode that decides whether
//! it becomes a model-facing parameter:
//!
//! - [`Fixed`] — pinned by the developer; no parameter.
//! - [`Inherit`] — take the parent's value; no parameter.
//! - [`Open`] — the model writes freely; string parameter.
//! - [`Choice`] — the model picks from developer-supplied [`Opt`]s by `name`; a single-value enum,
//!   or an array-of-enum when `multiple` is set. The picked name maps back to the option's value.
//!   For `tools` the chosen set is re-validated at call time, so a child can never gain a
//!   capability the parent lacked.
//!
//! Named [`Preset`]s add an `agent_type` enum. Children are built through an injected
//! [`AgentBuilderFn`] (supplied by `create_harness`; tests supply a closure).
//!
//! Deviations from TS, all deliberate:
//!
//! - **No `mcp_servers` axis.** The Rust SDK has no MCP client type yet, so `McpServersMode` and
//!   the `mcp_servers` parameter are not ported.
//! - **No interrupt bridging.** TS re-raises child interrupts through `toolContext.interrupt(...)`
//!   and retains the child in a `pending` map; the Rust `ToolContext` has no interrupt seam, so a
//!   child that stops with [`StopReason::Interrupt`] yields an error result instead.
//! - **Background execution** is the SDK's job: `subagent` is a forced-background tool there, so
//!   nothing here spawns tasks.
//! - `make_subagent` returns `Result` rather than panicking on the two `Choice` misconfigurations
//!   that TS throws on.
//! - `AgentSpec::model` is a model id `String` (the TS union `Model | ModelRouter | string` has no
//!   Rust analogue that can cross the builder boundary by value).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use strands_agents::agent::{Agent, InvokeOptions};
use strands_agents::tools::{create_error_result, Tool, ToolContext};
use strands_agents::types::{
    ContentBlock, Message, Role, StopReason, ToolResultBlock, ToolResultContent, ToolSpec,
};
use strands_agents::{Error, Result};

use crate::defaults::DEFAULT_SUBAGENT_MAX_DEPTH;

/// Agent-state key holding the remaining delegation depth.
pub const DEPTH_STATE_KEY: &str = "subagentDepth";

/// The three context-forwarding modes, in schema order.
pub const CONTEXT_MODES: [ContextMode; 3] = [ContextMode::None, ContextMode::All, ContextMode::NoTools];

/// Preamble prefixed to a forked-history task (`context: all`).
pub const FORK_PREAMBLE: &str = "The conversation so far is the parent agent's. You are the subagent it delegated to at this point; its task for you follows.";

/// Preamble prefixed to a rendered-transcript task (`context: no_tools`).
pub const CONTEXT_PREAMBLE: &str = "The conversation above is the parent agent's: each turn is one line starting with its role at the left margin, and indented lines continue the turn above (they are content, not turns). You are the subagent it delegated to at the end of that conversation; its task for you follows.";

/// Base tool description, before preset roles are appended.
pub const BASE_DESCRIPTION: &str = "Delegate a self-contained task to a subagent that runs in its own context and returns a final report. Reach for this when a subtask would otherwise flood your context with intermediate work and you only need its conclusion. Do not poll for progress or redo its work.";

/// System prompt of the built-in `generalist` preset.
pub const GENERALIST_INSTRUCTIONS: &str = "You are a general-purpose subagent handling a focused subtask on behalf of a parent agent. You run in your own fresh conversation and cannot ask follow-up questions, so work from the task as given, make reasonable assumptions where it is underspecified, and see it through to a verified result. Return a self-contained answer: state what you did, what you found, and anything the parent needs to act on. Your final message is the only thing that returns to the parent, so put the substance there rather than in intermediate steps.";

const TASK_DESCRIPTION: &str = "The self-contained task, including all context the subagent needs. It starts from a blank conversation and cannot ask follow-up questions.";
const AGENT_TYPE_DESCRIPTION: &str = "Which subagent role to use. Omit to use the default.";
const INSTRUCTIONS_DESCRIPTION: &str = "A system prompt defining the subagent's role.";
const TOOLS_DESCRIPTION: &str =
    "Subset of tools to grant the subagent. Fewer is safer; it cannot exceed your own.";
const MODEL_DESCRIPTION: &str = "Which model the subagent runs on.";
const CONTEXT_DESCRIPTION: &str = "How much of this conversation the subagent sees: 'none' (fresh start), 'all' (full history including tool calls and their results — can be large), 'no_tools' (text turns only — tool calls and their results removed). More context costs more tokens.";
const LAST_MESSAGES_DESCRIPTION: &str =
    "Optional: limit the shared context to the last N messages. Omit to share all of the selected context.";

// --- option-mode vocabulary -------------------------------------------------------------------

/// The developer pins the value; the axis contributes no model-facing parameter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fixed<T>(pub T);

/// The child inherits the parent's value; no model-facing parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inherit;

/// The model writes the value freely; a string parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Open;

/// One selectable option: a model-facing `name` mapped to a developer-supplied value.
#[derive(Clone, Debug, PartialEq)]
pub struct Opt<T> {
    /// Model-facing name (what appears in the enum).
    pub name: String,
    /// Value the name resolves to.
    pub value: T,
    /// Optional description, rendered into the parameter description.
    pub description: String,
}

impl<T> Opt<T> {
    /// Option whose value is `value` and description empty.
    pub fn new(name: impl Into<String>, value: T) -> Self {
        Self {
            name: name.into(),
            value,
            description: String::new(),
        }
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

impl Opt<String> {
    /// Option whose value is its own name (the TS `new Option(name)` default).
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self::new(name.clone(), name)
    }
}

/// A developer-supplied set the model picks from by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice<T> {
    /// The selectable options, in schema order.
    pub options: Vec<Opt<T>>,
    /// When set, the parameter is an array-of-enum and the model may pick several.
    pub multiple: bool,
}

impl<T: Clone> Choice<T> {
    /// Single-select choice.
    pub fn new(options: Vec<Opt<T>>) -> Self {
        Self {
            options,
            multiple: false,
        }
    }

    /// Multi-select choice (array-of-enum parameter).
    pub fn multiple(options: Vec<Opt<T>>) -> Self {
        Self {
            options,
            multiple: true,
        }
    }

    /// Option names, in order.
    pub fn names(&self) -> Vec<String> {
        self.options.iter().map(|o| o.name.clone()).collect()
    }

    /// Whether `name` is one of the options.
    pub fn has(&self, name: &str) -> bool {
        self.options.iter().any(|o| o.name == name)
    }

    /// The matching option's value, if any.
    pub fn value_for(&self, name: &str) -> Option<T> {
        self.options
            .iter()
            .find(|o| o.name == name)
            .map(|o| o.value.clone())
    }
}

impl Choice<String> {
    /// Choice over bare names (each name is its own value).
    pub fn of_names<I, S>(names: I, multiple: bool) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            options: names.into_iter().map(Opt::named).collect(),
            multiple,
        }
    }

    /// Like TS `valueFor`: falls back to the name itself.
    fn value_or_name(&self, name: &str) -> String {
        self.value_for(name).unwrap_or_else(|| name.to_owned())
    }
}

/// How much of the parent conversation a child sees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    /// Fresh start: only the task.
    #[default]
    None,
    /// Full history, including tool calls and their results.
    All,
    /// Text turns only, rendered as a transcript.
    NoTools,
}

impl ContextMode {
    /// Wire name (`none` / `all` / `no_tools`).
    pub fn as_str(self) -> &'static str {
        match self {
            ContextMode::None => "none",
            ContextMode::All => "all",
            ContextMode::NoTools => "no_tools",
        }
    }

    /// Parse a wire name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(ContextMode::None),
            "all" => Some(ContextMode::All),
            "no_tools" => Some(ContextMode::NoTools),
            _ => None,
        }
    }
}

/// Authority over the child's system prompt.
#[derive(Clone, Debug, PartialEq)]
pub enum InstructionsMode {
    /// Free-form string parameter.
    Open(Open),
    /// Pick from a set of prompts.
    Choice(Choice<String>),
    /// Pinned (`None` = the harness default prompt).
    Fixed(Fixed<Option<String>>),
}

impl Default for InstructionsMode {
    fn default() -> Self {
        InstructionsMode::Open(Open)
    }
}

/// Authority over the child's tool set.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolsMode {
    /// Pick a subset by name (`multiple` must be true).
    Choice(Choice<String>),
    /// Pinned (`None` = inherit the parent's tools).
    Fixed(Fixed<Option<Vec<String>>>),
    /// Inherit the parent's tools; no parameter.
    Inherit(Inherit),
}

/// Authority over the child's model.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelMode {
    /// Inherit the parent's model; no parameter.
    Inherit(Inherit),
    /// Pick from a set of model ids.
    Choice(Choice<String>),
    /// Pinned model id (`None` = inherit).
    Fixed(Fixed<Option<String>>),
}

impl Default for ModelMode {
    fn default() -> Self {
        ModelMode::Inherit(Inherit)
    }
}

/// Authority over context forwarding.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextAxis {
    /// Pinned mode.
    Fixed(Fixed<Option<ContextMode>>),
    /// The model picks a mode.
    Choice(Choice<ContextMode>),
}

impl Default for ContextAxis {
    fn default() -> Self {
        ContextAxis::Fixed(Fixed(Some(ContextMode::None)))
    }
}

// --- presets and specs ------------------------------------------------------------------------

/// Fields of a named subagent role.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetOptions {
    /// System prompt.
    pub instructions: Option<String>,
    /// Tool names.
    pub tools: Option<Vec<String>>,
    /// Model id.
    pub model: Option<String>,
    /// Context mode (defaults to `none`).
    pub context: Option<ContextMode>,
    /// Context message cap.
    pub last_messages: Option<i64>,
    /// One-line role description, shown in the `agent_type` enum docs.
    pub description: String,
}

/// A named subagent role.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    /// System prompt.
    pub instructions: Option<String>,
    /// Tool names.
    pub tools: Option<Vec<String>>,
    /// Model id.
    pub model: Option<String>,
    /// Context mode.
    pub context: ContextMode,
    /// Context message cap.
    pub last_messages: Option<i64>,
    /// Role description.
    pub description: String,
}

impl Preset {
    /// Preset from its options (`context` defaults to `none`).
    pub fn new(options: PresetOptions) -> Self {
        Self {
            instructions: options.instructions,
            tools: options.tools,
            model: options.model,
            context: options.context.unwrap_or(ContextMode::None),
            last_messages: options.last_messages,
            description: options.description,
        }
    }

    /// The built-in general-purpose preset.
    pub fn generalist() -> Self {
        Preset::new(PresetOptions {
            instructions: Some(GENERALIST_INSTRUCTIONS.to_owned()),
            description: "a general-purpose agent for a focused subtask that runs in its own context"
                .to_owned(),
            ..Default::default()
        })
    }
}

/// A resolved delegation request: what the child agent should be.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSpec {
    /// The task prompt (before context forwarding is applied).
    pub task: String,
    /// Selected preset name, if any.
    pub agent_type: Option<String>,
    /// System prompt; `None` = the harness default.
    pub instructions: Option<String>,
    /// Tool names; `None` = inherit the parent's tools.
    pub tools: Option<Vec<String>>,
    /// Model id; `None` = inherit.
    pub model: Option<String>,
    /// How much parent context to forward.
    pub context: ContextMode,
    /// Cap on forwarded messages.
    pub last_messages: Option<i64>,
}

/// What the injected builder receives (the resolved spec).
pub type ChildAgentConfig = AgentSpec;

/// Builds the child agent from a resolved spec.
pub type AgentBuilderFn = Arc<dyn Fn(ChildAgentConfig) -> BoxFuture<'static, Result<Agent>> + Send + Sync>;

/// Configuration for [`make_subagent`].
#[derive(Clone)]
pub struct MakeSubagentOptions {
    /// Builds the child agent.
    pub builder: AgentBuilderFn,
    /// Named roles, in `agent_type` enum order.
    pub presets: BTreeMap<String, Preset>,
    /// Preset used when the model omits `agent_type` (defaults to the first preset).
    pub default_preset: Option<String>,
    /// Authority over the child's system prompt.
    pub instructions: InstructionsMode,
    /// Authority over the child's tool set (defaults to `Choice(inherited_tools, multiple)`).
    pub tools: Option<ToolsMode>,
    /// Authority over the child's model.
    pub model: ModelMode,
    /// Authority over context forwarding.
    pub context: ContextAxis,
    /// Tool names the parent has, offered to the child by default.
    pub inherited_tools: Vec<String>,
    /// Delegation depth budget.
    pub max_depth: u32,
    /// Tool name.
    pub name: String,
}

impl MakeSubagentOptions {
    /// Options with TS defaults and the given builder.
    pub fn new(builder: AgentBuilderFn) -> Self {
        Self {
            builder,
            presets: BTreeMap::new(),
            default_preset: None,
            instructions: InstructionsMode::default(),
            tools: None,
            model: ModelMode::default(),
            context: ContextAxis::default(),
            inherited_tools: Vec::new(),
            max_depth: DEFAULT_SUBAGENT_MAX_DEPTH,
            name: "subagent".to_owned(),
        }
    }
}

// --- schema -----------------------------------------------------------------------------------

fn choice_prop<T: Clone>(choice: &Choice<T>, base: &str) -> Value
where
    Opt<T>: Clone,
{
    let names: Vec<String> = choice.options.iter().map(|o| o.name.clone()).collect();
    let described: Vec<&Opt<T>> = choice
        .options
        .iter()
        .filter(|o| !o.description.is_empty())
        .collect();
    let mut description = base.to_owned();
    if !described.is_empty() {
        let lines: Vec<String> = choice
            .options
            .iter()
            .map(|o| format!("- {}: {}", o.name, o.description))
            .collect();
        description = format!("{description}\nOptions:\n{}", lines.join("\n"));
    }
    if choice.multiple {
        json!({ "type": "array", "items": { "type": "string", "enum": names }, "description": description })
    } else {
        json!({ "type": "string", "enum": names, "description": description })
    }
}

/// Build the model-facing input schema for the configured axes.
fn build_schema(
    presets: &BTreeMap<String, Preset>,
    instructions: &InstructionsMode,
    tools: &ToolsMode,
    model: &ModelMode,
    context: &ContextAxis,
) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "task".to_owned(),
        json!({ "type": "string", "description": TASK_DESCRIPTION }),
    );
    if !presets.is_empty() {
        properties.insert(
            "agent_type".to_owned(),
            json!({
                "type": "string",
                "enum": presets.keys().cloned().collect::<Vec<_>>(),
                "description": AGENT_TYPE_DESCRIPTION,
            }),
        );
    }
    match instructions {
        InstructionsMode::Open(_) => {
            properties.insert(
                "instructions".to_owned(),
                json!({ "type": "string", "description": INSTRUCTIONS_DESCRIPTION }),
            );
        }
        InstructionsMode::Choice(choice) => {
            properties.insert(
                "instructions".to_owned(),
                choice_prop(choice, INSTRUCTIONS_DESCRIPTION),
            );
        }
        InstructionsMode::Fixed(_) => {}
    }
    if let ToolsMode::Choice(choice) = tools {
        properties.insert("tools".to_owned(), choice_prop(choice, TOOLS_DESCRIPTION));
    }
    if let ModelMode::Choice(choice) = model {
        properties.insert("model".to_owned(), choice_prop(choice, MODEL_DESCRIPTION));
    }
    if let ContextAxis::Choice(choice) = context {
        properties.insert("context".to_owned(), choice_prop(choice, CONTEXT_DESCRIPTION));
        if choice.options.iter().any(|o| o.value != ContextMode::None) {
            properties.insert(
                "last_messages".to_owned(),
                json!({ "type": "integer", "description": LAST_MESSAGES_DESCRIPTION }),
            );
        }
    }
    json!({ "type": "object", "properties": Value::Object(properties), "required": ["task"] })
}

fn describe_roles(base: &str, presets: &BTreeMap<String, Preset>) -> String {
    if presets.is_empty() {
        return base.to_owned();
    }
    let lines: Vec<String> = presets
        .iter()
        .map(|(name, preset)| {
            let desc = if preset.description.is_empty() {
                name.as_str()
            } else {
                preset.description.as_str()
            };
            format!("- {name}: {desc}")
        })
        .collect();
    format!(
        "{base}\n\nAvailable subagents (agent_type):\n{}",
        lines.join("\n")
    )
}

// --- the tool ---------------------------------------------------------------------------------

/// Delegation tool produced by [`make_subagent`].
pub struct SubagentTool {
    spec: ToolSpec,
    presets: BTreeMap<String, Preset>,
    default_preset: Option<String>,
    instructions: InstructionsMode,
    tools: ToolsMode,
    model: ModelMode,
    context: ContextAxis,
    builder: AgentBuilderFn,
    max_depth: u32,
}

impl std::fmt::Debug for SubagentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentTool")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

/// Build a delegation tool whose schema is derived from the configured axis modes.
pub fn make_subagent(options: MakeSubagentOptions) -> Result<SubagentTool> {
    let MakeSubagentOptions {
        builder,
        presets,
        default_preset,
        instructions,
        tools,
        model,
        context,
        inherited_tools,
        max_depth,
        name,
    } = options;

    let default_preset = default_preset.or_else(|| presets.keys().next().cloned());

    let tools = match tools {
        None => ToolsMode::Choice(Choice::of_names(inherited_tools.clone(), true)),
        Some(ToolsMode::Choice(choice)) => {
            if choice.options.is_empty() {
                return Err(Error::other(
                    "tools=Choice([]) offers no options; use Fixed([]) for a toolless child, or Inherit().",
                ));
            }
            if !choice.multiple {
                return Err(Error::other(
                    "tools=Choice(...) must be multiple=true; a subagent selects a subset of tools.",
                ));
            }
            ToolsMode::Choice(choice)
        }
        Some(other) => other,
    };

    let spec = ToolSpec {
        name: name.clone(),
        description: describe_roles(BASE_DESCRIPTION, &presets),
        input_schema: Some(build_schema(&presets, &instructions, &tools, &model, &context)),
        output_schema: None,
        annotations: None,
    };

    Ok(SubagentTool {
        spec,
        presets,
        default_preset,
        instructions,
        tools,
        model,
        context,
        builder,
        max_depth,
    })
}

impl SubagentTool {
    /// Names the model may pick on `tools`, when that axis is a `Choice`.
    fn allowed_tools(&self) -> Option<Vec<String>> {
        match &self.tools {
            ToolsMode::Choice(choice) => Some(choice.names()),
            _ => None,
        }
    }

    /// Resolve raw tool input into an [`AgentSpec`].
    pub fn resolve_spec(&self, raw: &Map<String, Value>) -> Result<AgentSpec> {
        let task = match raw.get("task") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };

        let requested_type = raw.get("agent_type").and_then(Value::as_str);
        if let Some(t) = requested_type {
            if !self.presets.contains_key(t) {
                return Err(Error::other(format!(
                    "Unknown agent_type {}; valid values: {}.",
                    serde_json::to_string(t).unwrap_or_else(|_| format!("\"{t}\"")),
                    self.presets.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
        }

        let overriding_instructions =
            !matches!(self.instructions, InstructionsMode::Fixed(_)) && raw.contains_key("instructions");
        let agent_type = match requested_type {
            Some(t) => Some(t.to_owned()),
            None if overriding_instructions => None,
            None => self.default_preset.clone(),
        };
        let preset = agent_type.as_deref().and_then(|t| self.presets.get(t));

        let mut spec = AgentSpec {
            task,
            agent_type: agent_type.clone(),
            ..Default::default()
        };

        // instructions
        spec.instructions = match (&self.instructions, raw.get("instructions")) {
            (InstructionsMode::Open(_), Some(v)) => Some(as_string(v)),
            (InstructionsMode::Choice(choice), Some(Value::String(name))) if choice.has(name) => {
                Some(choice.value_or_name(name))
            }
            (InstructionsMode::Fixed(Fixed(value)), _) => match preset.and_then(|p| p.instructions.clone()) {
                Some(p) => Some(p),
                None => value.clone(),
            },
            _ => preset.and_then(|p| p.instructions.clone()),
        };

        // tools
        let allowed = self.allowed_tools();
        spec.tools = match (&self.tools, raw.get("tools"), allowed.as_ref()) {
            (ToolsMode::Choice(choice), Some(v), Some(allowed)) => {
                let requested = as_string_list(v);
                Some(
                    requested
                        .into_iter()
                        .filter(|n| allowed.contains(n))
                        .map(|n| choice.value_or_name(&n))
                        .collect(),
                )
            }
            (ToolsMode::Choice(choice), None, Some(allowed)) => match preset.and_then(|p| p.tools.clone()) {
                Some(preset_tools) => {
                    let values: Vec<String> = allowed.iter().map(|n| choice.value_or_name(n)).collect();
                    Some(preset_tools.into_iter().filter(|t| values.contains(t)).collect())
                }
                None if choice.multiple => Some(allowed.iter().map(|n| choice.value_or_name(n)).collect()),
                None => None,
            },
            (ToolsMode::Fixed(Fixed(value)), _, _) => match preset.and_then(|p| p.tools.clone()) {
                Some(p) => Some(p),
                None => value.clone(),
            },
            _ => preset.and_then(|p| p.tools.clone()),
        };

        // model
        spec.model = match (&self.model, raw.get("model")) {
            (ModelMode::Choice(choice), Some(Value::String(name))) if choice.has(name) => {
                Some(choice.value_or_name(name))
            }
            (ModelMode::Fixed(Fixed(value)), _) => match preset.and_then(|p| p.model.clone()) {
                Some(p) => Some(p),
                None => value.clone(),
            },
            _ => preset.and_then(|p| p.model.clone()),
        };

        // context + last_messages
        match (&self.context, raw.get("context")) {
            (ContextAxis::Choice(choice), Some(Value::String(name))) if choice.has(name) => {
                spec.context = choice.value_for(name).unwrap_or_default();
            }
            (ContextAxis::Fixed(Fixed(value)), _) => {
                if let Some(preset) = preset {
                    spec.context = preset.context;
                    spec.last_messages = preset.last_messages;
                } else if let Some(mode) = value {
                    spec.context = *mode;
                }
            }
            _ => {
                if let Some(preset) = preset {
                    spec.context = preset.context;
                    spec.last_messages = preset.last_messages;
                }
            }
        }
        if let Some(v) = raw.get("last_messages") {
            if !v.is_null() {
                spec.last_messages = v.as_f64().filter(|n| n.is_finite()).map(|n| n.trunc() as i64);
            }
        }

        Ok(spec)
    }
}

fn as_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn as_string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().map(as_string).collect(),
        Value::Null => Vec::new(),
        other => vec![as_string(other)],
    }
}

// --- context forwarding -----------------------------------------------------------------------

fn has_text_or_other(message: &Message) -> bool {
    !message.content.is_empty()
}

/// Clone the parent's messages, dropping reasoning blocks and unanswered tool uses.
pub fn fork_messages(parent: &[Message], last_n: Option<i64>) -> Vec<Message> {
    let answered: Vec<String> = parent
        .iter()
        .flat_map(|m| m.tool_results().map(|r| r.tool_use_id.clone()))
        .collect();

    let mut forked: Vec<Message> = Vec::new();
    for message in parent {
        let content: Vec<ContentBlock> = message
            .content
            .iter()
            .filter(|block| match block {
                ContentBlock::Reasoning(_) => false,
                ContentBlock::ToolUse(use_block) => answered.contains(&use_block.tool_use_id),
                _ => true,
            })
            .cloned()
            .collect();
        let candidate = Message::new(message.role, content);
        if has_text_or_other(&candidate) {
            forked.push(candidate);
        }
    }

    let Some(last_n) = last_n.filter(|n| *n > 0) else {
        return forked;
    };
    let start = forked.len().saturating_sub(last_n as usize);
    let mut index = start;
    while index > 0 && !is_valid_trim_point(&forked, index) {
        index -= 1;
    }
    forked.split_off(index.min(forked.len()))
}

fn is_valid_trim_point(messages: &[Message], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if message.role != Role::User {
        return false;
    }
    if message.tool_results().next().is_some() {
        return false;
    }
    if message.tool_uses().next().is_some() {
        return messages
            .get(index + 1)
            .is_some_and(|next| next.tool_results().next().is_some());
    }
    true
}

/// Append the task to a forked history (`context: all`).
pub fn with_history(task: &str, mut messages: Vec<Message>) -> Vec<Message> {
    if messages.is_empty() {
        return vec![Message::user(task)];
    }
    let block = ContentBlock::text(format!("{FORK_PREAMBLE}\n\n{task}"));
    let last_is_user = messages.last().is_some_and(|m| m.role == Role::User);
    if last_is_user {
        if let Some(last) = messages.last_mut() {
            last.content.push(block);
        }
    } else {
        messages.push(Message::new(Role::User, vec![block]));
    }
    messages
}

fn strip_framing(text: &str) -> String {
    let opener = "<parent_context>\n";
    if !text.starts_with(opener) {
        return text.to_owned();
    }
    let marker = format!("</parent_context>\n\n{CONTEXT_PREAMBLE}\n\n");
    match text.find(&marker) {
        Some(at) => text[at + marker.len()..].to_owned(),
        None => text.to_owned(),
    }
}

/// Render the parent conversation as an indented transcript (`context: no_tools`).
pub fn render_context(parent: &[Message], last_n: Option<i64>) -> String {
    let mut lines: Vec<String> = Vec::new();
    for message in parent {
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        let text = strip_framing(text.trim());
        if text.is_empty() {
            continue;
        }
        let role = if message.role == Role::Assistant {
            "assistant"
        } else {
            "user"
        };
        let indented = text.replace('\n', "\n  ");
        lines.push(format!("{role}: {indented}"));
    }
    if let Some(last_n) = last_n.filter(|n| *n > 0) {
        let start = lines.len().saturating_sub(last_n as usize);
        lines = lines.split_off(start);
    }
    lines
        .join("\n")
        .replace("</parent_context>", "<\\/parent_context>")
        .replace("<parent_context>", "<\\parent_context>")
}

/// Wrap a rendered transcript and the task into one prompt (`context: no_tools`).
pub fn with_context(task: &str, block: &str) -> String {
    if block.trim().is_empty() {
        return task.to_owned();
    }
    format!("<parent_context>\n{block}\n</parent_context>\n\n{CONTEXT_PREAMBLE}\n\n{task}")
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let tool_use_id = ctx.tool_use.tool_use_id.clone();

        let depth = match ctx.agent.state().get(DEPTH_STATE_KEY) {
            None => i64::from(self.max_depth),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite())
                .map(|n| n as i64)
                .unwrap_or(0),
        };
        if depth <= 0 {
            return Ok(create_error_result(
                format!(
                    "Delegation depth limit reached ({} levels); you cannot delegate further. Complete this task yourself instead of calling subagent again.",
                    self.max_depth
                ),
                &tool_use_id,
            ));
        }

        let raw = match &ctx.tool_use.input {
            Value::Object(map) => map.clone(),
            _ => Map::new(),
        };

        let spec = match self.resolve_spec(&raw) {
            Ok(spec) => spec,
            Err(err) => return Ok(subagent_error(&err.to_string(), &tool_use_id)),
        };
        let child = match (self.builder)(spec.clone()).await {
            Ok(child) => child,
            Err(err) => return Ok(subagent_error(&err.to_string(), &tool_use_id)),
        };
        child.state().set(DEPTH_STATE_KEY, depth - 1);

        let prompt = match spec.context {
            ContextMode::None => vec![Message::user(&spec.task)],
            ContextMode::All => with_history(
                &spec.task,
                fork_messages(&ctx.agent.messages(), spec.last_messages),
            ),
            ContextMode::NoTools => vec![Message::user(with_context(
                &spec.task,
                &render_context(&ctx.agent.messages(), spec.last_messages),
            ))],
        };

        let options = InvokeOptions::default().with_cancel(ctx.cancel.clone());
        let mut stream = child.stream_with(prompt, options);
        let mut result = None;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    if let Some(agent_result) = event.result() {
                        result = Some(agent_result.clone());
                    }
                    // `AgentStreamEvent` is not serializable, so forward its type name.
                    ctx.progress(json!({ "type": event.type_name() })).await;
                }
                Err(err) => return Ok(subagent_error(&err.to_string(), &tool_use_id)),
            }
        }

        let Some(result) = result else {
            return Ok(subagent_error_raw("Subagent produced no result.", &tool_use_id));
        };
        match result.stop_reason {
            StopReason::Cancelled => Ok(subagent_error_raw("Subagent was cancelled.", &tool_use_id)),
            StopReason::Interrupt => Ok(subagent_error_raw(
                "Subagent paused for human input, which delegation does not yet forward.",
                &tool_use_id,
            )),
            _ => Ok(ToolResultBlock::success(
                &tool_use_id,
                vec![ToolResultContent::Text(result.to_string())],
            )),
        }
    }
}

fn subagent_error(message: &str, tool_use_id: &str) -> ToolResultBlock {
    create_error_result(format!("Subagent error: {message}"), tool_use_id)
}

fn subagent_error_raw(message: &str, tool_use_id: &str) -> ToolResultBlock {
    create_error_result(message, tool_use_id)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use strands_agents::agent::InvocationState;
    use strands_agents::test_support::{MockMessageModel, MockResponse};
    use strands_agents::types::{ToolUse, ToolUseBlock};
    use tokio_util::sync::CancellationToken;

    use super::*;

    type Captured = Arc<Mutex<Vec<AgentSpec>>>;

    fn mock(text: &str) -> Arc<MockMessageModel> {
        MockMessageModel::shared(vec![MockResponse::message(Message::assistant(text))])
    }

    /// A builder that records every spec and answers with `reply`.
    fn recording_builder(reply: &str) -> (AgentBuilderFn, Captured, Arc<MockMessageModel>) {
        let specs: Captured = Arc::new(Mutex::new(Vec::new()));
        let model = mock(reply);
        let (seen, child_model) = (specs.clone(), model.clone());
        let builder: AgentBuilderFn = Arc::new(move |spec: ChildAgentConfig| {
            let (seen, child_model) = (seen.clone(), child_model.clone());
            Box::pin(async move {
                seen.lock().unwrap().push(spec);
                Agent::builder().model(child_model).build()
            })
        });
        (builder, specs, model)
    }

    async fn parent_agent(messages: Vec<Message>) -> Agent {
        Agent::builder()
            .model(mock("parent"))
            .messages(messages)
            .build()
            .unwrap()
    }

    fn ctx_for(agent: &Agent, input: Value) -> ToolContext {
        ToolContext::new(
            ToolUse {
                name: "subagent".to_owned(),
                tool_use_id: "t1".to_owned(),
                input,
            },
            agent.clone(),
            InvocationState::default(),
            CancellationToken::new(),
        )
    }

    fn options(builder: AgentBuilderFn) -> MakeSubagentOptions {
        let mut options = MakeSubagentOptions::new(builder);
        options.inherited_tools = vec!["read".to_owned(), "shell".to_owned()];
        options
    }

    fn properties(tool: &SubagentTool) -> Map<String, Value> {
        tool.spec().input_schema.clone().unwrap()["properties"]
            .as_object()
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn derives_the_expected_parameters_from_the_axis_modes() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.model = ModelMode::Choice(Choice::of_names(["fast", "smart"], false));
        opts.context = ContextAxis::Choice(Choice::multiple(
            CONTEXT_MODES.iter().map(|m| Opt::new(m.as_str(), *m)).collect(),
        ));
        let tool = make_subagent(opts).unwrap();
        let props = properties(&tool);
        assert_eq!(
            props.keys().cloned().collect::<Vec<_>>(),
            [
                "task",
                "instructions",
                "tools",
                "model",
                "context",
                "last_messages"
            ]
        );
        assert_eq!(props["task"]["description"], json!(TASK_DESCRIPTION));
        assert_eq!(
            props["instructions"],
            json!({ "type": "string", "description": INSTRUCTIONS_DESCRIPTION })
        );
        assert_eq!(props["tools"]["type"], json!("array"));
        assert_eq!(props["tools"]["items"]["enum"], json!(["read", "shell"]));
        assert_eq!(props["model"]["enum"], json!(["fast", "smart"]));
        assert_eq!(props["last_messages"]["type"], json!("integer"));
        assert_eq!(
            tool.spec().input_schema.clone().unwrap()["required"],
            json!(["task"])
        );
    }

    #[tokio::test]
    async fn drops_parameters_for_fixed_and_inherit_axes() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.instructions = InstructionsMode::Fixed(Fixed(Some("pinned".to_owned())));
        opts.tools = Some(ToolsMode::Inherit(Inherit));
        opts.model = ModelMode::Inherit(Inherit);
        let tool = make_subagent(opts).unwrap();
        assert_eq!(properties(&tool).keys().cloned().collect::<Vec<_>>(), ["task"]);
    }

    #[tokio::test]
    async fn adds_an_agent_type_enum_and_lists_the_roles_from_presets() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.presets.insert("generalist".to_owned(), Preset::generalist());
        let tool = make_subagent(opts).unwrap();
        let props = properties(&tool);
        assert_eq!(props["agent_type"]["enum"], json!(["generalist"]));
        assert_eq!(props["agent_type"]["description"], json!(AGENT_TYPE_DESCRIPTION));
        assert_eq!(
            tool.description(),
            format!(
                "{BASE_DESCRIPTION}\n\nAvailable subagents (agent_type):\n- generalist: a general-purpose agent for a focused subtask that runs in its own context"
            )
        );
    }

    #[tokio::test]
    async fn rejects_a_misconfigured_tools_choice_at_construction() {
        let (builder, _, _) = recording_builder("ok");
        let mut empty = options(builder.clone());
        empty.tools = Some(ToolsMode::Choice(Choice::multiple(Vec::new())));
        assert_eq!(
            make_subagent(empty).unwrap_err().to_string(),
            "tools=Choice([]) offers no options; use Fixed([]) for a toolless child, or Inherit()."
        );

        let mut single = options(builder);
        single.tools = Some(ToolsMode::Choice(Choice::of_names(["read"], false)));
        assert_eq!(
            make_subagent(single).unwrap_err().to_string(),
            "tools=Choice(...) must be multiple=true; a subagent selects a subset of tools."
        );
    }

    #[tokio::test]
    async fn omits_last_messages_when_the_only_context_mode_is_none() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder.clone());
        opts.context = ContextAxis::Choice(Choice::new(vec![Opt::new("none", ContextMode::None)]));
        assert!(!properties(&make_subagent(opts).unwrap()).contains_key("last_messages"));

        let mut history = options(builder);
        history.context = ContextAxis::Choice(Choice::new(vec![Opt::new("no_tools", ContextMode::NoTools)]));
        assert!(properties(&make_subagent(history).unwrap()).contains_key("last_messages"));
    }

    #[tokio::test]
    async fn renders_per_option_descriptions_into_the_parameter_description() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.model = ModelMode::Choice(Choice::new(vec![
            Opt::new("fast", "model-a".to_owned()).with_description("cheap"),
            Opt::new("smart", "model-b".to_owned()),
        ]));
        let props = properties(&make_subagent(opts).unwrap());
        assert_eq!(
            props["model"]["description"],
            json!(format!("{MODEL_DESCRIPTION}\nOptions:\n- fast: cheap\n- smart: "))
        );
    }

    #[tokio::test]
    async fn builds_the_child_from_the_spec_and_returns_its_output() {
        let (builder, specs, _) = recording_builder("child says hi");
        let tool = make_subagent(options(builder)).unwrap();
        let parent = parent_agent(vec![]).await;
        let result = tool
            .invoke(ctx_for(
                &parent,
                json!({ "task": "do it", "instructions": "be brief" }),
            ))
            .await
            .unwrap();
        assert_eq!(result.text(), "child says hi");
        let spec = specs.lock().unwrap()[0].clone();
        assert_eq!(spec.task, "do it");
        assert_eq!(spec.instructions.as_deref(), Some("be brief"));
        assert_eq!(spec.tools, Some(vec!["read".to_owned(), "shell".to_owned()]));
        assert_eq!(spec.context, ContextMode::None);
    }

    #[tokio::test]
    async fn rejects_an_off_schema_agent_type() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.presets.insert("generalist".to_owned(), Preset::generalist());
        let tool = make_subagent(opts).unwrap();
        let err = tool
            .resolve_spec(json!({ "task": "t", "agent_type": "nope" }).as_object().unwrap())
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Unknown agent_type \"nope\"; valid values: generalist.");
    }

    #[tokio::test]
    async fn lets_ad_hoc_instructions_suppress_the_default_preset() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.presets.insert("generalist".to_owned(), Preset::generalist());
        let tool = make_subagent(opts).unwrap();
        let spec = tool
            .resolve_spec(
                json!({ "task": "t", "instructions": "custom" })
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(spec.agent_type, None);
        assert_eq!(spec.instructions.as_deref(), Some("custom"));

        let default = tool
            .resolve_spec(json!({ "task": "t" }).as_object().unwrap())
            .unwrap();
        assert_eq!(default.agent_type.as_deref(), Some("generalist"));
        assert_eq!(default.instructions.as_deref(), Some(GENERALIST_INSTRUCTIONS));
    }

    #[tokio::test]
    async fn clamps_and_coerces_the_tools_argument() {
        let (builder, _, _) = recording_builder("ok");
        let tool = make_subagent(options(builder)).unwrap();
        let clamped = tool
            .resolve_spec(
                json!({ "task": "t", "tools": ["read", "rm"] })
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(clamped.tools, Some(vec!["read".to_owned()]));

        let scalar = tool
            .resolve_spec(json!({ "task": "t", "tools": "shell" }).as_object().unwrap())
            .unwrap();
        assert_eq!(scalar.tools, Some(vec!["shell".to_owned()]));

        let malformed = tool
            .resolve_spec(json!({ "task": "t", "tools": 7 }).as_object().unwrap())
            .unwrap();
        assert_eq!(malformed.tools, Some(Vec::<String>::new()));
    }

    #[tokio::test]
    async fn ignores_an_off_enum_choice_value_and_a_fixed_axis_argument() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.model = ModelMode::Fixed(Fixed(Some("pinned-model".to_owned())));
        let tool = make_subagent(opts).unwrap();
        let spec = tool
            .resolve_spec(json!({ "task": "t", "model": "sneaky" }).as_object().unwrap())
            .unwrap();
        assert_eq!(spec.model.as_deref(), Some("pinned-model"));
    }

    #[tokio::test]
    async fn maps_a_choice_name_back_to_its_value() {
        let (builder, _, _) = recording_builder("ok");
        let mut opts = options(builder);
        opts.model = ModelMode::Choice(Choice::new(vec![Opt::new("fast", "model-a".to_owned())]));
        let tool = make_subagent(opts).unwrap();
        let spec = tool
            .resolve_spec(json!({ "task": "t", "model": "fast" }).as_object().unwrap())
            .unwrap();
        assert_eq!(spec.model.as_deref(), Some("model-a"));
    }

    #[tokio::test]
    async fn starts_the_child_fresh_under_context_none() {
        let (builder, _, child) = recording_builder("ok");
        let tool = make_subagent(options(builder)).unwrap();
        let parent = parent_agent(vec![Message::user("earlier"), Message::assistant("answer")]).await;
        tool.invoke(ctx_for(&parent, json!({ "task": "fresh task" })))
            .await
            .unwrap();
        let (messages, _) = child.calls()[0].clone();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text(), "fresh task");
    }

    #[tokio::test]
    async fn forks_the_parents_messages_under_context_all() {
        let (builder, _, child) = recording_builder("ok");
        let mut opts = options(builder);
        opts.context = ContextAxis::Fixed(Fixed(Some(ContextMode::All)));
        let tool = make_subagent(opts).unwrap();
        let parent = parent_agent(vec![Message::user("earlier"), Message::assistant("answer")]).await;
        tool.invoke(ctx_for(&parent, json!({ "task": "next" })))
            .await
            .unwrap();
        let (messages, _) = child.calls()[0].clone();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].text(), "earlier");
        assert_eq!(messages[2].text(), format!("{FORK_PREAMBLE}\n\nnext"));
        assert_eq!(messages[2].role, Role::User);
    }

    #[tokio::test]
    async fn drops_in_flight_tool_calls_and_reasoning_under_context_all() {
        let unanswered = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("thinking out loud"),
                ContentBlock::ToolUse(ToolUseBlock {
                    tool_use_id: "pending".to_owned(),
                    name: "read".to_owned(),
                    input: json!({}),
                    reasoning_signature: None,
                }),
            ],
        );
        let forked = fork_messages(&[Message::user("hi"), unanswered], None);
        assert_eq!(forked.len(), 2);
        assert_eq!(forked[1].content.len(), 1);
        assert_eq!(forked[1].text(), "thinking out loud");
    }

    #[tokio::test]
    async fn sends_the_bare_task_under_context_all_when_the_parent_is_empty() {
        let messages = with_history("solo", Vec::new());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text(), "solo");
    }

    #[tokio::test]
    async fn keeps_the_tail_with_last_messages_under_context_all() {
        let history = vec![
            Message::user("one"),
            Message::assistant("two"),
            Message::user("three"),
            Message::assistant("four"),
        ];
        let forked = fork_messages(&history, Some(2));
        assert_eq!(
            forked.iter().map(Message::text).collect::<Vec<_>>(),
            ["three", "four"]
        );
    }

    #[tokio::test]
    async fn renders_a_block_under_context_no_tools() {
        let (builder, _, child) = recording_builder("ok");
        let mut opts = options(builder);
        opts.context = ContextAxis::Fixed(Fixed(Some(ContextMode::NoTools)));
        let tool = make_subagent(opts).unwrap();
        let parent = parent_agent(vec![
            Message::user("line one\nline two"),
            Message::assistant("reply"),
        ])
        .await;
        tool.invoke(ctx_for(&parent, json!({ "task": "go" })))
            .await
            .unwrap();
        let (messages, _) = child.calls()[0].clone();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].text(),
            format!(
                "<parent_context>\nuser: line one\n  line two\nassistant: reply\n</parent_context>\n\n{CONTEXT_PREAMBLE}\n\ngo"
            )
        );
    }

    #[tokio::test]
    async fn strips_a_nested_framing_under_context_no_tools() {
        let nested = format!(
            "<parent_context>\nuser: inner\n</parent_context>\n\n{CONTEXT_PREAMBLE}\n\nthe real task"
        );
        let rendered = render_context(&[Message::user(nested)], None);
        assert_eq!(rendered, "user: the real task");
    }

    #[tokio::test]
    async fn counts_rendered_entries_for_last_messages_under_context_no_tools() {
        let history = vec![
            Message::user("one"),
            Message::assistant("two"),
            Message::user("three"),
        ];
        assert_eq!(render_context(&history, Some(1)), "user: three");
    }

    #[tokio::test]
    async fn refuses_once_the_budget_is_exhausted_without_building_a_child() {
        let (builder, specs, _) = recording_builder("ok");
        let tool = make_subagent(options(builder)).unwrap();
        let parent = parent_agent(vec![]).await;
        parent.state().set(DEPTH_STATE_KEY, 0);
        let result = tool
            .invoke(ctx_for(&parent, json!({ "task": "t" })))
            .await
            .unwrap();
        assert_eq!(
            result.text(),
            "Error: Delegation depth limit reached (2 levels); you cannot delegate further. Complete this task yourself instead of calling subagent again."
        );
        assert!(specs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn starts_a_parent_that_never_delegated_at_max_depth() {
        let (builder, _, _) = recording_builder("ok");
        let tool = make_subagent(options(builder)).unwrap();
        let parent = parent_agent(vec![]).await;
        assert!(parent.state().get(DEPTH_STATE_KEY).is_none());
        let result = tool
            .invoke(ctx_for(&parent, json!({ "task": "t" })))
            .await
            .unwrap();
        assert_eq!(result.text(), "ok");
    }

    #[tokio::test]
    async fn turns_a_failed_build_into_an_error_result() {
        let builder: AgentBuilderFn = Arc::new(|_| Box::pin(async { Err(Error::other("boom")) }));
        let tool = make_subagent(options(builder)).unwrap();
        let parent = parent_agent(vec![]).await;
        let result = tool
            .invoke(ctx_for(&parent, json!({ "task": "t" })))
            .await
            .unwrap();
        assert!(
            result.text().starts_with("Error: Subagent error: "),
            "got {}",
            result.text()
        );
    }
}
