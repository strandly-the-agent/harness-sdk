//! The model-facing schema and the spec resolution of the `subagent` tool (TS `buildSchema`,
//! `resolveSpec`).

use std::collections::HashSet;

use indexmap::IndexMap;
use serde_json::{Map, json};
use strands::JsonValue;
use strands::agent::ModelSource;

use super::{
    AgentSpec, Choice, ContextAxis, ContextMode, InstructionsMode, McpServersMode, ModelMode, Preset, ToolsMode,
};
use crate::error::{HarnessError, Result};

const INSTRUCTIONS_DESCRIPTION: &str = "A system prompt defining the subagent's role.";

/// What a picked option name resolves to when its option carries no value (TS defaults an
/// option's `value` to its `name`).
pub(super) trait AxisValue: Clone {
    fn from_name(name: &str) -> Self;
}

impl AxisValue for String {
    fn from_name(name: &str) -> Self {
        name.to_owned()
    }
}

impl AxisValue for ModelSource {
    fn from_name(name: &str) -> Self {
        Self::from(name)
    }
}

impl AxisValue for ContextMode {
    fn from_name(name: &str) -> Self {
        match name {
            "all" => Self::All,
            "no_tools" => Self::NoTools,
            _ => Self::None,
        }
    }
}

fn has<T>(choice: &Choice<T>, name: &str) -> bool {
    choice.options.iter().any(|option| option.name == name)
}

/// The value the picked `name` resolves to.
fn value_for<T: AxisValue>(choice: &Choice<T>, name: &str) -> T {
    choice
        .options
        .iter()
        .find(|option| option.name == name)
        .and_then(|option| option.value.clone())
        .unwrap_or_else(|| T::from_name(name))
}

fn values<T: AxisValue>(choice: &Choice<T>) -> Vec<T> {
    choice
        .options
        .iter()
        .map(|option| value_for(choice, &option.name))
        .collect()
}

/// The names the model asked for that the choice offers, each mapped to its value. A scalar is a
/// one-element list; anything else grants nothing.
fn pick(choice: &Choice<String>, requested: &JsonValue) -> Vec<String> {
    let requested = match requested {
        JsonValue::Array(items) => items.iter().map(js_string).collect(),
        JsonValue::String(name) => vec![name.clone()],
        _ => Vec::new(),
    };
    requested
        .iter()
        .filter(|name| has(choice, name))
        .map(|name| value_for(choice, name))
        .collect()
}

/// A string enum property (an array of enum when the choice is multiple). Per-option descriptions
/// render into the property's description, since JSON Schema has none per enum value.
fn choice_prop<T>(choice: &Choice<T>, base: &str) -> JsonValue {
    let names: Vec<&str> = choice.options.iter().map(|option| option.name.as_str()).collect();
    let lines: Vec<String> = choice
        .options
        .iter()
        .filter(|option| !option.description.is_empty())
        .map(|option| format!("- {}: {}", option.name, option.description))
        .collect();
    let mut prop = if choice.multiple {
        json!({"type": "array", "items": {"type": "string", "enum": names}})
    } else {
        json!({"type": "string", "enum": names})
    };
    let description = match (base.is_empty(), lines.is_empty()) {
        (_, true) => base.to_owned(),
        (true, false) => format!("Options:\n{}", lines.join("\n")),
        (false, false) => format!("{base}\nOptions:\n{}", lines.join("\n")),
    };
    if !description.is_empty()
        && let Some(prop) = prop.as_object_mut()
    {
        prop.insert("description".to_owned(), JsonValue::String(description));
    }
    prop
}

/// The axis modes and presets of one `subagent` tool.
pub(super) struct Axes {
    pub(super) presets: IndexMap<String, Preset>,
    pub(super) default_preset: Option<String>,
    pub(super) instructions: InstructionsMode,
    pub(super) tools: ToolsMode,
    pub(super) mcp_servers: McpServersMode,
    pub(super) model: ModelMode,
    pub(super) context: ContextAxis,
}

impl Axes {
    pub(super) fn describe_roles(&self, base: &str) -> String {
        if self.presets.is_empty() {
            return base.to_owned();
        }
        let roles: Vec<String> = self
            .presets
            .iter()
            .map(|(name, preset)| {
                let description = if preset.description.is_empty() {
                    name
                } else {
                    &preset.description
                };
                format!("- {name}: {description}")
            })
            .collect();
        format!("{base}\n\nAvailable subagents (agent_type):\n{}", roles.join("\n"))
    }

