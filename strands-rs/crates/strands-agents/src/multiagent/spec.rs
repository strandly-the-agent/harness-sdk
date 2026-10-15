//! Multi-agent specification and resolution (`strands-ts/src/multiagent/spec.ts`).
//!
//! The authority-mode system for model-driven multi-agent patterns, where the model configures
//! child agents at runtime. The developer sets axis policies ([`ScalarAxis`], [`ListAxis`]) that
//! shape the parameters the model sees and govern what values it can supply; [`Preset`]s name
//! partially applied roles. [`resolve_spec`] merges model-supplied arguments, preset defaults,
//! and axis policies into an [`AgentSpec`] used to build child agents.
//!
//! An `Inherit` axis resolves to `None` (meaning "inherit all") and contributes no parameter.

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Map;

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::models::Model;
use crate::types::JsonValue;

/// Turns a resolved spec into a child agent, built the way the parent was (TS `AgentBuilder`).
pub type AgentBuilder = Arc<dyn Fn(AgentSpec) -> Result<Agent> + Send + Sync>;

/// The model a spec selects: an id the builder resolves, or an already-built model.
#[derive(Clone)]
pub enum SpecModel {
    /// Provider model id (TS `model: string`).
    Id(String),
    /// A model instance to hand to the child as-is.
    Model(Arc<dyn Model>),
}

impl From<String> for SpecModel {
    fn from(id: String) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for SpecModel {
    fn from(id: &str) -> Self {
        Self::Id(id.to_owned())
    }
}

impl From<Arc<dyn Model>> for SpecModel {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::Model(model)
    }
}

/// Model instances compare by identity, mirroring TS reference equality.
impl PartialEq for SpecModel {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Id(left), Self::Id(right)) => left == right,
            (Self::Model(left), Self::Model(right)) => {
                std::ptr::eq(Arc::as_ptr(left).cast::<()>(), Arc::as_ptr(right).cast::<()>())
            }
            _ => false,
        }
    }
}

impl fmt::Debug for SpecModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => f.debug_tuple("Id").field(id).finish(),
            Self::Model(_) => f.write_str("Model(..)"),
        }
    }
}

/// A selectable entry in a [`Choice`] (TS `Option`, renamed to avoid clashing with [`Option`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption<T = String> {
    /// The name the model picks by.
    pub name: String,
    /// Value the name maps to; `None` is TS `UNSET` and is filled from `name` on normalization.
    pub value: Option<T>,
    /// Model-facing description, folded into the axis schema.
    pub description: String,
}

impl<T> ChoiceOption<T> {
    /// Option whose value is filled from its name (TS `new Option(name)`).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
            description: String::new(),
        }
    }

    /// Map this option's name onto `value`.
    pub fn with_value(mut self, value: impl Into<T>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Describe this option for the model.
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

/// The model picks from a developer-supplied set; `multiple` lets it pick more than one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice<T = String> {
    /// Entries the model can pick from.
    pub options: Vec<ChoiceOption<T>>,
    /// When `true`, the axis produces an array enum allowing multiple selections.
    pub multiple: bool,
}

impl<T> Choice<T> {
    /// Single-select choice over `options` (bare names or [`ChoiceOption`]s).
    pub fn new<O: Into<ChoiceOption<T>>>(options: impl IntoIterator<Item = O>) -> Self {
        Self {
            options: options.into_iter().map(Into::into).collect(),
            multiple: false,
        }
    }

    /// Let the model pick more than one entry.
    pub fn multiple(mut self) -> Self {
        self.multiple = true;
        self
    }
}

impl<T: Clone + From<String>> Choice<T> {
    /// The options with every `None` (TS `UNSET`) value filled from its name.
    pub fn normalized(&self) -> Vec<ChoiceOption<T>> {
        self.options
            .iter()
            .map(|option| match option.value {
                Some(_) => option.clone(),
                None => ChoiceOption {
                    name: option.name.clone(),
                    value: Some(T::from(option.name.clone())),
                    description: option.description.clone(),
                },
            })
            .collect()
    }

