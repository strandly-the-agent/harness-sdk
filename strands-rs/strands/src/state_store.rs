//! Key-value JSON state outside the conversation (TS `state-store.ts`).

use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Map;

use crate::error::{Error, JsonValidationError, Result};
use crate::types::json::{JsonValue, js_ordered_entries};

/// Key-value storage for application or model state, kept outside the conversation (TS
/// `StateStore`, called `AppState` in prose where TS does).
///
/// The model doesn't see this state during inference; tools and application code read and write
/// it. Clones share one store, so a clone handed to a tool writes through to the agent. Reads return
/// copies, so a returned value never aliases stored state. [`StateStore::snapshot`] makes an
/// independent copy.
///
/// ```
/// use strands::StateStore;
///
/// let state = StateStore::new();
/// state.set("userId", "user-123").unwrap();
/// let user_id: Option<String> = state.get("userId").unwrap();
/// assert_eq!(user_id.as_deref(), Some("user-123"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct StateStore {
    inner: Arc<RwLock<Map<String, JsonValue>>>,
}

impl StateStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value for `key` deserialized as `T`, or `None` when the key doesn't exist.
    ///
    /// # Errors
    ///
    /// Fails if the stored value doesn't deserialize as `T`.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.get_value(key)
            .map(serde_json::from_value)
            .transpose()
            .map_err(Error::from)
    }

    /// A copy of the JSON value for `key`, or `None` when the key doesn't exist.
    #[must_use]
    pub fn get_value(&self, key: &str) -> Option<JsonValue> {
        self.read().get(key).cloned()
    }

    /// Stores `value` under `key`, replacing any previous value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::JsonValidation`] if `value` can't be serialized as JSON (for example a map
    /// with non-string keys).
    pub fn set(&self, key: impl Into<String>, value: impl Serialize) -> Result<()> {
        let key = key.into();
        let value = serde_json::to_value(value).map_err(|error| {
            JsonValidationError::new(format!("value for key \"{key}\" cannot be serialized: {error}"))
        })?;
        self.write().insert(key, value);
        Ok(())
    }

    /// Removes `key`. Removing a missing key does nothing.
    pub fn delete(&self, key: &str) {
        self.write().shift_remove(key);
    }

    /// Removes every key.
    pub fn clear(&self) {
        self.write().clear();
    }

    /// A copy of the whole state, its keys in [`StateStore::keys`] order.
    #[must_use]
    pub fn get_all(&self) -> Map<String, JsonValue> {
        js_ordered_entries(&self.read())
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The keys in JavaScript `Object.keys` order, as TS returns them: keys that are array indexes
    /// (`"0"`, `"1"`, up to `"4294967294"`) ascending, then the rest in insertion order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        js_ordered_entries(&self.read())
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// A new, independent store holding a copy of this one's state.
    ///
    /// ```
    /// use strands::StateStore;
    ///
    /// let state = StateStore::new();
    /// state.set("count", 1).unwrap();
    /// let copy = state.snapshot();
    /// copy.set("count", 2).unwrap();
    /// assert_eq!(state.get::<u64>("count").unwrap(), Some(1));
    /// ```
    #[must_use]
    pub fn snapshot(&self) -> Self {
        Self::from(self.get_all())
    }

    /// The state as a JSON object (TS `[stateToJSONSymbol]`).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by agent snapshots"))]
    pub(crate) fn state_to_json(&self) -> JsonValue {
        JsonValue::Object(self.get_all())
    }

    /// Replaces the state from JSON; anything but an object clears it (TS
    /// `[loadStateFromJSONSymbol]`).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by agent snapshots"))]
    pub(crate) fn load_state_from_json(&self, json: JsonValue) {
        *self.write() = match json {
            JsonValue::Object(map) => map,
            _ => Map::new(),
        };
    }

    fn read(&self) -> RwLockReadGuard<'_, Map<String, JsonValue>> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Map<String, JsonValue>> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl From<Map<String, JsonValue>> for StateStore {
    /// A store holding `initial` (TS `new StateStore(initialState)`).
    fn from(initial: Map<String, JsonValue>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
        }
    }
}

#[cfg(test)]
mod tests;
