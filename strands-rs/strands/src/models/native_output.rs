//! Native structured output support (TS `models/native-output.ts`).
//!
//! Decides which structured output mechanisms a model call may use, strongest first, remembers
//! the mechanisms a model rejected, and converts schemas to the subset native output accepts.
#![cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use crate::error::ModelError;
use crate::logging::warn_once;
use crate::models::model_info::ModelInfo;
use crate::models::strict_schema::child_schemas;
use crate::tools::ToolChoice;
use crate::types::json::{self, JsonValue};

type SchemaNode = serde_json::Map<String, JsonValue>;

/// How a call enforces a structured output schema (TS `StructuredOutputMechanism`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StructuredOutputMechanism {
    /// A forced call to the structured output tool.
    Forced,
    /// The provider's native structured output.
    Native,
    /// The structured output tool with automatic tool choice.
    Auto,
}

impl StructuredOutputMechanism {
    const ALL: [Self; 3] = [Self::Forced, Self::Native, Self::Auto];
}

const SUPPORTED_STRING_FORMATS: [&str; 10] = [
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

const SCHEMA_TYPE_KEYS: [&str; 5] = ["type", "anyOf", "oneOf", "allOf", "$ref"];

/// Error text of a model that can't be forced to call a tool.
pub(crate) const FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES: [&str; 2] = [
    "tool_choice: type \"tool\" and \"any\" are not supported",
    "doesn't support the toolConfig.toolChoice",
];

/// Error text of a model without native structured output.
pub(crate) const NATIVE_OUTPUT_UNSUPPORTED_MESSAGES: [&str; 2] = [
    "doesn't support the outputConfig field",
    "output_config.format: Extra inputs are not permitted",
];

/// Error text of native structured output rejecting one schema.
pub(crate) const NATIVE_OUTPUT_REJECTED_MESSAGES: [&str; 4] = [
    "output_config.format.schema",
    "Schemas contains too many",
    "Schema is too complex",
    "Citations cannot be enabled when output format is set",
];

/// What the models of one provider were learned to reject.
///
/// TS keeps this in module-level maps keyed by `provider:modelId`; here each provider owns one,
/// so there is no process-global state. Schemas are identified by their JSON.
#[derive(Debug, Default)]
pub(crate) struct StructuredOutputSupport {
    unsupported: Mutex<HashMap<String, HashSet<StructuredOutputMechanism>>>,
    rejected_schemas: Mutex<HashMap<String, HashSet<String>>>,
}

fn model_key(provider: &str, model_id: &str) -> String {
    format!("{provider}:{model_id}")
}

fn schema_key(schema: &JsonValue) -> String {
    schema.to_string()
}

impl StructuredOutputSupport {
    /// The mechanisms the call may use, strongest first (TS `structuredOutputMechanisms`).
    pub(crate) fn structured_output_mechanisms(
        &self,
        provider: &str,
        model_id: &str,
        tool_choice: Option<&ToolChoice>,
        schema: &JsonValue,
        model_info: &ModelInfo,
    ) -> Vec<StructuredOutputMechanism> {
        let mut unsupported = self.unsupported_mechanisms(provider, model_id, schema);
        let capabilities = model_info.capabilities();
        if capabilities.forced_tool_choice == Some(false) {
            unsupported.insert(StructuredOutputMechanism::Forced);
        }
        if capabilities.native_structured_output == Some(false) {
            unsupported.insert(StructuredOutputMechanism::Native);
        }
        let forces_tool = matches!(
            tool_choice,
            Some(ToolChoice::Any { .. } | ToolChoice::Tool { .. })
        );
        let feature = if unsupported.contains(&StructuredOutputMechanism::Native) {
            None
        } else {
            find_unsupported_native_output_feature(schema)
        };
        if let Some(feature) = feature {
            warn_once(&format!(
                "feature=<{feature}> | native structured output cannot represent the schema, if the model cannot be forced to call the structured output tool structured output falls back to auto tool choice"
            ));
        }
        StructuredOutputMechanism::ALL
            .into_iter()
            .filter(|mechanism| {
                !unsupported.contains(mechanism)
                    && (*mechanism != StructuredOutputMechanism::Forced || forces_tool)
                    && (*mechanism != StructuredOutputMechanism::Native || feature.is_none())
            })
            .collect()
    }

    /// Records a rejection the error `message` shows, returning whether it was one (TS
    /// `handleRejectedMechanism`).
    pub(crate) fn handle_rejected_mechanism(
        &self,
        provider: &str,
        model_id: &str,
        mechanism: StructuredOutputMechanism,
        schema: &JsonValue,
        message: &str,
    ) -> bool {
        let key = model_key(provider, model_id);
        if mechanism == StructuredOutputMechanism::Native
            && NATIVE_OUTPUT_REJECTED_MESSAGES
                .iter()
                .any(|signal| message.contains(signal))
        {
            self.rejected_schemas
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(key)
                .or_default()
                .insert(schema_key(schema));
            tracing::warn!(
                "provider=<{provider}>, model_id=<{model_id}>, error=<{message}> | native structured output rejected the schema, using auto tool choice instead"
            );
            return true;
        }
        if !is_unsupported_mechanism_message(message, mechanism) {
            return false;
        }
        self.unsupported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_default()
            .insert(mechanism);
        tracing::debug!(
            "provider=<{provider}>, model_id=<{model_id}>, mechanism=<{mechanism:?}> | model rejects structured output mechanism"
        );
        true
    }

    fn unsupported_mechanisms(
        &self,
        provider: &str,
        model_id: &str,
        schema: &JsonValue,
    ) -> HashSet<StructuredOutputMechanism> {
        let key = model_key(provider, model_id);
        let mut unsupported = self
            .unsupported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let rejected = self
            .rejected_schemas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .is_some_and(|schemas| schemas.contains(&schema_key(schema)));
        if rejected {
            unsupported.insert(StructuredOutputMechanism::Native);
        }
        unsupported
    }
}

fn is_unsupported_mechanism_message(message: &str, mechanism: StructuredOutputMechanism) -> bool {
    let signals: &[&str] = match mechanism {
        StructuredOutputMechanism::Forced => &FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES,
        StructuredOutputMechanism::Native => &NATIVE_OUTPUT_UNSUPPORTED_MESSAGES,
        StructuredOutputMechanism::Auto => &[],
    };
    signals.iter().any(|signal| message.contains(signal))
}

/// The first schema feature native output can't represent (TS `findUnsupportedNativeOutputFeature`).
pub(crate) fn find_unsupported_native_output_feature(schema: &JsonValue) -> Option<&'static str> {
    schema.as_object().and_then(find_unsupported_feature)
}

