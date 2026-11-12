//! Per-invocation caller state (TS `InvocationState` in `types/agent.ts`).

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

/// Values the caller passes to every hook and tool of one invocation (TS `invocationState`).
///
/// A typed map keyed by type, like [`http::Extensions`], so live values (clients, pools) reach
/// tools and hooks without downcasting. Clones share the same map. The SDK never stores its
/// own state here.
///
/// ```
/// use strands::InvocationState;
///
/// #[derive(Clone)]
/// struct TenantId(String);
///
/// let state = InvocationState::new();
/// state.insert(TenantId("acme".into()));
/// assert_eq!(state.get::<TenantId>().map(|t| t.0), Some("acme".to_string()));
/// ```
#[derive(Clone, Default)]
pub struct InvocationState {
    inner: Arc<RwLock<http::Extensions>>,
}

impl InvocationState {
    /// An empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `value`, returning the previous value of the same type.
    pub fn insert<T: Clone + Send + Sync + 'static>(&self, value: T) -> Option<T> {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(value)
    }

    /// A clone of the stored value of type `T`.
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get::<T>()
            .cloned()
    }

    /// Removes and returns the stored value of type `T`.
    pub fn remove<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove::<T>()
    }

    /// Whether no value is stored.
    pub fn is_empty(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// Whether `other` shares this state's map.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl fmt::Debug for InvocationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvocationState")
            .field(
                "len",
                &self.inner.read().unwrap_or_else(PoisonError::into_inner).len(),
            )
            .finish()
    }
}
