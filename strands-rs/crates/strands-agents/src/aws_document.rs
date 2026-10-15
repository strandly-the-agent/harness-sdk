//! Conversions between `serde_json::Value` and the smithy `Document` type, shared by the Bedrock
//! model provider and the Bedrock Knowledge Base memory store.

use aws_smithy_types::{Document, Number};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// `serde_json::Value` → smithy `Document`.
pub(crate) fn json_to_document(value: &Value) -> Result<Document> {
    Ok(match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            let number = if let Some(u) = n.as_u64() {
                Number::PosInt(u)
            } else if let Some(i) = n.as_i64() {
                Number::NegInt(i)
            } else if let Some(f) = n.as_f64() {
                Number::Float(f)
            } else {
                return Err(Error::model(format!("unsupported JSON number: {n}")));
            };
            Document::Number(number)
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(items) => {
            Document::Array(items.iter().map(json_to_document).collect::<Result<Vec<_>>>()?)
        }
        Value::Object(entries) => Document::Object(
            entries
                .iter()
                .map(|(key, value)| Ok((key.clone(), json_to_document(value)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

/// Smithy `Document` → `serde_json::Value`.
pub(crate) fn document_to_json(document: &Document) -> Value {
    match document {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(Number::PosInt(u)) => Value::from(*u),
        Document::Number(Number::NegInt(i)) => Value::from(*i),
        Document::Number(Number::Float(f)) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Document::String(s) => Value::String(s.clone()),
        Document::Array(items) => Value::Array(items.iter().map(document_to_json).collect()),
        Document::Object(entries) => {
            let mut map = Map::with_capacity(entries.len());
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort();
            for key in keys {
                map.insert(key.clone(), document_to_json(&entries[key]));
            }
            Value::Object(map)
        }
    }
}
