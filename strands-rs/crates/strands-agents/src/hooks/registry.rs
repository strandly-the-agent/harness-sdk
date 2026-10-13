//! Hook registry (`strands-ts/src/hooks/registry.ts`). Callbacks are keyed by event type,
//! ordered by `order` (ties: registration order), and run sequentially.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;

use crate::error::Result;

/// A hookable event. Implemented by every struct in [`super::events`].
pub trait HookEvent: Any + Send {
    /// `true` for `After*` events, whose callbacks run in reverse registration order.
    fn reverse_callbacks() -> bool
    where
        Self: Sized,
    {
        false
    }
}

/// Preset callback orders. Any `i32` is valid; lower runs first.
pub mod hook_order {
    /// Runs before everything else.
    pub const SDK_FIRST: i32 = -100;
    /// Intervention output handlers.
    pub const INTERVENTION_OUTPUT: i32 = -90;
    /// Default.
    pub const DEFAULT: i32 = 0;
    /// Model routing.
    pub const MODEL_ROUTING: i32 = 50;
    /// Intervention input handlers.
    pub const INTERVENTION_INPUT: i32 = 90;
    /// Runs after everything else.
    pub const SDK_LAST: i32 = 100;
}

/// An async callback for event `E`.
pub type HookCallback<E> = Arc<dyn for<'a> Fn(&'a mut E) -> BoxFuture<'a, Result<()>> + Send + Sync>;

struct Entry {
    id: u64,
    order: i32,
    callback: Arc<dyn Any + Send + Sync>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    callbacks: HashMap<TypeId, Vec<Entry>>,
}

/// Registry of hook callbacks. Cheap to clone; clones share state.
#[derive(Clone, Default)]
pub struct HookRegistry {
    inner: Arc<Mutex<Inner>>,
}

/// Removes a registered callback when called. Idempotent.
pub struct HookCleanup(Box<dyn FnOnce() + Send + Sync>);

impl HookCleanup {
    /// Remove the callback.
    pub fn remove(self) {
        (self.0)()
    }
}

impl std::fmt::Debug for HookRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("HookRegistry")
            .field(
                "callbacks",
                &inner.callbacks.values().map(Vec::len).sum::<usize>(),
            )
            .finish()
    }
}

