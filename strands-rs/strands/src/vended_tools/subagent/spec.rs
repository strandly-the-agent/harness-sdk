//! The authority-mode axes the subagent tool's schema and child spec come from (TS
//! `multiagent/spec.ts`, the part the subagent tool uses).
//!
//! Each axis takes a policy: `Fixed` pins a value the model can't see, `Inherit` takes the
//! parent's, `Open` lets the model write it, and `Choice` lets the model pick from a set.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::json;

use crate::JsonValue;
use crate::agent::{Agent, ModelSource};
use crate::error::Result;
use crate::experimental::context_manager::ContextManagerPreset;
use crate::plugins::Plugin;
use crate::tools::Tool;
use crate::types::agent::ToolAgent;
use crate::types::json::JsonObject;

/// Turns a resolved spec into a child agent (TS `AgentBuilder`).
pub type AgentBuilder = Arc<dyn Fn(&AgentSpec) -> Result<Agent> + Send + Sync>;

const AGENT_TYPE_DESCRIPTION: &str = "Role to assign this agent.";
const INSTRUCTIONS_DESCRIPTION: &str = "A system prompt defining this agent's role.";
const TOOLS_DESCRIPTION: &str = "Subset of tools to grant this agent. Fewer is safer; it cannot exceed your own. Omit to grant all inherited tools.";
const MODEL_DESCRIPTION: &str = "Which model this agent runs on. Omit to inherit the parent's model.";

/// A selectable entry in a [`Choice`] (TS `Option`, renamed for the Rust prelude).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone)]
pub struct ChoiceOption<T> {
    /// The name the model picks.
    pub name: String,
    /// The value behind the name; `None` uses the name itself.
    pub value: Option<T>,
    /// Shown to the model next to the name when not empty.
    pub description: String,
}

impl<T> ChoiceOption<T> {
    /// An option whose value is its name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
            description: String::new(),
        }
    }

    /// Sets the value behind the name.
    #[must_use]
    pub fn with_value(mut self, value: impl Into<T>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets the description shown to the model.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

impl<T> From<&str> for ChoiceOption<T> {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl<T> From<String> for ChoiceOption<T> {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// The model picks from a developer-supplied set (TS `Choice`); with `multiple` it may pick
/// several.
///
/// ```
/// use strands::vended_tools::subagent::{Choice, ChoiceOption};
///
/// let tools = Choice::<String>::multiple(["read", "shell"]);
/// let style = Choice::<String>::new([ChoiceOption::new("short").with_value("Answer in one line.")]);
/// assert!(tools.multiple && !style.multiple);
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone)]
pub struct Choice<T> {
    /// The options, in the order the model sees them.
    pub options: Vec<ChoiceOption<T>>,
    /// Whether the model may pick more than one.
    pub multiple: bool,
}

impl<T> Choice<T> {
    /// A single-pick choice.
    pub fn new(options: impl IntoIterator<Item = impl Into<ChoiceOption<T>>>) -> Self {
        Self {
            options: options.into_iter().map(Into::into).collect(),
            multiple: false,
        }
    }

    /// A multiple-pick choice.
    pub fn multiple(options: impl IntoIterator<Item = impl Into<ChoiceOption<T>>>) -> Self {
        Self {
            multiple: true,
            ..Self::new(options)
        }
    }

    fn option(&self, name: &str) -> Option<&ChoiceOption<T>> {
        self.options.iter().find(|option| option.name == name)
    }

    fn names(&self) -> Vec<&str> {
        self.options.iter().map(|option| option.name.as_str()).collect()
    }
}

impl<T: Clone + From<String>> Choice<T> {
    /// The value behind `name`, or `name` itself when no option has it (TS `valueFor`).
    fn value_for(&self, name: &str) -> T {
        self.option(name)
            .and_then(|option| option.value.clone())
            .unwrap_or_else(|| T::from(name.to_owned()))
    }
}

/// Policy for the child's system prompt (TS `Open | Choice | Fixed`).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum InstructionsAxis {
    /// The model writes it.
    #[default]
    Open,
    /// The model picks one.
    Choice(Choice<String>),
    /// Pinned (`None` leaves it unset).
    Fixed(Option<String>),
}

/// Policy for the child's tools (TS `Choice | Fixed | Inherit`); a `Choice` must be multiple.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum ToolsAxis {
    /// The parent's tools.
    #[default]
    Inherit,
    /// The model picks a subset of tool names.
    Choice(Choice<String>),
    /// Pinned tool names (`None` inherits).
    Fixed(Option<Vec<String>>),
}

