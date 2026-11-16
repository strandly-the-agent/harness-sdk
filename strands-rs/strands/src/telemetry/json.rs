//! Attribute JSON (TS `telemetry/json.ts` `jsonReplacer`).
//!
//! TS replaces values `JSON.stringify` can't write (`bigint`, functions, symbols, raw buffers) with
//! `"<replaced>"`. Rust values serialize through serde, where binary content is already base64 text
//! as in TS `toJSON()`, so the placeholder stands for a value whose serializer fails. The one raw
//! buffer TS writes, a guard-content image in the system prompt, gets the placeholder in
//! `tracer::system_part`.

use serde::Serialize;

/// TS `'<replaced>'`.
const REPLACED: &str = "\"<replaced>\"";

/// `JSON.stringify(value, jsonReplacer)`.
pub(crate) fn to_json<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| REPLACED.to_owned())
}

#[cfg(test)]
mod tests {
    //! Ports `telemetry/__tests__/json.test.ts`.

    use serde::Serializer;
    use serde_json::{Value, json};

    use super::*;

    fn round_trip<T: Serialize>(value: &T) -> Value {
        serde_json::from_str(&to_json(value)).expect("valid JSON")
    }

    #[test]
    fn serializes_strings() {
        assert_eq!(round_trip(&json!({"key": "value"})), json!({"key": "value"}));
    }

    #[test]
    fn serializes_numbers() {
        assert_eq!(
            round_trip(&json!({"int": 42, "float": 3.5})),
            json!({"int": 42, "float": 3.5})
        );
    }

    #[test]
    fn serializes_booleans() {
        assert_eq!(
            round_trip(&json!({"yes": true, "no": false})),
            json!({"yes": true, "no": false})
        );
    }

    #[test]
    fn serializes_null() {
        assert_eq!(round_trip(&json!({"value": null})), json!({"value": null}));
    }

    #[test]
    fn serializes_simple_objects() {
        assert_eq!(
            round_trip(&json!({"name": "test", "count": 5})),
            json!({"name": "test", "count": 5})
        );
    }

    #[test]
    fn serializes_arrays() {
        assert_eq!(round_trip(&json!([1, "two", true])), json!([1, "two", true]));
    }

    #[test]
    fn handles_date_objects() {
        let date: jiff::Timestamp = "2025-01-01T12:00:00Z".parse().expect("a timestamp");
        let text = crate::types::snapshot::iso_timestamp(date);
        assert_eq!(
            round_trip(&json!({"date": text})),
            json!({"date": "2025-01-01T12:00:00.000Z"})
        );
    }

    /// The Rust form of the TS `replaces ...` cases: a value serde can't write.
    #[test]
    fn replaces_functions() {
        struct Opaque;
        impl Serialize for Opaque {
            fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("not serializable"))
            }
        }
        assert_eq!(to_json(&Opaque), "\"<replaced>\"");
    }
}
