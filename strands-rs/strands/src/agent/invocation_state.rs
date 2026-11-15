//! Per-invocation state shared by hooks and tools (TS `InvocationState` in `types/agent.ts`).

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

/// Values the caller passes to the hooks and tools of one invocation (TS `InvocationState`).
///
/// A typed map with one value per type, like [`http::Extensions`], so live values such as
/// clients and pools reach tools without downcasting. Cloning is cheap and every clone sees the
/// same values. It belongs to the caller: the SDK never stores its own state here.
///
/// ```
/// use strands::InvocationState;
///
/// #[derive(Clone)]
/// struct RequestId(String);
///
/// let state = InvocationState::new();
/// state.insert(RequestId("req-1".into()));
/// let view = state.clone();
/// assert_eq!(view.get::<RequestId>().map(|id| id.0).as_deref(), Some("req-1"));
/// ```
#[derive(Clone, Default)]
pub struct InvocationState {
    values: Arc<RwLock<http::Extensions>>,
}

impl InvocationState {
    /// An empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `value`, returning the value of the same type it replaces.
    pub fn insert<T: Clone + Send + Sync + 'static>(&self, value: T) -> Option<T> {
        self.values
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(value)
    }

    /// A clone of the stored value of type `T`.
    #[must_use]
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.values
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get::<T>()
            .cloned()
    }

    /// Removes and returns the stored value of type `T`.
    pub fn remove<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.values
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove::<T>()
    }

    /// Whether a value of type `T` is stored.
    #[must_use]
    pub fn contains<T: Clone + Send + Sync + 'static>(&self) -> bool {
        self.values
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get::<T>()
            .is_some()
    }
}

impl fmt::Debug for InvocationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvocationState").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Counter(u32);

    #[test]
    fn clones_share_values() {
        let state = InvocationState::new();
        let other = state.clone();
        assert_eq!(state.insert(Counter(1)), None);
        assert_eq!(other.get::<Counter>(), Some(Counter(1)));
        assert_eq!(other.insert(Counter(2)), Some(Counter(1)));
        assert!(state.contains::<Counter>());
        assert_eq!(state.remove::<Counter>(), Some(Counter(2)));
        assert!(!other.contains::<Counter>());
    }
}