/// Policy for the child's model (TS `Inherit | Choice | Fixed`); an option without a value names
/// a Bedrock model id.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum ModelAxis {
    /// The parent's model.
    #[default]
    Inherit,
    /// The model picks one.
    Choice(Choice<ModelSource>),
    /// Pinned (`None` inherits).
    Fixed(Option<ModelSource>),
}

/// Policy for how much of the parent's conversation the child sees (TS `Fixed | Choice`): the
/// modes are `none`, `all` and `no_tools`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ContextAxis {
    /// Pinned.
    Fixed(Option<String>),
    /// The model picks one.
    Choice(Choice<String>),
}

impl Default for ContextAxis {
    fn default() -> Self {
        Self::Fixed(Some("none".to_owned()))
    }
}

/// A named role the model selects with `agent_type` (TS `Preset`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct Preset {
    /// The child's system prompt.
    pub instructions: Option<String>,
    /// The child's tool names.
    pub tools: Option<Vec<String>>,
    /// The child's model.
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// Shown to the model next to the preset's name.
    #[builder(default)]
    pub description: String,
}

/// The resolved child configuration handed to the builder (TS `AgentSpec`).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct AgentSpec {
    /// The child's name and id: the tool's name.
    pub name: Option<String>,
    /// The selected preset.
    pub agent_type: Option<String>,
    /// The child's system prompt.
    pub instructions: Option<String>,
    /// The child's tool names; `None` inherits all of the parent's.
    pub tools: Option<Vec<String>>,
    /// The child's model; `None` inherits the parent's.
    pub model: Option<ModelSource>,
}

/// The presets and axis policies the schema and specs come from.
#[derive(Debug, Clone)]
pub(super) struct Axes {
    pub(super) presets: IndexMap<String, Preset>,
    pub(super) default_preset: Option<String>,
    pub(super) instructions: InstructionsAxis,
    pub(super) tools: ToolsAxis,
    pub(super) model: ModelAxis,
}

/// A spec resolved from the model's input, with the instructions as given (TS checks their
/// type after resolving).
pub(super) struct Resolved {
    pub(super) spec: AgentSpec,
    pub(super) instructions: Option<JsonValue>,
}

/// The model-visible axes and an `agent_type` choice over the presets, as schema properties
/// (TS `_toSchemaProperties`).
pub(super) fn schema_properties(axes: &Axes) -> Result<JsonObject, String> {
    let mut properties = JsonObject::new();
    if !axes.presets.is_empty() {
        let options: Vec<ChoiceOption<String>> = axes
            .presets
            .iter()
            .map(|(name, preset)| ChoiceOption::new(name.as_str()).with_description(describe_preset(preset)))
            .collect();
        let mut description = AGENT_TYPE_DESCRIPTION.to_owned();
        if let Some(default) = &axes.default_preset {
            description.push_str(&format!(" Omit to use the default ('{default}')."));
        }
        properties.insert(
            "agent_type".into(),
            choice_property("agent_type", description, &Choice::new(options))?,
        );
    }
    match &axes.instructions {
        InstructionsAxis::Open => {
            properties.insert(
                "instructions".into(),
                json!({ "type": "string", "description": INSTRUCTIONS_DESCRIPTION }),
            );
        }
        InstructionsAxis::Choice(choice) => {
            properties.insert(
                "instructions".into(),
                choice_property("instructions", INSTRUCTIONS_DESCRIPTION.to_owned(), choice)?,
            );
        }
        InstructionsAxis::Fixed(_) => {}
    }
    if let ToolsAxis::Choice(choice) = &axes.tools {
        properties.insert(
            "tools".into(),
            choice_property("tools", TOOLS_DESCRIPTION.to_owned(), choice)?,
        );
    }
    if let ModelAxis::Choice(choice) = &axes.model {
        properties.insert(
            "model".into(),
            choice_property("model", MODEL_DESCRIPTION.to_owned(), choice)?,
        );
    }
    Ok(properties)
}

fn choice_property<T>(key: &str, description: String, choice: &Choice<T>) -> Result<JsonValue, String> {
    if choice.options.is_empty() {
        return Err(format!(
            "{key}: new Choice([]) offers no options; use new Fixed(...), or omit {key} to use its default."
        ));
    }
    if key == "tools" && !choice.multiple {
        return Err(format!(
            "{key}: new Choice(...) must set multiple to true; a child agent selects a subset of {key}."
        ));
    }
    let description = with_option_lines(description, choice);
    Ok(if choice.multiple {
        json!({ "type": "array", "items": { "type": "string", "enum": choice.names() }, "description": description })
    } else {
        json!({ "type": "string", "enum": choice.names(), "description": description })
    })
}

