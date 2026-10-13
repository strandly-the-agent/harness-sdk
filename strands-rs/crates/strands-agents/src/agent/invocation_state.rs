//! Per-invocation shared state (`InvocationState` in TS): a mutable bag shared by every event
//! and tool context of one invocation. Clones share the same underlying map.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// Mutable, type-erased key/value bag shared across one invocation.
#[derive(Clone, Default)]
pub struct InvocationState {
    inner: Arc<Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>>,
}

impl InvocationState {
    /// Empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `value` under `key`, replacing any previous value.
    pub fn insert<T: Any + Send + Sync>(&self, key: impl Into<String>, value: T) {
        self.inner.lock().insert(key.into(), Arc::new(value));
    }

    /// Value under `key` as `T`, if present and of that type.
    pub fn get<T: Any + Send + Sync>(&self, key: &str) -> Option<Arc<T>> {
        self.inner
            .lock()
            .get(key)
            .cloned()
            .and_then(|v| v.downcast::<T>().ok())
    }

    /// Remove `key`.
    pub fn remove(&self, key: &str) {
        self.inner.lock().remove(key);
    }

    /// `true` when `key` is present.
    pub fn contains(&self, key: &str) -> bool {
        self.inner.lock().contains_key(key)
    }

    /// Keys currently stored.
    pub fn keys(&self) -> Vec<String> {
        self.inner.lock().keys().cloned().collect()
    }

    /// `true` when both handles share the same underlying map.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl std::fmt::Debug for InvocationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.inner.lock().keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_identity_and_values() {
        let a = InvocationState::new();
        let b = a.clone();
        a.insert("n", 5u32);
        assert_eq!(*b.get::<u32>("n").unwrap(), 5);
        assert!(b.get::<String>("n").is_none());
        assert!(a.ptr_eq(&b));
        assert!(!a.ptr_eq(&InvocationState::new()));
        b.remove("n");
        assert!(!a.contains("n"));
    }
}
