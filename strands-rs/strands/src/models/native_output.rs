//! Structured output enforcement for model providers: mechanism order, learned support and native
//! output schemas (TS `models/native-output.ts`, internal).
//!
//! What a model rejects is learned from its errors and kept without eviction: support is a property
//! of the model, and a stale entry costs at most a weaker mechanism. TS keeps it process-wide; here
//! each provider owns a [`StructuredOutputSupport`], because the crate keeps no process-global state
//! other than warn-once sets.
#![cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use serde_json::Map;

use crate::error::{DataError, Result};
use crate::logging::{WARNED, warn_once_in};
use crate::models::model_info::ModelInfo;
use crate::models::strict_schema::child_schemas;
use crate::types::json::{JsonValue, to_string};
use crate::types::tools::ToolChoice;

/// A way a provider can enforce a structured output request, strongest first (TS
/// `StructuredOutputMechanism`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StructuredOutputMechanism {
    /// Force a call to the structured output tool.
    Forced,
    /// The provider's native structured output.
    Native,
    /// Auto tool choice: best effort.
    Auto,
}

impl StructuredOutputMechanism {
    const ALL: [Self; 3] = [Self::Forced, Self::Native, Self::Auto];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Forced => "forced",
            Self::Native => "native",
            Self::Auto => "auto",
        }
    }
}

/// Error substrings meaning the model can't be forced to call a tool (TS
/// `FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES`).
pub(crate) const FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES: [&str; 2] = [
    "tool_choice: type \"tool\" and \"any\" are not supported",
    "doesn't support the toolConfig.toolChoice",
];

/// Error substrings meaning the model has no native structured output (TS
/// `NATIVE_OUTPUT_UNSUPPORTED_MESSAGES`).
pub(crate) const NATIVE_OUTPUT_UNSUPPORTED_MESSAGES: [&str; 2] = [
    "doesn't support the outputConfig field",
    "output_config.format: Extra inputs are not permitted",
];

/// Error substrings meaning native structured output can't express the schema or request (TS
/// `NATIVE_OUTPUT_REJECTED_MESSAGES`).
pub(crate) const NATIVE_OUTPUT_REJECTED_MESSAGES: [&str; 4] = [
    "output_config.format.schema",
    "Schemas contains too many",
    "Schema is too complex",
    "Citations cannot be enabled when output format is set",
];

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

// A schema node needs one of these to be representable; without them it accepts any value.
const SCHEMA_TYPE_KEYS: [&str; 5] = ["type", "anyOf", "oneOf", "allOf", "$ref"];

/// Structured output support learned from model errors, keyed by provider and model id (the TS
/// module-level `UNSUPPORTED_MECHANISMS` and `REJECTED_NATIVE_SCHEMAS`). Rejected native schemas
/// are keyed by their JSON text, where TS keys by Zod schema identity.
#[derive(Debug, Default)]
pub(crate) struct StructuredOutputSupport {
    unsupported: Mutex<HashMap<String, HashSet<StructuredOutputMechanism>>>,
    rejected_native_schemas: Mutex<HashMap<String, HashSet<String>>>,
}

fn model_key(provider: &str, model_id: &str) -> String {
    format!("{provider}:{model_id}")
}

fn schema_key(schema: &JsonValue) -> String {
    to_string(schema).unwrap_or_default()
}

impl StructuredOutputSupport {
    /// The mechanisms to try for a structured output request, strongest first, skipping those the
    /// model is known or learned to reject (TS `structuredOutputMechanisms`). Native output is
    /// skipped for a schema it can't represent. Never empty: auto is always available.
    pub(crate) fn structured_output_mechanisms(
        &self,
        provider: &str,
        model_id: &str,
        tool_choice: Option<&ToolChoice>,
        schema: &JsonValue,
        model_info: &ModelInfo,
    ) -> Vec<StructuredOutputMechanism> {
        self.structured_output_mechanisms_in(&WARNED, provider, model_id, tool_choice, schema, model_info)
    }