/// `description` followed by `Options:` and a `- name: description` line per described option.
pub(super) fn with_option_lines<T>(description: String, choice: &Choice<T>) -> String {
    let lines: Vec<String> = choice
        .options
        .iter()
        .filter(|option| !option.description.is_empty())
        .map(|option| format!("- {}: {}", option.name, option.description))
        .collect();
    if lines.is_empty() {
        description
    } else {
        format!("{description}\nOptions:\n{}", lines.join("\n"))
    }
}

/// The preset's description plus the fields it sets, without the instruction text.
fn describe_preset(preset: &Preset) -> String {
    let mut parts = Vec::new();
    if preset.instructions.is_some() {
        parts.push("instructions".to_owned());
    }
    if let Some(tools) = &preset.tools {
        let names = if tools.is_empty() {
            "none".to_owned()
        } else {
            tools.join(", ")
        };
        parts.push(format!("tools: {names}"));
    }
    match &preset.model {
        Some(ModelSource::Id(id)) => parts.push(format!("model: {id}")),
        Some(_) => parts.push("model".to_owned()),
        None => {}
    }
    if parts.is_empty() {
        return preset.description.clone();
    }
    let sets = format!("sets {}", parts.join("; "));
    if preset.description.is_empty() {
        sets
    } else {
        format!("{} ({sets})", preset.description)
    }
}

/// Combines the model's input with the axis policies (TS `_resolveSpecs` for one input): a
/// model-supplied argument wins, then the preset's value, then the axis default. An omitted
/// `agent_type` falls back to the default preset unless the model wrote its own instructions.
pub(super) fn resolve(name: &str, input: &JsonObject, axes: &Axes) -> Result<Resolved, String> {
    let model_agent_type = present(input, "agent_type");
    let known = model_agent_type
        .and_then(JsonValue::as_str)
        .filter(|agent_type| axes.presets.contains_key(*agent_type));
    if let (Some(value), None) = (model_agent_type, known) {
        let mut valid: Vec<&str> = axes.presets.keys().map(String::as_str).collect();
        valid.sort_unstable();
        return Err(format!(
            "Unknown agent_type {}; valid values: {}.",
            serde_json::to_string(value).unwrap_or_default(),
            valid.join(", ")
        ));
    }
    let overriding_instructions =
        !matches!(axes.instructions, InstructionsAxis::Fixed(_)) && input.contains_key("instructions");
    let agent_type = match known {
        Some(agent_type) => Some(agent_type.to_owned()),
        None if !overriding_instructions => axes.default_preset.clone(),
        None => None,
    };
    let preset = agent_type
        .as_ref()
        .and_then(|agent_type| axes.presets.get(agent_type));
    let instructions = resolve_instructions(
        present(input, "instructions"),
        &axes.instructions,
        preset.and_then(|preset| preset.instructions.as_ref()),
    );
    let spec = AgentSpec {
        name: Some(name.to_owned()),
        agent_type,
        instructions: instructions
            .as_ref()
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        tools: resolve_tools(
            present(input, "tools"),
            &axes.tools,
            preset.and_then(|preset| preset.tools.as_ref()),
        ),
        model: resolve_model(
            present(input, "model").and_then(JsonValue::as_str),
            &axes.model,
            preset.and_then(|preset| preset.model.as_ref()),
        ),
    };
    Ok(Resolved { spec, instructions })
}

/// The value at `key` unless it is missing or `null` (TS `?? UNSET`).
fn present<'a>(input: &'a JsonObject, key: &str) -> Option<&'a JsonValue> {
    input.get(key).filter(|value| !value.is_null())
}

fn resolve_instructions(
    model_value: Option<&JsonValue>,
    axis: &InstructionsAxis,
    preset: Option<&String>,
) -> Option<JsonValue> {
    match (axis, model_value) {
        (InstructionsAxis::Open, Some(value)) => return Some(value.clone()),
        (InstructionsAxis::Choice(choice), Some(JsonValue::String(name)))
            if choice.option(name).is_some() =>
        {
            return Some(JsonValue::String(choice.value_for(name)));
        }
        _ => {}
    }
    if let Some(preset) = preset {
        return Some(JsonValue::String(preset.clone()));
    }
    match axis {
        InstructionsAxis::Fixed(value) => value.clone().map(JsonValue::String),
        _ => None,
    }
}