fn find_unsupported_feature(schema: &SchemaNode) -> Option<&'static str> {
    if schema.contains_key("prefixItems") {
        return Some("tuple fields");
    }
    let open = schema
        .get("additionalProperties")
        .is_some_and(|value| value != &JsonValue::Bool(false));
    if open || schema.contains_key("patternProperties") {
        return Some("record fields");
    }
    if !SCHEMA_TYPE_KEYS.iter().any(|key| schema.contains_key(*key)) {
        return Some("untyped fields");
    }
    child_schemas(schema)
        .into_iter()
        .find_map(find_unsupported_feature)
}

/// `schema` in the subset native output accepts; keywords it can't take move into the
/// description (TS `toNativeOutputSchema`).
///
/// # Errors
///
/// A node without `type`, `anyOf`, `oneOf` or `allOf` (and no `$ref`).
pub(crate) fn to_native_output_schema(schema: &JsonValue) -> Result<JsonValue, ModelError> {
    match schema {
        JsonValue::Object(node) => convert_node(node).map(JsonValue::Object),
        _ => Err(untyped()),
    }
}

fn untyped() -> ModelError {
    ModelError::new("Schema must have a 'type', 'anyOf', 'oneOf', or 'allOf' field.")
}

fn convert_value(value: &JsonValue) -> Result<JsonValue, ModelError> {
    match value {
        JsonValue::Object(node) => convert_node(node).map(JsonValue::Object),
        // TS converts whatever sits there as a node; a non-object has no type.
        _ => Err(untyped()),
    }
}

fn convert_map(map: &SchemaNode) -> Result<SchemaNode, ModelError> {
    map.iter()
        .map(|(name, node)| Ok((name.clone(), convert_value(node)?)))
        .collect()
}

