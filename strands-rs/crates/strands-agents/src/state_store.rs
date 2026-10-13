//! Key/value JSON state (`strands-ts/src/state-store.ts`). Every read and write copies, so
//! callers can never alias stored state.

use parking_lot::RwLock;
use serde_json::Map;

use crate::types::JsonValue;

/// Thread-safe JSON object store.
#[derive(Debug, Default)]
pub struct StateStore {
    inner: RwLock<Map<String, JsonValue>>,
}

impl StateStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store seeded with `initial`.
    pub fn from_map(initial: Map<String, JsonValue>) -> Self {
        Self {
            inner: RwLock::new(initial),
        }
    }

    /// Value for `key`, if present.
    pub fn get(&self, key: &str) -> Option<JsonValue> {
        self.inner.read().get(key).cloned()
    }

    /// Value for `key` deserialized as `T`; `None` when absent or not convertible.
    pub fn get_as<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(|v| serde_json::from_value(v).ok())
    }

    /// Set `key`.
    pub fn set(&self, key: impl Into<String>, value: impl Into<JsonValue>) {
        self.inner.write().insert(key.into(), value.into());
    }

    /// Remove `key`.
    pub fn delete(&self, key: &str) {
        self.inner.write().remove(key);
    }

    /// Remove everything.
    pub fn clear(&self) {
        self.inner.write().clear();
    }

    /// Copy of all entries.
    pub fn get_all(&self) -> Map<String, JsonValue> {
        self.inner.read().clone()
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> Vec<String> {
        self.inner.read().keys().cloned().collect()
    }

    /// Serialize the whole store (`stateToJSON`).
    pub fn to_json(&self) -> JsonValue {
        JsonValue::Object(self.get_all())
    }

    /// Replace the store from JSON (`loadStateFromJSON`); non-objects reset to empty.
    pub fn load_json(&self, json: JsonValue) {
        let map = match json {
            JsonValue::Object(m) => m,
            _ => Map::new(),
        };
        *self.inner.write() = map;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_get_delete_clear() {
        let s = StateStore::new();
        s.set("a", json!({"x": 1}));
        assert_eq!(s.get("a"), Some(json!({"x": 1})));
        assert_eq!(
            s.get_as::<serde_json::Map<String, JsonValue>>("a").unwrap()["x"],
            1
        );
        s.set("b", 2);
        assert_eq!(s.keys(), vec!["a", "b"]);
        s.delete("a");
        assert_eq!(s.get("a"), None);
        s.clear();
        assert!(s.keys().is_empty());
    }

    #[test]
    fn reads_are_copies() {
        let s = StateStore::new();
        s.set("a", json!([1]));
        let mut v = s.get("a").unwrap();
        v.as_array_mut().unwrap().push(json!(2));
        assert_eq!(s.get("a"), Some(json!([1])));
    }

    #[test]
    fn load_json_resets_on_non_object() {
        let s = StateStore::new();
        s.set("a", 1);
        s.load_json(json!([1, 2]));
        assert!(s.keys().is_empty());
        s.load_json(json!({"k": "v"}));
        assert_eq!(s.to_json(), json!({"k": "v"}));
    }
}