    /// Derives the input schema from the axis modes: each axis adds zero or one parameter.
    pub(super) fn schema(&self) -> JsonValue {
        let mut props = Map::new();
        props.insert(
            "task".to_owned(),
            json!({
                "type": "string",
                "description": "The self-contained task, including all context the subagent needs. It starts from a blank conversation and cannot ask follow-up questions.",
            }),
        );
        if !self.presets.is_empty() {
            props.insert(
                "agent_type".to_owned(),
                json!({
                    "type": "string",
                    "enum": self.presets.keys().collect::<Vec<_>>(),
                    "description": "Which subagent role to use. Omit to use the default.",
                }),
            );
        }
        match &self.instructions {
            InstructionsMode::Open => {
                props.insert(
                    "instructions".to_owned(),
                    json!({"type": "string", "description": INSTRUCTIONS_DESCRIPTION}),
                );
            }
            InstructionsMode::Choice(choice) => {
                props.insert("instructions".to_owned(), choice_prop(choice, INSTRUCTIONS_DESCRIPTION));
            }
            InstructionsMode::Fixed(_) => {}
        }
        if let ToolsMode::Choice(choice) = &self.tools {
            props.insert(
                "tools".to_owned(),
                choice_prop(
                    choice,
                    "Subset of tools to grant the subagent. Fewer is safer; it cannot exceed your own.",
                ),
            );
        }
        if let McpServersMode::Choice(choice) = &self.mcp_servers {
            props.insert(
                "mcp_servers".to_owned(),
                choice_prop(
                    choice,
                    "Subset of MCP servers whose tools to grant the subagent. Fewer is safer; it cannot exceed your own. Omit to grant all of them.",
                ),
            );
        }
        if let ModelMode::Choice(choice) = &self.model {
            props.insert(
                "model".to_owned(),
                choice_prop(choice, "Which model the subagent runs on."),
            );
        }
        if let ContextAxis::Choice(choice) = &self.context {
            props.insert(
                "context".to_owned(),
                choice_prop(
                    choice,
                    "How much of this conversation the subagent sees: 'none' (fresh start), 'all' (full history including tool calls and their results — can be large), 'no_tools' (text turns only — tool calls and their results removed). More context costs more tokens.",
                ),
            );
            if values(choice).iter().any(|mode| *mode != ContextMode::None) {
                props.insert(
                    "last_messages".to_owned(),
                    json!({
                        "type": "integer",
                        "description": "Optional: limit the shared context to the last N messages. Omit to share all of the selected context.",
                    }),
                );
            }
        }
        json!({"type": "object", "properties": props, "required": ["task"]})
    }