fn resolve_model(
    model_value: Option<&str>,
    axis: &ModelAxis,
    preset: Option<&ModelSource>,
) -> Option<ModelSource> {
    if let (ModelAxis::Choice(choice), Some(name)) = (axis, model_value)
        && choice.option(name).is_some()
    {
        return Some(choice.value_for(name));
    }
    if let Some(preset) = preset {
        return Some(preset.clone());
    }
    match axis {
        ModelAxis::Fixed(value) => value.clone(),
        _ => None,
    }
}

/// TS `resolveList`: the model's picks, then the preset's, then every choice, else the axis
/// default; a `Choice` drops names that aren't options.
fn resolve_tools(
    model_value: Option<&JsonValue>,
    axis: &ToolsAxis,
    preset: Option<&Vec<String>>,
) -> Option<Vec<String>> {
    if let (ToolsAxis::Choice(choice), Some(value)) = (axis, model_value) {
        let requested: Vec<&str> = match value {
            JsonValue::String(name) => vec![name.as_str()],
            JsonValue::Array(names) => names.iter().filter_map(JsonValue::as_str).collect(),
            _ => Vec::new(),
        };
        return Some(
            requested
                .into_iter()
                .filter(|name| choice.option(name).is_some())
                .map(|name| choice.value_for(name))
                .collect(),
        );
    }
    if let Some(preset) = preset {
        return Some(match axis {
            ToolsAxis::Choice(choice) => {
                let allowed: Vec<String> = choice
                    .names()
                    .into_iter()
                    .map(|name| choice.value_for(name))
                    .collect();
                preset
                    .iter()
                    .filter(|tool| allowed.contains(tool))
                    .cloned()
                    .collect()
            }
            _ => preset.clone(),
        });
    }
    match axis {
        ToolsAxis::Choice(choice) if choice.multiple => Some(
            choice
                .names()
                .into_iter()
                .map(|name| choice.value_for(name))
                .collect(),
        ),
        ToolsAxis::Fixed(value) => value.clone(),
        _ => None,
    }
}

/// Builds a child inheriting the parent's model, tools, sandbox, printer setting, trace
/// attributes and intervention handlers (TS `_defaultBuilder`): `spec.tools` picks from the
/// parent's tools, skipping (with a warning) names it doesn't own, and `spec.name` becomes the
/// child's name and id. The child gets its own `auto` context manager unless its model is
/// stateful, so the parent's context manager tools are not inherited.
pub(super) fn default_build(parent: &ToolAgent, spec: &AgentSpec) -> Result<Agent> {
    let defaults = parent.child_defaults();
    let managed = defaults
        .context_manager
        .as_ref()
        .map(Plugin::get_tools)
        .unwrap_or_default();
    let inherited = |tool: &&Arc<dyn Tool>| {
        !managed
            .iter()
            .any(|managed| std::ptr::addr_eq(Arc::as_ptr(managed), Arc::as_ptr(tool)))
    };
    let registry = parent.tool_registry();
    let tools: Vec<Arc<dyn Tool>> = match &spec.tools {
        None => registry.list().filter(inherited).cloned().collect(),
        Some(names) => names
            .iter()
            .filter_map(|name| {
                let tool = registry.get(name).filter(inherited).cloned();
                if tool.is_none() {
                    tracing::warn!(tool_name = %name, "subagent requested tool but parent does not own it, skipping");
                }
                tool
            })
            .collect(),
    };
    let model = spec
        .model
        .clone()
        .unwrap_or_else(|| ModelSource::Model(Arc::clone(parent.model())));
    // TS reads `stateful` off a model instance only; a router or a model id has none.
    let stateful = matches!(&model, ModelSource::Model(model) if model.stateful());
    Agent::builder()
        .system_prompt(spec.instructions.clone().unwrap_or_default())
        .tools(tools)
        .model(model)
        .maybe_context_manager((!stateful).then_some(ContextManagerPreset::Auto))
        .printer(defaults.printer)
        .trace_attributes(parent.trace_attributes().iter().cloned())
        .interventions(defaults.interventions.iter().cloned())
        .maybe_sandbox(parent.agent_sandbox().source())
        .maybe_name(spec.name.clone())
        .maybe_id(spec.name.clone())
        .build()
}
