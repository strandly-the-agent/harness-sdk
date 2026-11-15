//! Recovery when a history edit invalidates replayed thinking blocks (TS `models/thinking-binding.ts`).

use crate::types::json::{JsonObject, JsonValue};

pub(crate) const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

const THINKING_BINDING_ERROR: &str = "bound to a different conversation";

pub(crate) fn is_thinking_binding_error(message: &str) -> bool {
    message.contains(THINKING_BINDING_ERROR)
}

/// `thinking` with `block_binding.prefix_mismatch_behavior: drop_block`, unless it isn't an object
/// or already sets a binding.
pub(crate) fn thinking_with_drop_block(thinking: Option<&JsonValue>) -> Option<JsonValue> {
    let thinking = thinking?.as_object()?;
    if thinking.contains_key("block_binding") {
        return None;
    }
    let mut thinking = thinking.clone();
    thinking.insert(
        "block_binding".to_owned(),
        serde_json::json!({ "prefix_mismatch_behavior": "drop_block" }),
    );
    Some(JsonValue::Object(thinking))
}

/// The request fields with `drop_block` thinking and the binding beta (TS `fieldsWithDropBlock`).
pub(crate) fn fields_with_drop_block(fields: Option<&JsonValue>) -> Option<JsonObject> {
    let fields = fields?.as_object()?;
    let thinking = thinking_with_drop_block(fields.get("thinking"))?;
    let mut betas = fields
        .get("anthropic_beta")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    if !betas.iter().any(|beta| beta == THINKING_BINDING_BETA) {
        betas.push(JsonValue::from(THINKING_BINDING_BETA));
    }
    let mut fields = fields.clone();
    fields.insert("anthropic_beta".to_owned(), JsonValue::Array(betas));
    fields.insert("thinking".to_owned(), thinking);
    Some(fields)
}
