//! Key-value JSON state shared by an agent, its hooks and its tools (TS `state-store.ts`).

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{JsonValidationError, Result};
use crate::types::json::JsonValue;

type JsonMap = serde_json::Map<String, JsonValue>;

/// A key-value store of JSON values (TS `StateStore`).
///
/// Cloning is cheap and shares the same state, so the agent, hooks and tools all see one store.
/// Values go in and out as copies: changing a value read from the store never changes the store.
/// Use [`StateStore::snapshot`] for an independent copy. The agent's `app_state` and `model_state`
/// are both a `StateStore`.
///
/// ```
/// use strands::StateStore;
///
/// let state = StateStore::new();
/// state.set("count", 1)?;
/// assert_eq!(state.get::<u32>("count")?, Some(1));
/// assert_eq!(state.keys(), ["count"]);
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone, Default)]
pub struct StateStore {
    state: Arc<RwLock<JsonMap>>,
}

impl StateStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value at `key` deserialized as `T`, or `None` when the key is absent.
    ///
    /// # Errors
    ///
    /// [`Error::Json`](crate::Error::Json) when the stored value doesn't deserialize as `T`.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.get_value(key)
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    /// A copy of the JSON value at `key`, or `None` when the key is absent.
    pub fn get_value(&self, key: &str) -> Option<JsonValue> {
        self.read().get(key).cloned()
    }

    /// Stores a copy of `value` at `key`, replacing any previous value.
    ///
    /// # Errors
    ///
    /// [`Error::JsonValidation`](crate::Error::JsonValidation) when `value` can't be represented
    /// as JSON (for example a map with non-string keys).
    pub fn set(&self, key: impl Into<String>, value: impl Serialize) -> Result<()> {
        let key = key.into();
        let value = serde_json::to_value(value).map_err(|error| {
            JsonValidationError::new(format!("value for key \"{key}\" cannot be serialized: {error}"))
        })?;
        self.write().insert(key, value);
        Ok(())
    }

    /// Removes `key`. Removing an absent key does nothing.
    pub fn delete(&self, key: &str) {
        self.write().shift_remove(key);
    }

    /// Removes every key.
    pub fn clear(&self) {
        self.write().clear();
    }

    /// A copy of the whole state.
    pub fn get_all(&self) -> JsonMap {
        self.read().clone()
    }

    /// The keys, in insertion order.
    pub fn keys(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// An independent store holding a copy of this one's state.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        Self::from(self.get_all())
    }

    /// The state as a JSON object, for snapshots (TS `stateToJSONSymbol`).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by snapshots and sessions"))]
    pub(crate) fn to_json(&self) -> JsonValue {
        JsonValue::Object(self.get_all())
    }

    /// Replaces the state with `json`; anything but an object clears it (TS `loadStateFromJSONSymbol`).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by snapshots and sessions"))]
    pub(crate) fn load_json(&self, json: JsonValue) {
        *self.write() = match json {
            JsonValue::Object(map) => map,
            _ => JsonMap::new(),
        };
    }

    fn read(&self) -> RwLockReadGuard<'_, JsonMap> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, JsonMap> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl From<JsonMap> for StateStore {
    /// A store holding `initial` (TS `new StateStore(initialState)`).
    fn from(initial: JsonMap) -> Self {
        Self {
            state: Arc::new(RwLock::new(initial)),
        }
    }
}

impl fmt::Debug for StateStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StateStore").field(&*self.read()).finish()
    }
}

#[cfg(test)]
mod tests;
