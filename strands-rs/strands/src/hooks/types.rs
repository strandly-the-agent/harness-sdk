//! Hook registration types (TS `hooks/types.ts`).

use std::fmt;
use std::ops::{Add, Sub};
use std::sync::Arc;

use async_trait::async_trait;

/// When a hook runs relative to the other hooks of the same event (TS `HookOrder`). Lower values
/// run first.
///
/// Any value is valid; the constants are reference points, not bounds. [`SDK_FIRST`](Self::SDK_FIRST)
/// and [`SDK_LAST`](Self::SDK_LAST) mark where the SDK's own hooks run, so you can place yours
/// around them:
///
/// ```
/// use strands::hooks::HookOrder;
///
/// let before_sdk = HookOrder::SDK_FIRST - 1;
/// assert_eq!(before_sdk, HookOrder(-101));
/// assert!(before_sdk < HookOrder::SDK_FIRST);
/// assert_eq!(HookOrder::default(), HookOrder::DEFAULT);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookOrder(pub i32);

impl HookOrder {
    /// Where the SDK's earliest hooks run, `-100`.
    pub const SDK_FIRST: Self = Self(-100);
    /// Where interventions inspecting outputs run, `-90`.
    pub const INTERVENTION_OUTPUT: Self = Self(-90);
    /// The order of a hook registered without one, `0`.
    pub const DEFAULT: Self = Self(0);
    /// Where model routing runs, `50`.
    pub const MODEL_ROUTING: Self = Self(50);
    /// Where interventions inspecting inputs run, `90`.
    pub const INTERVENTION_INPUT: Self = Self(90);
    /// Where the SDK's latest hooks run, `100`.
    pub const SDK_LAST: Self = Self(100);
}

impl From<i32> for HookOrder {
    fn from(order: i32) -> Self {
        Self(order)
    }
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

/// Options for registering a hook (TS `HookCallbackOptions`).
///
/// ```
/// use strands::hooks::{HookOptions, HookOrder};
///
/// let options = HookOptions::builder().order(HookOrder::SDK_LAST).build();
/// assert_eq!(options, HookOptions { order: HookOrder::SDK_LAST });
/// assert_eq!(HookOptions::default().order, HookOrder::DEFAULT);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
pub struct HookOptions {
    /// When the hook runs; lower values run first. Defaults to [`HookOrder::DEFAULT`].
    #[builder(default, into)]
    pub order: HookOrder,
}

/// Removes the hook it was returned for (TS `HookCleanup`).
///
/// [`remove`](Self::remove) is idempotent and does nothing once the hook is gone. Dropping the
/// handle keeps the hook registered, so ignoring it is the usual case.
#[derive(Clone)]
pub struct HookCleanup {
    remove: Arc<dyn Fn() + Send + Sync>,
}

impl HookCleanup {
    pub(crate) fn new(remove: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            remove: Arc::new(remove),
        }
    }

    /// Removes the hook. Calling it again, or after the registry is gone, does nothing.
    pub fn remove(&self) {
        (self.remove)();
    }
}

impl fmt::Debug for HookCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookCleanup").finish_non_exhaustive()
    }
}

/// A hook written as a type, registered with
/// [`HookRegistry::add_hook_impl`](crate::hooks::HookRegistry::add_hook_impl).
///
/// Implement it with [`async_trait`](crate::async_trait). Events that borrow the agent name their
/// lifetime in the impl:
///
/// ```
/// use strands::async_trait;
/// use strands::hooks::{BeforeInvocationEvent, Hook, HookRegistry};
///
/// struct Audit;
///
/// #[async_trait]
/// impl<'x> Hook<BeforeInvocationEvent<'x>> for Audit {
///     async fn handle(&self, event: &mut BeforeInvocationEvent<'x>) {
///         println!("invocation with {} messages", event.messages().len());
///     }
/// }
///
/// let registry = HookRegistry::new();
/// registry.add_hook_impl(Audit);
/// ```
#[async_trait]
pub trait Hook<E: Send>: Send + Sync + 'static {
    /// Handles one event.
    async fn handle(&self, event: &mut E);
}