    /// [`Self::structured_output_mechanisms`], warning once per `warned` set.
    fn structured_output_mechanisms_in(
        &self,
        warned: &Mutex<HashSet<String>>,
        provider: &str,
        model_id: &str,
        tool_choice: Option<&ToolChoice>,
        schema: &JsonValue,
        model_info: &ModelInfo,
    ) -> Vec<StructuredOutputMechanism> {
        let mut unsupported = self.unsupported_mechanisms(provider, model_id, schema);
        let capabilities = model_info.capabilities.unwrap_or_default();
        if capabilities.forced_tool_choice == Some(false) {
            unsupported.insert(StructuredOutputMechanism::Forced);
        }
        if capabilities.native_structured_output == Some(false) {
            unsupported.insert(StructuredOutputMechanism::Native);
        }
        let forces_tool = matches!(tool_choice, Some(ToolChoice::Any {} | ToolChoice::Tool { .. }));
        let feature = if unsupported.contains(&StructuredOutputMechanism::Native) {
            None
        } else {
            find_unsupported_native_output_feature(schema)
        };
        if let Some(feature) = feature {
            let message = format!(
                "feature=<{feature}> | native structured output cannot represent the schema, if the model cannot be forced to call the structured output tool structured output falls back to auto tool choice"
            );
            warn_once_in(warned, &message, |message| tracing::warn!("{message}"));
        }
        StructuredOutputMechanism::ALL
            .into_iter()
            .filter(|mechanism| !unsupported.contains(mechanism))
            .filter(|mechanism| *mechanism != StructuredOutputMechanism::Forced || forces_tool)
            .filter(|mechanism| *mechanism != StructuredOutputMechanism::Native || feature.is_none())
            .collect()
    }