    /// Renders this axis as a JSON Schema property for a tool's `inputSchema`.
    ///
    /// Produces a `string` enum (or an `array` of enum when `multiple`), with per-option
    /// descriptions folded into the property's `description` after `description`.
    pub fn to_schema_property(&self, description: &str) -> Map<String, JsonValue> {
        let names: Vec<JsonValue> = self
            .options
            .iter()
            .map(|option| option.name.clone().into())
            .collect();
        let lines: Vec<String> = self
            .options
            .iter()
            .filter(|option| !option.description.is_empty())
            .map(|option| format!("- {}: {}", option.name, option.description))
            .collect();

        let mut desc = description.to_owned();
        if !lines.is_empty() {
            let block = format!("Options:\n{}", lines.join("\n"));
            desc = if description.is_empty() {
                block
            } else {
                format!("{description}\n{block}")
            };
        }

        let mut property = Map::new();
        if self.multiple {
            property.insert("type".to_owned(), "array".into());
            property.insert(
                "items".to_owned(),
                JsonValue::Object(Map::from_iter([
                    ("type".to_owned(), "string".into()),
                    ("enum".to_owned(), names.into()),
                ])),
            );
        } else {
            property.insert("type".to_owned(), "string".into());
            property.insert("enum".to_owned(), names.into());
        }
        if !desc.is_empty() {
            property.insert("description".to_owned(), desc.into());
        }
        property
    }

    /// The value behind `name`, or `name` itself when no option matches (passthrough).
    pub fn value_for(&self, name: &str) -> T {
        self.options
            .iter()
            .find(|option| option.name == name)
            .and_then(|option| option.value.clone())
            .unwrap_or_else(|| T::from(name.to_owned()))
    }
}

/// Policy for a scalar axis: one model-facing parameter (TS `Open | Choice | Fixed | Inherit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarAxis<T = String> {
    /// The developer pins the value; the axis contributes no model-facing parameter.
    Fixed(Option<T>),
    /// The child takes the parent's value; the axis contributes no model-facing parameter.
    Inherit,
    /// The model writes the value freely; the axis contributes a string parameter.
    Open,
    /// The model picks from a developer-supplied set.
    Choice(Choice<T>),
}

impl<T> ScalarAxis<T> {
    /// Pin this axis to `value` (TS `new Fixed(value)`).
    pub fn fixed(value: impl Into<T>) -> Self {
        Self::Fixed(Some(value.into()))
    }
}

// `#[derive(Default)]` would add a spurious `T: Default` bound, which `SpecModel` cannot satisfy.
#[allow(clippy::derivable_impls)]
impl<T> Default for ScalarAxis<T> {
    fn default() -> Self {
        Self::Inherit
    }
}

/// Policy for a list axis: a multi-valued model-facing parameter (TS `Choice | Fixed | Inherit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListAxis<T = String> {
    /// The developer pins the list; the axis contributes no model-facing parameter.
    Fixed(Option<Vec<T>>),
    /// The child takes the parent's list; the axis contributes no model-facing parameter.
    Inherit,
    /// The model picks from a developer-supplied set.
    Choice(Choice<T>),
}

impl<T> ListAxis<T> {
    /// Pin this axis to `values` (TS `new Fixed([...])`).
    pub fn fixed(values: impl IntoIterator<Item = impl Into<T>>) -> Self {
        Self::Fixed(Some(values.into_iter().map(Into::into).collect()))
    }
}

// `#[derive(Default)]` would add a spurious `T: Default` bound, which `SpecModel` cannot satisfy.
#[allow(clippy::derivable_impls)]
impl<T> Default for ListAxis<T> {
    fn default() -> Self {
        Self::Inherit
    }
}

/// A named role: a partially applied child configuration selected via `agent_type`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preset {
    /// System prompt for the child.
    pub instructions: Option<String>,
    /// Tool names the child gets.
    pub tools: Option<Vec<String>>,
    /// Model the child runs on.
    pub model: Option<SpecModel>,
    /// Model-facing description of the role.
    pub description: String,
}

