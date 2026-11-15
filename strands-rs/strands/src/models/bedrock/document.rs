//! `serde_json::Value` to and from `aws_smithy_types::Document` (the SDK's serde bridge needs
//! `--cfg aws_sdk_unstable`).

use std::collections::HashMap;

use aws_smithy_types::{Document, Number};
use serde_json::Number as JsonNumber;

use crate::types::json::{JsonObject, JsonValue};

/// Non-negative integers become `PosInt`, negative integers `NegInt`, everything else `Float`.
pub(crate) fn json_to_document(value: &JsonValue) -> Document {
    match value {
        JsonValue::Null => Document::Null,
        JsonValue::Bool(flag) => Document::Bool(*flag),
        JsonValue::Number(number) => Document::Number(json_number_to_document(number)),
        JsonValue::String(text) => Document::String(text.clone()),
        JsonValue::Array(items) => Document::Array(items.iter().map(json_to_document).collect()),
        JsonValue::Object(map) => Document::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), json_to_document(value)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}

fn json_number_to_document(number: &JsonNumber) -> Number {
    if let Some(unsigned) = number.as_u64() {
        return Number::PosInt(unsigned);
    }
    if let Some(signed) = number.as_i64() {
        return Number::NegInt(signed);
    }
    Number::Float(number.as_f64().unwrap_or(f64::NAN))
}

/// The JSON form of `document`; a non-finite float becomes `null`, as `JSON.stringify` writes it.
pub(crate) fn document_to_json(document: &Document) -> JsonValue {
    match document {
        Document::Null => JsonValue::Null,
        Document::Bool(flag) => JsonValue::Bool(*flag),
        Document::Number(number) => document_number_to_json(*number),
        Document::String(text) => JsonValue::String(text.clone()),
        Document::Array(items) => JsonValue::Array(items.iter().map(document_to_json).collect()),
        Document::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            JsonValue::Object(
                keys.into_iter()
                    .filter_map(|key| map.get(key).map(|value| (key.clone(), document_to_json(value))))
                    .collect::<JsonObject>(),
            )
        }
    }
}

fn document_number_to_json(number: Number) -> JsonValue {
    match number {
        Number::PosInt(unsigned) => JsonValue::from(unsigned),
        Number::NegInt(signed) => JsonValue::from(signed),
        Number::Float(float) => JsonNumber::from_f64(float).map_or(JsonValue::Null, JsonValue::Number),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn maps_integers_by_sign_and_everything_else_to_float() {
        let cases = [
            (json!(0), Number::PosInt(0)),
            (json!(u64::MAX), Number::PosInt(u64::MAX)),
            (json!(-1), Number::NegInt(-1)),
            (json!(i64::MIN), Number::NegInt(i64::MIN)),
            (json!(1.5), Number::Float(1.5)),
            (json!(-2.25), Number::Float(-2.25)),
        ];
        for (value, number) in cases {
            assert_eq!(json_to_document(&value), Document::Number(number), "{value}");
            assert_eq!(document_to_json(&Document::Number(number)), value);
        }
    }

    #[test]
    fn keeps_negative_zero_as_a_float() {
        let Document::Number(Number::Float(zero)) = json_to_document(&json!(-0.0)) else {
            panic!("expected a float");
        };
        assert!(zero == 0.0 && zero.is_sign_negative());
        let back = document_to_json(&Document::Number(Number::Float(-0.0)));
        assert!(
            back.as_f64()
                .is_some_and(|value| value == 0.0 && value.is_sign_negative())
        );
    }

    #[test]
    fn writes_non_finite_floats_as_null() {
        assert_eq!(
            document_to_json(&Document::Number(Number::Float(f64::NAN))),
            JsonValue::Null
        );
        assert_eq!(
            document_to_json(&Document::Number(Number::Float(f64::INFINITY))),
            JsonValue::Null
        );
    }

    #[test]
    fn round_trips_nested_values() {
        let value = json!({ "a": [1, -2, 3.5, "x", null, true], "b": { "c": {} } });
        assert_eq!(document_to_json(&json_to_document(&value)), value);
    }

    #[test]
    fn a_cloned_document_keeps_its_iteration_order() {
        let value = json!({ "zeta": 1, "alpha": 2, "mid": 3, "beta": 4, "omega": 5, "kappa": 6 });
        let Document::Object(original) = json_to_document(&value) else {
            panic!("expected an object");
        };
        let cloned = original.clone();
        assert!(original.keys().eq(cloned.keys()));
    }
}
