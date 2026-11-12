//! Hook ordering and registration handles (TS `hooks/types.ts`).

use std::fmt;
use std::ops::{Add, Sub};
use std::sync::{Arc, Mutex, PoisonError};

/// Where a hook runs relative to other hooks on the same event (TS `HookOrder`).
///
/// Lower values run first. Offsets work: `HookOrder::SDK_FIRST - 1` runs before the SDK's own
/// first hooks. On `After*` events, hooks of the same order run in reverse registration order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookOrder(pub i32);

impl HookOrder {
    /// SDK hooks that must run before everything else.
    pub const SDK_FIRST: Self = Self(-100);
    /// Intervention hooks that inspect outputs.
    pub const INTERVENTION_OUTPUT: Self = Self(-90);
    /// The default order.
    pub const DEFAULT: Self = Self(0);
    /// Model routing hooks.
    pub const MODEL_ROUTING: Self = Self(50);
    /// Intervention hooks that inspect inputs.
    pub const INTERVENTION_INPUT: Self = Self(90);
    /// SDK hooks that must run after everything else.
    pub const SDK_LAST: Self = Self(100);
}

impl Add<i32> for HookOrder {
    type Output = Self;

    fn add(self, offset: i32) -> Self {
        Self(self.0.saturating_add(offset))
    }
}

impl Sub<i32> for HookOrder {
    type Output = Self;

    fn sub(self, offset: i32) -> Self {
        Self(self.0.saturating_sub(offset))
    }
}

impl From<i32> for HookOrder {
    fn from(order: i32) -> Self {
        Self(order)
    }
}

/// Options for registering a hook (TS `HookCallbackOptions`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
pub struct HookOptions {
    /// Where the hook runs relative to others; [`HookOrder::DEFAULT`] when not set.
    #[builder(default, into)]
    pub order: HookOrder,
}

type Remove = Box<dyn FnOnce() + Send + Sync>;

/// Removes a registered hook (TS `HookCleanup`).
///
/// Call [`HookCleanup::remove`]; dropping the handle keeps the hook registered, so ignoring it
/// is fine.
pub struct HookCleanup {
    remove: Arc<Mutex<Option<Remove>>>,
}

impl HookCleanup {
    pub(crate) fn new(remove: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            remove: Arc::new(Mutex::new(Some(Box::new(remove)))),
        }
    }

    /// Removes the hook. Calling it again does nothing.
    pub fn remove(&self) {
        let remove = self.remove.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(remove) = remove {
            remove();
        }
    }
}

impl Clone for HookCleanup {
    fn clone(&self) -> Self {
        Self {
            remove: Arc::clone(&self.remove),
        }
    }
}

impl fmt::Debug for HookCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookCleanup").finish_non_exhaustive()
    }
}