impl Preset {
    /// Set the preset's system prompt.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Set the preset's tool names.
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    /// Set the preset's model.
    pub fn with_model(mut self, model: impl Into<SpecModel>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Describe the role for the model.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Resolved child configuration handed to an [`AgentBuilder`]. `None` means "inherit".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSpec {
    /// Name for the child agent.
    pub name: Option<String>,
    /// Preset the child was resolved from.
    pub agent_type: Option<String>,
    /// System prompt for the child.
    pub instructions: Option<String>,
    /// Tool names the child gets.
    pub tools: Option<Vec<String>>,
    /// MCP server names the child connects to.
    pub mcp_servers: Option<Vec<String>>,
    /// Model the child runs on.
    pub model: Option<SpecModel>,
}

/// Axis policies, presets, and the default preset handed to [`resolve_spec`].
///
/// Every axis defaults to `Inherit`, contributing no model-facing parameter.
#[derive(Clone, Debug, Default)]
pub struct ResolveSpecAxes {
    /// Named roles selectable via `agent_type`.
    pub presets: IndexMap<String, Preset>,
    /// Preset applied when the model supplies no `agent_type`.
    pub default_preset: Option<String>,
    /// Policy for the child's system prompt.
    pub instructions: ScalarAxis<String>,
    /// Policy for the child's tool selection.
    pub tools: ListAxis<String>,
    /// Policy for the child's MCP server selection.
    pub mcp_servers: ListAxis<String>,
    /// Policy for the child's model.
    pub model: ScalarAxis<SpecModel>,
}

/// Resolves a scalar axis: model-supplied value → preset → axis default.
fn resolve_scalar<T: Clone + From<String>>(
    model_value: Option<&JsonValue>,
    axis: &ScalarAxis<T>,
    preset_value: Option<&T>,
) -> Option<T> {
    if let Some(value) = model_value.and_then(JsonValue::as_str) {
        match axis {
            ScalarAxis::Open => return Some(T::from(value.to_owned())),
            ScalarAxis::Choice(choice) if choice.options.iter().any(|option| option.name == value) => {
                return Some(choice.value_for(value))
            }
            _ => {}
        }
    }
    if let Some(value) = preset_value {
        return Some(value.clone());
    }
    match axis {
        ScalarAxis::Fixed(value) => value.clone(),
        _ => None,
    }
}

/// Resolves a list axis: model-supplied value → preset → axis default → `None` (inherit).
///
/// A [`ListAxis::Choice`] drops values that are not valid options.
fn resolve_list<T: Clone + From<String> + PartialEq>(
    model_value: Option<&JsonValue>,
    axis: &ListAxis<T>,
    preset_values: Option<&[T]>,
) -> Option<Vec<T>> {
    let choice = match axis {
        ListAxis::Choice(choice) => Some(choice),
        _ => None,
    };

    if let (Some(value), Some(choice)) = (model_value, choice) {
        let requested: Vec<&str> = match value {
            JsonValue::String(name) => vec![name.as_str()],
            JsonValue::Array(items) => items.iter().filter_map(JsonValue::as_str).collect(),
            _ => Vec::new(),
        };
        return Some(
            requested
                .into_iter()
                .filter(|name| choice.options.iter().any(|option| option.name == *name))
                .map(|name| choice.value_for(name))
                .collect(),
        );
    }

    if let Some(values) = preset_values {
        return Some(match choice {
            Some(choice) => {
                let allowed: Vec<T> = choice
                    .normalized()
                    .into_iter()
                    .filter_map(|option| option.value)
                    .collect();
                values
                    .iter()
                    .filter(|value| allowed.contains(value))
                    .cloned()
                    .collect()
            }
            None => values.to_vec(),
        });
    }

    match axis {
        ListAxis::Choice(choice) if choice.multiple => Some(
            choice
                .normalized()
                .into_iter()
                .filter_map(|option| option.value)
                .collect(),
        ),
        ListAxis::Fixed(values) => values.clone(),
        _ => None,
    }
}

/// Combines the model's arguments, the selected preset, and the axis policies into a spec.
///
/// Precedence per axis: a model-supplied argument wins, then the preset's value, then the axis
/// default. An omitted `agent_type` falls back to `default_preset`, so a bare model input behaves
/// like the default role; ad-hoc `instructions` suppress that fallback.
///
/// # Errors
/// Returns [`Error::ToolValidation`] when `agent_type` is present but is not a known preset name.
pub fn resolve_spec(model_input: &Map<String, JsonValue>, axes: &ResolveSpecAxes) -> Result<AgentSpec> {
    // agent_type is a closed enum: a provided value must be an exact preset name. A null falls
    // back to the default preset, matching TS's `!= null` check.
    let requested_type = model_input.get("agent_type").filter(|value| !value.is_null());
    let known_type = requested_type
        .and_then(JsonValue::as_str)
        .filter(|name| axes.presets.contains_key(*name));
    if let (Some(requested), None) = (requested_type, known_type) {
        let mut valid: Vec<&str> = axes.presets.keys().map(String::as_str).collect();
        valid.sort_unstable();
        return Err(Error::ToolValidation(format!(
            "Unknown agent_type {requested}; valid values: {}.",
            valid.join(", ")
        )));
    }

    // Ad-hoc instructions (only possible when the axis exposes one) override the default preset.
    let overriding_instructions = matches!(axes.instructions, ScalarAxis::Open | ScalarAxis::Choice(_))
        && model_input.contains_key("instructions");

    let agent_type = match known_type {
        Some(name) => Some(name.to_owned()),
        None if !overriding_instructions => axes.default_preset.clone(),
        None => None,
    };
    let preset = agent_type.as_deref().and_then(|name| axes.presets.get(name));

    Ok(AgentSpec {
        name: model_input
            .get("name")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        agent_type,
        instructions: resolve_scalar(
            model_input.get("instructions"),
            &axes.instructions,
            preset.and_then(|preset| preset.instructions.as_ref()),
        ),
        tools: resolve_list(
            model_input.get("tools"),
            &axes.tools,
            preset.and_then(|preset| preset.tools.as_deref()),
        ),
        mcp_servers: resolve_list(model_input.get("mcp_servers"), &axes.mcp_servers, None),
        model: resolve_scalar(
            model_input.get("model"),
            &axes.model,
            preset.and_then(|preset| preset.model.as_ref()),
        ),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::MockMessageModel;

    /// Mirrors the TS test suite's shared `AXES`.
    fn axes() -> ResolveSpecAxes {
        ResolveSpecAxes {
            instructions: ScalarAxis::Open,
            tools: ListAxis::Choice(Choice::new(["read", "shell"]).multiple()),
            ..Default::default()
        }
    }

    fn input(value: JsonValue) -> Map<String, JsonValue> {
        value.as_object().expect("object").clone()
    }

    fn all_tools() -> Option<Vec<String>> {
        Some(vec!["read".to_owned(), "shell".to_owned()])
    }

    #[test]
    fn wraps_strings_and_fills_unset() {
        let choice: Choice = Choice::new([
            ChoiceOption::new("alias")
                .with_value("sentinel")
                .with_description("d"),
            ChoiceOption::new("bare"),
            ChoiceOption::from("raw"),
        ]);

        let options = choice.normalized();

        assert_eq!(options.len(), 3);
        assert_eq!(options[0].value.as_deref(), Some("sentinel"));
        assert_eq!(options[0].description, "d");
        assert_eq!(options[1].value.as_deref(), Some("bare"));
        assert_eq!(options[2].name, "raw");
        assert_eq!(options[2].value.as_deref(), Some("raw"));
    }

    /// TS guards an explicit `null`; Rust's only "unset" is `None`, so an explicitly set
    /// empty value stands in for the falsy-but-legitimate case.
    #[test]
    fn does_not_overwrite_an_explicitly_set_value() {
        let choice: Choice = Choice::new([ChoiceOption::new("off").with_value("")]);
        assert_eq!(choice.normalized()[0].value.as_deref(), Some(""));
    }

    #[test]
    fn produces_single_enum_and_array_enum() {
        let single: Choice = Choice::new(["a", "b"]);
        assert_eq!(
            JsonValue::Object(single.to_schema_property("")),
            json!({"type": "string", "enum": ["a", "b"]})
        );

        let property = Choice::<String>::new(["a"]).multiple().to_schema_property("desc");
        assert_eq!(property["type"], json!("array"));
        assert_eq!(property["items"], json!({"type": "string", "enum": ["a"]}));
        assert_eq!(property["description"], json!("desc"));
    }

    #[test]
    fn folds_option_descriptions() {
        let choice: Choice = Choice::new([
            ChoiceOption::new("x").with_description("info"),
            ChoiceOption::from("y"),
        ]);

        let description = choice.to_schema_property("")["description"]
            .as_str()
            .unwrap()
            .to_owned();

        assert!(description.contains("- x: info"), "{description}");
        assert!(!description.contains("y:"), "{description}");
    }

    #[test]
    fn returns_mapped_value_for_known_option_and_passthrough_for_unknown() {
        let choice: Choice = Choice::new([ChoiceOption::new("alias").with_value("real")]);
        assert_eq!(choice.value_for("alias"), "real");
        assert_eq!(choice.value_for("missing"), "missing");
    }

    #[test]
    fn applies_preset_by_default() {
        let mut axes = axes();
        axes.presets
            .insert("g".to_owned(), Preset::default().with_instructions("help"));
        axes.default_preset = Some("g".to_owned());

        assert_eq!(
            resolve_spec(&input(json!({"task": "x"})), &axes).unwrap(),
            AgentSpec {
                agent_type: Some("g".to_owned()),
                instructions: Some("help".to_owned()),
                tools: all_tools(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn throws_on_unknown_agent_type() {
        let mut axes = axes();
        axes.presets.insert("g".to_owned(), Preset::default());
        axes.default_preset = Some("g".to_owned());

        let error = resolve_spec(&input(json!({"task": "x", "agent_type": "nope"})), &axes).unwrap_err();

        assert!(
            error.to_string().contains("Unknown agent_type \"nope\""),
            "{error}"
        );
    }

    /// TS also rejects prototype keys; in Rust they are simply unknown preset names.
    #[test]
    fn rejects_unknown_and_non_string_agent_type_values() {
        let mut axes = axes();
        axes.presets.insert("g".to_owned(), Preset::default());
        axes.default_preset = Some("g".to_owned());

        for bad in [
            json!("constructor"),
            json!("toString"),
            json!("__proto__"),
            json!(["g"]),
        ] {
            let error = resolve_spec(&input(json!({"task": "x", "agent_type": bad})), &axes).unwrap_err();
            assert!(error.to_string().contains("Unknown agent_type"), "{error}");
        }

        // null falls back to the default preset instead of erroring.
        assert_eq!(
            resolve_spec(&input(json!({"task": "x", "agent_type": null})), &axes).unwrap(),
            AgentSpec {
                agent_type: Some("g".to_owned()),
                tools: all_tools(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn suppresses_default_preset_when_ad_hoc_instructions_provided() {
        let mut axes = axes();
        axes.presets
            .insert("g".to_owned(), Preset::default().with_instructions("default"));
        axes.default_preset = Some("g".to_owned());

        assert_eq!(
            resolve_spec(&input(json!({"task": "x", "instructions": "ad-hoc"})), &axes).unwrap(),
            AgentSpec {
                instructions: Some("ad-hoc".to_owned()),
                tools: all_tools(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn clamps_tools_to_allowed_set() {
        assert_eq!(
            resolve_spec(&input(json!({"task": "x", "tools": ["read", "write"]})), &axes()).unwrap(),
            AgentSpec {
                tools: Some(vec!["read".to_owned()]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn fixed_ignores_model_supplied_value() {
        let axes = ResolveSpecAxes {
            instructions: ScalarAxis::fixed("pinned"),
            ..axes()
        };

        assert_eq!(
            resolve_spec(&input(json!({"task": "x", "instructions": "override"})), &axes).unwrap(),
            AgentSpec {
                instructions: Some("pinned".to_owned()),
                tools: all_tools(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn choice_maps_option_value() {
        let sentinel: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        let axes = ResolveSpecAxes {
            model: ScalarAxis::Choice(Choice::new([
                ChoiceOption::new("smart").with_value(SpecModel::Model(sentinel.clone()))
            ])),
            ..axes()
        };

        let spec = resolve_spec(&input(json!({"task": "x", "model": "smart"})), &axes).unwrap();

        assert_eq!(spec.model, Some(SpecModel::Model(sentinel)));
    }

    #[test]
    fn clamps_preset_tools_to_choice_set() {
        let mut axes = axes();
        axes.presets.insert(
            "worker".to_owned(),
            Preset::default().with_tools(["read", "write"]),
        );
        axes.default_preset = Some("worker".to_owned());

        assert_eq!(
            resolve_spec(&input(json!({"task": "x"})), &axes).unwrap(),
            AgentSpec {
                agent_type: Some("worker".to_owned()),
                tools: Some(vec!["read".to_owned()]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn resolves_fixed_tools_list() {
        let axes = ResolveSpecAxes {
            tools: ListAxis::fixed(["a", "b"]),
            ..axes()
        };

        assert_eq!(
            resolve_spec(&input(json!({"task": "x"})), &axes).unwrap(),
            AgentSpec {
                tools: Some(vec!["a".to_owned(), "b".to_owned()]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn resolves_fixed_empty_list_to_empty_vec_not_none() {
        let axes = ResolveSpecAxes {
            mcp_servers: ListAxis::Fixed(Some(Vec::new())),
            ..axes()
        };

        assert_eq!(
            resolve_spec(&input(json!({"task": "x"})), &axes).unwrap(),
            AgentSpec {
                tools: all_tools(),
                mcp_servers: Some(Vec::new()),
                ..Default::default()
            }
        );
    }
}