impl HookRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an async callback for `E` at `order`.
    pub fn add_callback_with_order<E: HookEvent>(
        &self,
        order: i32,
        callback: HookCallback<E>,
    ) -> HookCleanup {
        let id = {
            let mut inner = self.inner.lock();
            inner.next_id += 1;
            let id = inner.next_id;
            let list = inner.callbacks.entry(TypeId::of::<E>()).or_default();
            let entry = Entry {
                id,
                order,
                callback: Arc::new(callback),
            };
            match list.iter().position(|e| e.order > order) {
                Some(pos) => list.insert(pos, entry),
                None => list.push(entry),
            }
            id
        };
        let inner = Arc::clone(&self.inner);
        HookCleanup(Box::new(move || {
            if let Some(list) = inner.lock().callbacks.get_mut(&TypeId::of::<E>()) {
                list.retain(|e| e.id != id);
            }
        }))
    }

    /// Register an async callback for `E` at the default order.
    ///
    /// ```ignore
    /// hooks.add_callback::<BeforeToolCallEvent>(|event| Box::pin(async move { Ok(()) }));
    /// ```
    pub fn add_callback<E, F>(&self, callback: F) -> HookCleanup
    where
        E: HookEvent,
        F: for<'a> Fn(&'a mut E) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        self.add_callback_with_order::<E>(hook_order::DEFAULT, Arc::new(callback))
    }

    /// Register a synchronous callback for `E` at the default order.
    pub fn add_sync_callback<E, F>(&self, callback: F) -> HookCleanup
    where
        E: HookEvent,
        F: Fn(&mut E) -> Result<()> + Send + Sync + 'static,
    {
        self.add_sync_callback_with_order(hook_order::DEFAULT, callback)
    }

    /// Register a synchronous callback for `E` at `order`.
    pub fn add_sync_callback_with_order<E, F>(&self, order: i32, callback: F) -> HookCleanup
    where
        E: HookEvent,
        F: Fn(&mut E) -> Result<()> + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        self.add_callback_with_order::<E>(
            order,
            Arc::new(move |event: &mut E| {
                let result = callback(event);
                Box::pin(async move { result })
            }),
        )
    }

    fn callbacks_for<E: HookEvent>(&self) -> Vec<HookCallback<E>> {
        let inner = self.inner.lock();
        let Some(list) = inner.callbacks.get(&TypeId::of::<E>()) else {
            return Vec::new();
        };
        let mut out: Vec<HookCallback<E>> = list
            .iter()
            .filter_map(|e| e.callback.downcast_ref::<HookCallback<E>>().cloned())
            .collect();
        if E::reverse_callbacks() {
            out.reverse();
        }
        out
    }

    /// Run every callback registered for `E`, sequentially, stopping at the first error.
    pub async fn invoke_callbacks<E: HookEvent>(&self, event: &mut E) -> Result<()> {
        for callback in self.callbacks_for::<E>() {
            callback(event).await?;
        }
        Ok(())
    }

    /// Number of callbacks registered for `E`.
    pub fn count<E: HookEvent>(&self) -> usize {
        self.inner
            .lock()
            .callbacks
            .get(&TypeId::of::<E>())
            .map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Forward(Vec<&'static str>);
    impl HookEvent for Forward {}

    struct Backward(Vec<&'static str>);
    impl HookEvent for Backward {
        fn reverse_callbacks() -> bool {
            true
        }
    }

    #[tokio::test]
    async fn runs_in_order_then_registration_order() {
        let hooks = HookRegistry::new();
        hooks.add_sync_callback_with_order(10, |e: &mut Forward| {
            e.0.push("late");
            Ok(())
        });
        hooks.add_sync_callback(|e: &mut Forward| {
            e.0.push("a");
            Ok(())
        });
        hooks.add_sync_callback(|e: &mut Forward| {
            e.0.push("b");
            Ok(())
        });
        hooks.add_sync_callback_with_order(hook_order::SDK_FIRST, |e: &mut Forward| {
            e.0.push("first");
            Ok(())
        });
        let mut ev = Forward(vec![]);
        hooks.invoke_callbacks(&mut ev).await.unwrap();
        assert_eq!(ev.0, vec!["first", "a", "b", "late"]);
    }

    #[tokio::test]
    async fn reverse_events_run_backwards_within_order() {
        let hooks = HookRegistry::new();
        hooks.add_sync_callback(|e: &mut Backward| {
            e.0.push("a");
            Ok(())
        });
        hooks.add_sync_callback(|e: &mut Backward| {
            e.0.push("b");
            Ok(())
        });
        hooks.add_sync_callback_with_order(hook_order::SDK_FIRST, |e: &mut Backward| {
            e.0.push("first");
            Ok(())
        });
        let mut ev = Backward(vec![]);
        hooks.invoke_callbacks(&mut ev).await.unwrap();
        assert_eq!(ev.0, vec!["b", "a", "first"]);
    }

    #[tokio::test]
    async fn cleanup_removes_once_and_async_callbacks_work() {
        let hooks = HookRegistry::new();
        let cleanup = hooks.add_callback(|e: &mut Forward| {
            Box::pin(async move {
                e.0.push("async");
                Ok(())
            })
        });
        assert_eq!(hooks.count::<Forward>(), 1);
        let mut ev = Forward(vec![]);
        hooks.invoke_callbacks(&mut ev).await.unwrap();
        assert_eq!(ev.0, vec!["async"]);
        cleanup.remove();
        assert_eq!(hooks.count::<Forward>(), 0);
        assert_eq!(hooks.count::<Backward>(), 0);
    }

    #[tokio::test]
    async fn errors_stop_the_chain() {
        let hooks = HookRegistry::new();
        hooks.add_sync_callback(|_: &mut Forward| Err(crate::Error::Plugin("nope".into())));
        hooks.add_sync_callback(|e: &mut Forward| {
            e.0.push("never");
            Ok(())
        });
        let mut ev = Forward(vec![]);
        assert!(hooks.invoke_callbacks(&mut ev).await.is_err());
        assert!(ev.0.is_empty());
    }
}
