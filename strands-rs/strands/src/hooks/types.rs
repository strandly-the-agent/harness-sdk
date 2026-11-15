//! Hook ordering and registration handles (TS `hooks/types.ts`).

use std::fmt;
use std::ops::{Add, Sub};

/// When a hook callback runs relative to others on the same event (TS `HookOrder`).
///
/// Lower runs first. Callbacks with the same order run in registration order, or in reverse
/// registration order on `After*` events. Offsets work: `HookOrder::SDK_FIRST - 1`.
///
/// ```
/// use strands::hooks::HookOrder;
///
/// assert!(HookOrder::SDK_FIRST - 1 < HookOrder::SDK_FIRST);
/// assert_eq!(HookOrder::default(), HookOrder::DEFAULT);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[expect(
    clippy::exhaustive_structs,
    reason = "a transparent newtype over the TS number"
)]
pub struct HookOrder(pub i32);

impl HookOrder {
    /// Runs before everything else (`-100`).
    pub const SDK_FIRST: Self = Self(-100);
    /// Intervention output checks (`-90`).
    pub const INTERVENTION_OUTPUT: Self = Self(-90);
    /// The default (`0`).
    pub const DEFAULT: Self = Self(0);
    /// Model routing (`50`).
    pub const MODEL_ROUTING: Self = Self(50);
    /// Intervention input checks (`90`).
    pub const INTERVENTION_INPUT: Self = Self(90);
    /// Runs after everything else (`100`).
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

/// Options for registering a hook callback (TS `HookCallbackOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bon::Builder)]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct HookCallbackOptions {
    /// When the callback runs; see [`HookOrder`].
    #[builder(default, into)]
    pub order: HookOrder,
}

/// Removes a registered hook callback (TS `HookCleanup`).
///
/// Dropping it keeps the callback registered; call [`HookCleanup::remove`] to unregister.
pub struct HookCleanup {
    remove: Box<dyn FnOnce() + Send + Sync>,
}

impl HookCleanup {
    pub(crate) fn new(remove: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            remove: Box::new(remove),
        }
    }

    /// Unregisters the callback. A no-op when the registry is gone.
    pub fn remove(self) {
        (self.remove)();
    }
}

impl fmt::Debug for HookCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookCleanup").finish_non_exhaustive()
    }
}