fn convert_node(schema: &SchemaNode) -> Result<SchemaNode, ModelError> {
    let mut remaining = schema.clone();
    let mut converted = SchemaNode::new();
    if let Some(JsonValue::Object(defs)) = remaining.shift_remove("$defs") {
        converted.insert("$defs".into(), JsonValue::Object(convert_map(&defs)?));
    }
    if let Some(reference) = remaining.shift_remove("$ref") {
        converted.insert("$ref".into(), reference);
        return Ok(converted);
    }
    let kind = remaining.shift_remove("type");
    convert_composition(&mut remaining, &mut converted, kind.as_ref())?;
    for key in ["enum", "description", "title"] {
        if let Some(value) = remaining.shift_remove(key) {
            converted.insert(key.into(), value);
        }
    }
    match kind.as_ref().and_then(JsonValue::as_str) {
        Some("object") => convert_object(&mut remaining, &mut converted)?,
        Some("string") => convert_string(&mut remaining, &mut converted),
        Some("array") => convert_array(&mut remaining, &mut converted)?,
        _ => {}
    }
    if !remaining.is_empty() {
        let leftover: Vec<String> = remaining
            .iter()
            .map(|(key, value)| {
                let value = json::to_vec(value).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                format!("{key}: {}", value.unwrap_or_default())
            })
            .collect();
        let unsupported = format!("{{{}}}", leftover.join(", "));
        let description = match converted.get("description") {
            Some(JsonValue::String(description)) => format!("{description}\n\n{unsupported}"),
            Some(other) => format!("{other}\n\n{unsupported}"),
            None => unsupported,
        };
        converted.insert("description".into(), JsonValue::String(description));
    }
    Ok(converted)
}

fn convert_list(list: &[JsonValue]) -> Result<JsonValue, ModelError> {
    list.iter()
        .map(convert_value)
        .collect::<Result<Vec<_>, _>>()
        .map(JsonValue::Array)
}

fn convert_composition(
    remaining: &mut SchemaNode,
    converted: &mut SchemaNode,
    kind: Option<&JsonValue>,
) -> Result<(), ModelError> {
    let any_of = remaining.shift_remove("anyOf");
    let one_of = remaining.shift_remove("oneOf");
    let all_of = remaining.shift_remove("allOf");
    match (any_of, one_of, all_of) {
        (Some(JsonValue::Array(list)), _, _) | (_, Some(JsonValue::Array(list)), _) => {
            converted.insert("anyOf".into(), convert_list(&list)?);
        }
        (_, _, Some(JsonValue::Array(list))) => {
            converted.insert("allOf".into(), convert_list(&list)?);
        }
        _ => {
            let kind = kind.ok_or_else(untyped)?;
            converted.insert("type".into(), kind.clone());
        }
    }
    Ok(())
}

fn convert_object(remaining: &mut SchemaNode, converted: &mut SchemaNode) -> Result<(), ModelError> {
    let properties = match remaining.shift_remove("properties") {
        Some(JsonValue::Object(properties)) => convert_map(&properties)?,
        _ => SchemaNode::new(),
    };
    converted.insert("properties".into(), JsonValue::Object(properties));
    remaining.shift_remove("additionalProperties");
    converted.insert("additionalProperties".into(), JsonValue::Bool(false));
    if let Some(required) = remaining.shift_remove("required") {
        converted.insert("required".into(), required);
    }
    Ok(())
}

fn convert_string(remaining: &mut SchemaNode, converted: &mut SchemaNode) {
    match remaining.shift_remove("format") {
        Some(JsonValue::String(format)) if SUPPORTED_STRING_FORMATS.contains(&format.as_str()) => {
            converted.insert("format".into(), JsonValue::String(format));
        }
        Some(format) => {
            remaining.insert("format".into(), format);
        }
        None => {}
    }
}

fn convert_array(remaining: &mut SchemaNode, converted: &mut SchemaNode) -> Result<(), ModelError> {
    match remaining.shift_remove("items") {
        Some(JsonValue::Object(items)) => {
            converted.insert("items".into(), JsonValue::Object(convert_node(&items)?));
        }
        Some(items) => {
            remaining.insert("items".into(), items);
        }
        None => {}
    }
    match remaining.shift_remove("minItems") {
        Some(min) if min == 0 || min == 1 => {
            converted.insert("minItems".into(), min);
        }
        Some(min) => {
            remaining.insert("minItems".into(), min);
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