    /// Records a provider error for a structured output request (TS `handleRejectedMechanism`).
    /// Returns `true` if the rejection was recorded and the next mechanism should be tried, `false`
    /// if the error is unrelated and should surface unchanged.
    pub(crate) fn handle_rejected_mechanism(
        &self,
        provider: &str,
        model_id: &str,
        mechanism: StructuredOutputMechanism,
        schema: &JsonValue,
        message: &str,
    ) -> bool {
        let key = model_key(provider, model_id);
        let contains_any = |signals: &[&str]| signals.iter().any(|signal| message.contains(signal));
        if mechanism == StructuredOutputMechanism::Native && contains_any(&NATIVE_OUTPUT_REJECTED_MESSAGES) {
            self.rejected_native_schemas
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
        let signals: &[&str] = match mechanism {
            StructuredOutputMechanism::Forced => &FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES,
            StructuredOutputMechanism::Native => &NATIVE_OUTPUT_UNSUPPORTED_MESSAGES,
            StructuredOutputMechanism::Auto => &[],
        };
        if !contains_any(signals) {
            return false;
        }
        self.unsupported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_default()
            .insert(mechanism);
        let mechanism = mechanism.as_str();
        tracing::debug!(
            "provider=<{provider}>, model_id=<{model_id}>, mechanism=<{mechanism}> | model rejects structured output mechanism"
        );
        true
    }

    /// Forgets everything learned (TS `clearStructuredOutputSupport`).
    pub(crate) fn clear(&self) {
        self.unsupported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.rejected_native_schemas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
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
            .rejected_native_schemas
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

/// The first schema feature native structured output can't represent without losing data: tuple
/// fields, record fields, or untyped fields (TS `findUnsupportedNativeOutputFeature`).
pub(crate) fn find_unsupported_native_output_feature(schema: &JsonValue) -> Option<&'static str> {
    match schema {
        JsonValue::Object(map) => find_unsupported_feature(map),
        _ => Some("untyped fields"),
    }
}

fn find_unsupported_feature(schema: &Map<String, JsonValue>) -> Option<&'static str> {
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

/// Converts a JSON schema to the subset native structured output accepts, as the Anthropic SDK's
/// `transformJSONSchema` does (TS `toNativeOutputSchema`): objects are closed and keywords outside
/// the subset move into the field description.
///
/// # Errors
///
/// Fails if a schema node has no `type`, `anyOf`, `oneOf`, `allOf` or `$ref`.
pub(crate) fn to_native_output_schema(schema: &JsonValue) -> Result<JsonValue> {
    convert_node(schema).map(JsonValue::Object)
}

fn convert_node(schema: &JsonValue) -> Result<Map<String, JsonValue>> {
    let mut remaining = schema.as_object().cloned().unwrap_or_default();
    let mut converted = Map::new();

    if let Some(JsonValue::Object(defs)) = remaining.shift_remove("$defs") {
        let defs = defs
            .iter()
            .map(|(name, definition)| Ok((name.clone(), JsonValue::Object(convert_node(definition)?))))
            .collect::<Result<Map<_, _>>>()?;
        converted.insert("$defs".to_owned(), JsonValue::Object(defs));
    }
    if let Some(reference) = remaining.shift_remove("$ref") {
        converted.insert("$ref".to_owned(), reference);
        return Ok(converted);
    }

    let kind = remaining.shift_remove("type");
    convert_composition(&mut remaining, &mut converted, kind.as_ref())?;
    for key in ["enum", "description", "title"] {
        if let Some(value) = remaining.shift_remove(key) {
            converted.insert(key.to_owned(), value);
        }
    }
    match kind.as_ref().and_then(JsonValue::as_str) {
        Some("object") => convert_object(&mut remaining, &mut converted)?,
        Some("string") => convert_string(&mut remaining, &mut converted),
        Some("array") => convert_array(&mut remaining, &mut converted)?,
        _ => {}
    }

    if !remaining.is_empty() {
        let leftover = remaining
            .iter()
            .map(|(key, value)| format!("{key}: {}", to_string(value).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", ");
        let unsupported = format!("{{{leftover}}}");
        let description = match converted.get("description") {
            Some(JsonValue::String(description)) => format!("{description}\n\n{unsupported}"),
            Some(description) => format!("{}\n\n{unsupported}", to_string(description).unwrap_or_default()),
            None => unsupported,
        };
        converted.insert("description".to_owned(), JsonValue::String(description));
    }
    Ok(converted)
}

fn convert_variants(variants: &[JsonValue]) -> Result<JsonValue> {
    variants
        .iter()
        .map(|variant| convert_node(variant).map(JsonValue::Object))
        .collect::<Result<Vec<_>>>()
        .map(JsonValue::Array)
}

fn convert_composition(
    remaining: &mut Map<String, JsonValue>,
    converted: &mut Map<String, JsonValue>,
    kind: Option<&JsonValue>,
) -> Result<()> {
    let any_of = remaining.shift_remove("anyOf");
    let one_of = remaining.shift_remove("oneOf");
    let all_of = remaining.shift_remove("allOf");
    if let Some(JsonValue::Array(variants)) = &any_of {
        converted.insert("anyOf".to_owned(), convert_variants(variants)?);
    } else if let Some(JsonValue::Array(variants)) = &one_of {
        converted.insert("anyOf".to_owned(), convert_variants(variants)?);
    } else if let Some(JsonValue::Array(variants)) = &all_of {
        converted.insert("allOf".to_owned(), convert_variants(variants)?);
    } else if let Some(kind) = kind {
        converted.insert("type".to_owned(), kind.clone());
    } else {
        return Err(DataError::new("Schema must have a 'type', 'anyOf', 'oneOf', or 'allOf' field.").into());
    }
    Ok(())
}

fn convert_object(
    remaining: &mut Map<String, JsonValue>,
    converted: &mut Map<String, JsonValue>,
) -> Result<()> {
    let properties = match remaining.shift_remove("properties") {
        Some(JsonValue::Object(properties)) => properties
            .iter()
            .map(|(name, property)| Ok((name.clone(), JsonValue::Object(convert_node(property)?))))
            .collect::<Result<Map<_, _>>>()?,
        _ => Map::new(),
    };
    converted.insert("properties".to_owned(), JsonValue::Object(properties));
    remaining.shift_remove("additionalProperties");
    converted.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
    if let Some(required) = remaining.shift_remove("required") {
        converted.insert("required".to_owned(), required);
    }
    Ok(())
}

fn convert_string(remaining: &mut Map<String, JsonValue>, converted: &mut Map<String, JsonValue>) {
    match remaining.shift_remove("format") {
        Some(JsonValue::String(format)) if SUPPORTED_STRING_FORMATS.contains(&format.as_str()) => {
            converted.insert("format".to_owned(), JsonValue::String(format));
        }
        Some(format) => {
            remaining.insert("format".to_owned(), format);
        }
        None => {}
    }
}

fn convert_array(
    remaining: &mut Map<String, JsonValue>,
    converted: &mut Map<String, JsonValue>,
) -> Result<()> {
    match remaining.shift_remove("items") {
        Some(items @ JsonValue::Object(_)) => {
            converted.insert("items".to_owned(), JsonValue::Object(convert_node(&items)?));
        }
        Some(items) => {
            remaining.insert("items".to_owned(), items);
        }
        None => {}
    }
    match remaining.shift_remove("minItems") {
        Some(min_items) if min_items == 0 || min_items == 1 => {
            converted.insert("minItems".to_owned(), min_items);
        }
        Some(min_items) => {
            remaining.insert("minItems".to_owned(), min_items);
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
