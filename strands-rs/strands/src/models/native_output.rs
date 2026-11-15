//! Native structured output: which mechanism to use and the native schema form
//! (TS `models/native-output.ts`).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::{Error, Result};
use crate::logging::warn_once::warn_once;
use crate::models::model_info::ModelInfo;
use crate::models::strict_schema::child_schemas;
use crate::types::json::{JsonObject, JsonValue, to_js_string};
use crate::types::tools::ToolChoice;

/// How structured output is requested from a model (TS `StructuredOutputMechanism`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StructuredOutputMechanism {
    /// Force the structured output tool through tool choice.
    Forced,
    /// The provider's native structured output.
    Native,
    /// Offer the structured output tool with automatic tool choice.
    Auto,
}

impl StructuredOutputMechanism {
    /// The TS string form (`forced`, `native`, `auto`).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Forced => "forced",
            Self::Native => "native",
            Self::Auto => "auto",
        }
    }
}

const STRUCTURED_OUTPUT_MECHANISMS: [StructuredOutputMechanism; 3] = [
    StructuredOutputMechanism::Forced,
    StructuredOutputMechanism::Native,
    StructuredOutputMechanism::Auto,
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

const SCHEMA_TYPE_KEYS: [&str; 5] = ["type", "anyOf", "oneOf", "allOf", "$ref"];

// Providers report these cases only as a generic 400, so the message text is the only signal.
pub(crate) const FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES: [&str; 2] = [
    "tool_choice: type \"tool\" and \"any\" are not supported",
    "doesn't support the toolConfig.toolChoice",
];

pub(crate) const NATIVE_OUTPUT_UNSUPPORTED_MESSAGES: [&str; 2] = [
    "doesn't support the outputConfig field",
    "output_config.format: Extra inputs are not permitted",
];

pub(crate) const NATIVE_OUTPUT_REJECTED_MESSAGES: [&str; 4] = [
    "output_config.format.schema",
    "Schemas contains too many",
    "Schema is too complex",
    "Citations cannot be enabled when output format is set",
];

#[derive(Debug, Default)]
struct Learned {
    unsupported: HashMap<String, HashSet<StructuredOutputMechanism>>,
    rejected_schemas: HashMap<String, HashSet<String>>,
}

/// What models were learned to reject, keyed by provider and model id. A provider owns one, so the
/// learning lasts as long as the provider (TS keeps it process-wide).
#[allow(dead_code, reason = "used by model providers")]
#[derive(Debug, Default)]
pub(crate) struct StructuredOutputSupport {
    learned: Mutex<Learned>,
}

fn model_key(provider: &str, model_id: &str) -> String {
    format!("{provider}:{model_id}")
}

fn schema_key(schema: &JsonValue) -> String {
    to_js_string(schema).unwrap_or_default()
}

#[allow(dead_code, reason = "used by model providers")]
impl StructuredOutputSupport {
    fn learned(&self) -> MutexGuard<'_, Learned> {
        self.learned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The mechanisms to try, strongest first (TS `structuredOutputMechanisms`).
    pub(crate) fn mechanisms(
        &self,
        provider: &str,
        model_id: &str,
        tool_choice: Option<&ToolChoice>,
        schema: &JsonValue,
        model_info: &ModelInfo,
    ) -> Vec<StructuredOutputMechanism> {
        let mut unsupported = self.unsupported_mechanisms(provider, model_id, schema);
        let capabilities = model_info.capabilities.clone().unwrap_or_default();
        if capabilities.forced_tool_choice == Some(false) {
            unsupported.insert(StructuredOutputMechanism::Forced);
        }
        if capabilities.native_structured_output == Some(false) {
            unsupported.insert(StructuredOutputMechanism::Native);
        }
        let forces_tool = matches!(tool_choice, Some(ToolChoice::Any | ToolChoice::Tool { .. }));
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
        STRUCTURED_OUTPUT_MECHANISMS
            .into_iter()
            .filter(|mechanism| !unsupported.contains(mechanism))
            .filter(|mechanism| *mechanism != StructuredOutputMechanism::Forced || forces_tool)
            .filter(|mechanism| *mechanism != StructuredOutputMechanism::Native || feature.is_none())
            .collect()
    }

    /// Records what a provider error says about `mechanism`; true when the error was recognized
    /// (TS `handleRejectedMechanism`).
    pub(crate) fn handle_rejected_mechanism(
        &self,
        provider: &str,
        model_id: &str,
        mechanism: StructuredOutputMechanism,
        schema: &JsonValue,
        message: &str,
    ) -> bool {
        let key = model_key(provider, model_id);
        let rejected = NATIVE_OUTPUT_REJECTED_MESSAGES
            .iter()
            .any(|signal| message.contains(signal));
        if mechanism == StructuredOutputMechanism::Native && rejected {
            self.learned()
                .rejected_schemas
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
        self.learned()
            .unsupported
            .entry(key)
            .or_default()
            .insert(mechanism);
        tracing::debug!(
            "provider=<{provider}>, model_id=<{model_id}>, mechanism=<{}> | model rejects structured output mechanism",
            mechanism.as_str()
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
        let learned = self.learned();
        let mut unsupported = learned.unsupported.get(&key).cloned().unwrap_or_default();
        if learned
            .rejected_schemas
            .get(&key)
            .is_some_and(|schemas| schemas.contains(&schema_key(schema)))
        {
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

fn find_unsupported_feature(schema: &JsonObject) -> Option<&'static str> {
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

/// `schema` in the form native structured output accepts: closed objects, `oneOf` as `anyOf`, and
/// unsupported keywords moved into the description (TS `toNativeOutputSchema`).
///
/// # Errors
///
/// Fails for a node without a `type`, `anyOf`, `oneOf` or `allOf`.
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn to_native_output_schema(schema: &JsonValue) -> Result<JsonValue> {
    convert_node(schema).map(JsonValue::Object)
}

fn as_node(value: &JsonValue) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn convert_node(schema: &JsonValue) -> Result<JsonObject> {
    let mut remaining = as_node(schema);
    let mut converted = JsonObject::new();
    if let Some(JsonValue::Object(defs)) = remaining.shift_remove("$defs") {
        let defs = defs
            .iter()
            .map(|(name, definition)| Ok((name.clone(), JsonValue::Object(convert_node(definition)?))))
            .collect::<Result<JsonObject>>()?;
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
            .map(|(key, value)| format!("{key}: {}", to_js_string(value).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", ");
        let unsupported = format!("{{{leftover}}}");
        let description = match converted.get("description") {
            Some(JsonValue::String(description)) => format!("{description}\n\n{unsupported}"),
            Some(other) => format!("{}\n\n{unsupported}", to_js_string(other).unwrap_or_default()),
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
    remaining: &mut JsonObject,
    converted: &mut JsonObject,
    kind: Option<&JsonValue>,
) -> Result<()> {
    let any_of = remaining.shift_remove("anyOf");
    let one_of = remaining.shift_remove("oneOf");
    let all_of = remaining.shift_remove("allOf");
    let (key, variants) = match (any_of, one_of, all_of) {
        (Some(JsonValue::Array(variants)), _, _) | (_, Some(JsonValue::Array(variants)), _) => {
            ("anyOf", variants)
        }
        (_, _, Some(JsonValue::Array(variants))) => ("allOf", variants),
        _ => {
            let Some(kind) = kind else {
                return Err(Error::other(
                    "Schema must have a 'type', 'anyOf', 'oneOf', or 'allOf' field.",
                ));
            };
            converted.insert("type".to_owned(), kind.clone());
            return Ok(());
        }
    };
    converted.insert(key.to_owned(), convert_variants(&variants)?);
    Ok(())
}

fn convert_object(remaining: &mut JsonObject, converted: &mut JsonObject) -> Result<()> {
    let properties = match remaining.shift_remove("properties") {
        Some(JsonValue::Object(properties)) => properties,
        _ => JsonObject::new(),
    };
    let properties = properties
        .iter()
        .map(|(name, property)| Ok((name.clone(), JsonValue::Object(convert_node(property)?))))
        .collect::<Result<JsonObject>>()?;
    converted.insert("properties".to_owned(), JsonValue::Object(properties));
    remaining.shift_remove("additionalProperties");
    converted.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
    if let Some(required) = remaining.shift_remove("required") {
        converted.insert("required".to_owned(), required);
    }
    Ok(())
}

fn convert_string(remaining: &mut JsonObject, converted: &mut JsonObject) {
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

fn convert_array(remaining: &mut JsonObject, converted: &mut JsonObject) -> Result<()> {
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
        Some(min) if min == 0 || min == 1 => {
            converted.insert("minItems".to_owned(), min);
        }
        Some(min) => {
            remaining.insert("minItems".to_owned(), min);
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
