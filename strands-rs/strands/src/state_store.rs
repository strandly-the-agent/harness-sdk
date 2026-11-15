//! Key-value JSON state shared by an agent and its hooks, tools and models (TS `state-store.ts`).

use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, JsonValidationError, Result};
use crate::types::json::{JsonObject, JsonValue};

/// JSON key-value state (TS `StateStore`), used for an agent's app state and model state.
///
/// A cheap-clone handle: clones share the same state. Values are stored as JSON, so every read
/// returns a copy and callers can never alias stored state. Use [`StateStore::snapshot`] for an
/// independent copy.
///
/// ```
/// use strands::StateStore;
///
/// let state = StateStore::new();
/// state.set("count", 1)?;
/// assert_eq!(state.get::<u32>("count")?, Some(1));
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct StateStore {
    state: Arc<RwLock<JsonObject>>,
}

impl StateStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store holding a copy of `initial`.
    pub fn from_map(initial: JsonObject) -> Self {
        Self {
            state: Arc::new(RwLock::new(initial)),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, JsonObject> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, JsonObject> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// The value at `key` as `T`, or `None` when the key is absent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Other`] when the stored value doesn't deserialize as `T`.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.get_value(key)
            .map(serde_json::from_value)
            .transpose()
            .map_err(Error::other)
    }

    /// A copy of the JSON value at `key`.
    pub fn get_value(&self, key: &str) -> Option<JsonValue> {
        self.read().get(key).cloned()
    }

    /// Stores a JSON copy of `value` at `key`, replacing any previous value.
    ///
    /// # Errors
    ///
    /// Returns a [`JsonValidationError`] when `value` can't be represented as JSON.
    pub fn set(&self, key: impl Into<String>, value: impl Serialize) -> Result<()> {
        let key = key.into();
        let value = serde_json::to_value(value).map_err(|error| {
            JsonValidationError::new(format!("value for key \"{key}\" cannot be serialized: {error}"))
        })?;
        self.write().insert(key, value);
        Ok(())
    }

    /// Removes `key`; a missing key is not an error.
    pub fn delete(&self, key: &str) {
        self.write().shift_remove(key);
    }

    /// Removes every key.
    pub fn clear(&self) {
        self.write().clear();
    }

    /// A copy of the whole state.
    pub fn get_all(&self) -> JsonObject {
        self.read().clone()
    }

    /// The keys, in insertion order.
    pub fn keys(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// An independent deep copy: later writes to either store don't reach the other.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        Self::from_map(self.get_all())
    }

    /// The state as a JSON object (TS `stateToJSONSymbol`).
    #[allow(dead_code, reason = "used by session persistence")]
    pub(crate) fn to_json(&self) -> JsonValue {
        JsonValue::Object(self.get_all())
    }

    /// Replaces the state with `json`, or clears it when `json` isn't an object (TS `loadStateFromJSONSymbol`).
    #[allow(dead_code, reason = "used by session persistence")]
    pub(crate) fn load_json(&self, json: JsonValue) {
        *self.write() = match json {
            JsonValue::Object(map) => map,
            _ => JsonObject::new(),
        };
    }
}

#[cfg(test)]
mod tests;