    /// Combines the model's arguments, the selected preset and the fixed axes into a spec.
    ///
    /// Per axis a model-supplied argument wins, then the preset's value, then the axis default.
    /// An omitted `agent_type` falls back to the default preset.
    pub(super) fn resolve(&self, input: &JsonValue) -> Result<AgentSpec> {
        let empty = Map::new();
        let raw = input.as_object().unwrap_or(&empty);
        let task = raw
            .get("task")
            .map_or_else(|| "undefined".to_owned(), js_string);

        // agent_type is a closed enum.
        let raw_agent_type = raw.get("agent_type").filter(|value| !value.is_null());
        let known = raw_agent_type
            .and_then(JsonValue::as_str)
            .filter(|name| self.presets.contains_key(*name));
        if let (Some(value), None) = (raw_agent_type, known) {
            return Err(HarnessError::UnknownAgentType {
                value: value.to_string(),
                valid: self.presets.keys().cloned().collect::<Vec<_>>().join(", "),
            });
        }
        // Ad-hoc instructions (only possible when the axis exposes one) override the default preset.
        let overriding = !matches!(self.instructions, InstructionsMode::Fixed(_)) && raw.contains_key("instructions");
        let agent_type = match known {
            Some(name) => Some(name.to_owned()),
            None if overriding => None,
            None => self.default_preset.clone(),
        };
        let preset = agent_type
            .as_deref()
            .filter(|name| !name.is_empty())
            .and_then(|name| self.presets.get(name));
        let mut spec = AgentSpec::new(task, agent_type.clone());

        // instructions: honoured only when the axis exposes one (Open, or a Choice offering the name).
        let raw_instructions = raw.get("instructions").map(js_string);
        spec.instructions = match (&self.instructions, raw_instructions) {
            (InstructionsMode::Open, Some(text)) => Some(text),
            (InstructionsMode::Choice(choice), Some(name)) if has(choice, &name) => Some(value_for(choice, &name)),
            (axis, _) => match (preset, axis) {
                (Some(preset), _) => preset.instructions.clone(),
                (None, InstructionsMode::Fixed(value)) => value.clone(),
                (None, _) => None,
            },
        };

        // tools: the model picks option names; each maps to the tool name it grants, clamped.
        let tools_choice = match &self.tools {
            ToolsMode::Choice(choice) => Some(choice),
            _ => None,
        };
        let preset_tools = preset.and_then(|preset| preset.tools.as_ref());
        spec.tools = match (raw.get("tools"), tools_choice, preset_tools) {
            (Some(requested), Some(choice), _) => Some(pick(choice, requested)),
            (_, Some(choice), Some(preset_tools)) => {
                let allowed: HashSet<String> = values(choice).into_iter().collect();
                Some(
                    preset_tools
                        .iter()
                        .filter(|tool| allowed.contains(*tool))
                        .cloned()
                        .collect(),
                )
            }
            (_, None, Some(preset_tools)) => Some(preset_tools.clone()),
            (_, Some(choice), None) if choice.multiple => Some(values(choice)),
            (_, _, None) => match &self.tools {
                ToolsMode::Fixed(value) => value.clone(),
                _ => None,
            },
        };

        // mcp_servers: the model picks server names, clamped to the parent's set; omitted grants all.
        spec.mcp_servers = match (raw.get("mcp_servers"), &self.mcp_servers) {
            (Some(requested), McpServersMode::Choice(choice)) => Some(pick(choice, requested)),
            (None, McpServersMode::Choice(choice)) if choice.multiple => Some(values(choice)),
            (_, McpServersMode::Fixed(value)) => value.clone(),
            _ => None,
        };

        // model: the picked name maps back to its value; an off-enum name is ignored.
        let raw_model = raw.get("model").map(js_string);
        spec.model = match (&self.model, raw_model) {
            (ModelMode::Choice(choice), Some(name)) if has(choice, &name) => Some(value_for(choice, &name)),
            (axis, _) => match (preset.and_then(|preset| preset.model.clone()), axis) {
                (Some(model), _) => Some(model),
                (None, ModelMode::Fixed(value)) => value.clone(),
                (None, _) => None,
            },
        };

        // context: honoured only when the Choice offers the name.
        let raw_context = raw.get("context").map(js_string);
        match (&self.context, raw_context, preset) {
            (ContextAxis::Choice(choice), Some(name), _) if has(choice, &name) => {
                spec.context = value_for(choice, &name);
            }
            (_, _, Some(preset)) => {
                spec.context = preset.context;
                spec.last_messages = preset.last_messages;
            }
            (ContextAxis::Fixed(mode), _, None) => spec.context = *mode,
            (ContextAxis::Choice(_), _, None) => {}
        }

        // last_messages: a model-supplied cap wins.
        if let Some(value) = raw.get("last_messages").filter(|value| !value.is_null()) {
            spec.last_messages = integer(js_number(value));
        }
        Ok(spec)
    }
}

/// The depth budget left on the parent: `max_depth` for a parent that never delegated, and 0 for
/// a stored value that is not a finite number, so it can never slip past the guard.
pub(super) fn remaining_depth(stored: Option<&JsonValue>, max_depth: u32) -> f64 {
    match stored {
        None => f64::from(max_depth),
        Some(value) => Some(js_number(value))
            .filter(|depth| depth.is_finite())
            .unwrap_or(0.0),
    }
}

/// `value` as a JSON number, an integer when it has no fraction (as JavaScript serialises it).
pub(super) fn json_number(value: f64) -> JsonValue {
    integer(value)
        .filter(|_| value.fract() == 0.0)
        .map_or_else(|| json!(value), |whole| json!(whole))
}

/// JavaScript `Math.trunc` of a finite number; `None` otherwise.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a float-to-int cast saturates, which is the wanted clamp"
)]
fn integer(value: f64) -> Option<i64> {
    value.is_finite().then(|| value.trunc() as i64)
}

/// JavaScript `Number(value)` for a JSON value (an array or object is not a number here).
fn js_number(value: &JsonValue) -> f64 {
    match value {
        JsonValue::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        JsonValue::String(text) if text.trim().is_empty() => 0.0,
        JsonValue::String(text) => text.trim().parse().unwrap_or(f64::NAN),
        JsonValue::Bool(flag) => f64::from(u8::from(*flag)),
        JsonValue::Null => 0.0,
        JsonValue::Array(_) | JsonValue::Object(_) => f64::NAN,
    }
}

/// JavaScript `String(value)` for a JSON value.
fn js_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) => match number.as_f64().and_then(integer) {
            Some(whole) if number.as_f64().is_some_and(|float| float.fract() == 0.0) => whole.to_string(),
            _ => number.to_string(),
        },
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Null => "null".to_owned(),
        JsonValue::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        JsonValue::Object(_) => "[object Object]".to_owned(),
    }
}
