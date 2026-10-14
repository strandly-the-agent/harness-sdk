//! `jsonReplacer` parity (`strands-ts/src/telemetry/json.ts`).

use serde::Serialize;
use serde_json::{Map, Value};

/// Keys whose values hold raw binary on the wire (TS `Uint8Array`/`ArrayBuffer`).
const BINARY_KEYS: [&str; 2] = ["bytes", "redactedContent"];

/// The literal TS replacement token for non-serializable values.
pub const REPLACED: &str = "<replaced>";

/// Serialize `value` the way TS does with `jsonReplacer`: binary payloads become
/// `"<replaced>"`, everything else passes through unchanged.
pub fn to_telemetry_json<T: Serialize + ?Sized>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(mut v) => {
            replace_binary(&mut v);
            v.to_string()
        }
        Err(_) => Value::String(REPLACED.to_string()).to_string(),
    }
}

fn replace_binary(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if BINARY_KEYS.contains(&key.as_str()) {
                    map.insert(key, Value::String(REPLACED.to_string()));
                } else if let Some(child) = map.get_mut(&key) {
                    replace_binary(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(replace_binary),
        _ => {}
    }
}

/// `{ "content": <json> }` style event payload builder used by the tracer.
pub(crate) fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, ImageBlock, ImageFormat, ImageSource, Message};

    #[test]
    fn passes_through_plain_values() {
        let message = Message::user("hi");
        assert!(to_telemetry_json(&message.content).contains("hi"));
    }

    #[test]
    fn replaces_binary_payloads() {
        let block = ContentBlock::Image(ImageBlock {
            format: ImageFormat::Png,
            source: ImageSource::Bytes(vec![1, 2, 3]),
        });
        let json = to_telemetry_json(&block);
        assert!(json.contains("<replaced>"), "{json}");
        assert!(!json.contains("AQID"), "{json}");
    }
}
