//! The typed hook registry (TS `hooks/registry.ts`).
//!
//! Each event type has its own callback list, selected at compile time through the sealed
//! [`HookEvent`] and [`Slot`] traits, so no callback is stored or looked up through `Any`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use futures::future::BoxFuture;

use crate::hooks::events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, ContentBlockEvent,
    InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent, ModelStreamUpdateEvent,
    ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::hooks::types::{Hook, HookCleanup, HookOptions, HookOrder};

/// A hookable event type (TS `HookableEvent`), used as the key of its callback list.
///
/// Events that borrow the agent are keyed by their `'static` form, so
/// `BeforeInvocationEvent<'static>` is the key whose callbacks receive
/// `&mut BeforeInvocationEvent<'a>`; `'static` events are their own key. Sealed: the SDK defines
/// every event.
pub trait HookEvent: private::Sealed + 'static {
    /// The event a callback receives, at the lifetime of the borrow it was fired with.
    type Event<'a>: Send;
}

/// Maps an event type of any lifetime to its [`HookEvent`] key, so
/// [`HookRegistry::add_async_hook`] can be named with the event type itself
/// (`add_async_hook::<BeforeInvocationEvent<'_>>`). Sealed.
pub trait EventKind: private::Sealed {
    /// The key of the event's callback list.
    type Kind: HookEvent;
}

/// Selects an event's callback list in a [`HookRegistry`]. Sealed; implemented for every
/// [`HookEvent`].
pub trait Slot: HookEvent + private::SlotAccess {}

/// A sync callback for the event keyed by `M`, as [`HookRegistry::add_hook`] takes it.
///
/// Implemented for closures taking `&mut` the event, for example
/// `|event: &mut BeforeInvocationEvent<'_>| ..` or `|event: &mut BeforeToolCallEvent| ..`. The
/// annotated argument type selects the event.
pub trait IntoSyncHook<M: HookEvent> {
    /// Converts the closure into a stored callback.
    fn into_callback(self) -> SyncCallback<M>;
}

/// A [`Hook`] implementation for the event keyed by `M`, as [`HookRegistry::add_hook_impl`]
/// takes it.
///
/// Implemented for every type implementing [`Hook`] for one event; the implemented event selects
/// the callback list.
pub trait IntoHookImpl<M: HookEvent> {
    /// Converts the hook into a stored callback.
    fn into_callback(self) -> AsyncCallback<M>;
}

/// A stored sync callback for the event keyed by `M`.
pub type SyncCallback<M> = Arc<dyn for<'a, 'b> Fn(&'b mut <M as HookEvent>::Event<'a>) + Send + Sync>;

/// A stored async callback for the event keyed by `M`.
pub type AsyncCallback<M> =
    Arc<dyn for<'a, 'b> Fn(&'b mut <M as HookEvent>::Event<'a>) -> BoxFuture<'b, ()> + Send + Sync>;

pub(crate) mod private {
    use super::{Entries, HookRegistry};

    pub trait Sealed {}

    pub trait SlotAccess: super::HookEvent + Sized {
        /// Whether same-order callbacks run in reverse registration order (TS
        /// `_shouldReverseCallbacks`, true for the `After*` events).
        const REVERSE: bool;

        fn entries(registry: &HookRegistry) -> &Entries<Self>;
    }
}

enum Callback<M: HookEvent> {
    Sync(SyncCallback<M>),
    Async(AsyncCallback<M>),
}

impl<M: HookEvent> Clone for Callback<M> {
    fn clone(&self) -> Self {
        match self {
            Self::Sync(callback) => Self::Sync(Arc::clone(callback)),
            Self::Async(callback) => Self::Async(Arc::clone(callback)),
        }
    }
}

struct Entry<M: HookEvent> {
    id: u64,
    order: HookOrder,
    callback: Callback<M>,
}

/// The callbacks registered for one event, lowest order first and same-order callbacks in
/// registration order.
pub struct Entries<M: HookEvent>(Arc<RwLock<Vec<Entry<M>>>>);

impl<M: HookEvent> Default for Entries<M> {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(Vec::new())))
    }
}

impl<M: HookEvent> Entries<M> {
    fn len(&self) -> usize {
        self.0.read().unwrap_or_else(PoisonError::into_inner).len()
    }

    fn insert(&self, entry: Entry<M>) -> HookCleanup {
        let id = entry.id;
        {
            let mut entries = self.0.write().unwrap_or_else(PoisonError::into_inner);
            let at = entries
                .iter()
                .position(|existing| existing.order > entry.order)
                .unwrap_or(entries.len());
            entries.insert(at, entry);
        }
        let entries: Weak<RwLock<Vec<Entry<M>>>> = Arc::downgrade(&self.0);
        HookCleanup::new(move || {
            if let Some(entries) = entries.upgrade() {
                entries
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|entry| entry.id != id);
            }
        })
    }

    fn snapshot(&self, reverse: bool) -> Vec<Callback<M>> {
        let mut entries: Vec<(HookOrder, Callback<M>)> = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|entry| (entry.order, entry.callback.clone()))
            .collect();
        if reverse {
            entries.reverse();
            entries.sort_by_key(|(order, _)| *order);
        }
        entries.into_iter().map(|(_, callback)| callback).collect()
    }
}

macro_rules! hook_events {
    (
        borrowed { $($b_field:ident: $b_ty:ident, reverse = $b_rev:literal;)* }
        owned { $($o_field:ident: $o_ty:ident, reverse = $o_rev:literal;)* }
    ) => {
        /// Hook callbacks by event type (TS `HookRegistry`).
        ///
        /// The agent shares one registry behind an `Arc`. Registering is `&self`, so hooks can be
        /// added and removed while events fire: a firing event runs the callbacks registered when
        /// it started. Callbacks run lowest [`HookOrder`] first; same-order callbacks run in
        /// registration order, or in reverse registration order on `After*` events so cleanup
        /// unwinds setup.
        ///
        /// ```
        /// use strands::hooks::{BeforeInvocationEvent, BeforeToolCallEvent, HookOptions, HookOrder, HookRegistry};
        ///
        /// let registry = HookRegistry::new();
        /// registry.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.messages_mut().clear());
        /// registry.add_async_hook::<BeforeToolCallEvent>(|event| {
        ///     Box::pin(async move {
        ///         let _ = event.tool_use().name.len();
        ///     })
        /// });
        /// let cleanup = registry.add_hook_with(
        ///     HookOptions { order: HookOrder::SDK_FIRST - 1 },
        ///     |_event: &mut BeforeToolCallEvent| {},
        /// );
        /// cleanup.remove();
        /// ```
        #[derive(Default)]
        pub struct HookRegistry {
            next_id: AtomicU64,
            $($b_field: Entries<$b_ty<'static>>,)*
            $($o_field: Entries<$o_ty>,)*
        }

        impl fmt::Debug for HookRegistry {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("HookRegistry")
                    $(.field(stringify!($b_field), &self.$b_field.len()))*
                    $(.field(stringify!($o_field), &self.$o_field.len()))*
                    .finish()
            }
        }

        $(
            impl private::Sealed for $b_ty<'_> {}

            impl HookEvent for $b_ty<'static> {
                type Event<'a> = $b_ty<'a>;
            }

            impl EventKind for $b_ty<'_> {
                type Kind = $b_ty<'static>;
            }

            impl private::SlotAccess for $b_ty<'static> {
                const REVERSE: bool = $b_rev;

                fn entries(registry: &HookRegistry) -> &Entries<Self> {
                    &registry.$b_field
                }
            }

            impl Slot for $b_ty<'static> {}

            impl<F> IntoSyncHook<$b_ty<'static>> for F
            where
                F: for<'a, 'b> Fn(&'b mut $b_ty<'a>) + Send + Sync + 'static,
            {
                fn into_callback(self) -> SyncCallback<$b_ty<'static>> {
                    Arc::new(self)
                }
            }

            impl<H> IntoHookImpl<$b_ty<'static>> for H
            where
                H: for<'a> Hook<$b_ty<'a>>,
            {
                fn into_callback(self) -> AsyncCallback<$b_ty<'static>> {
                    let hook = Arc::new(self);
                    Arc::new(move |event| {
                        let hook = Arc::clone(&hook);
                        Box::pin(async move { hook.handle(event).await })
                    })
                }
            }
        )*

        $(
            impl private::Sealed for $o_ty {}

            impl HookEvent for $o_ty {
                type Event<'a> = $o_ty;
            }

            impl EventKind for $o_ty {
                type Kind = $o_ty;
            }

            impl private::SlotAccess for $o_ty {
                const REVERSE: bool = $o_rev;

                fn entries(registry: &HookRegistry) -> &Entries<Self> {
                    &registry.$o_field
                }
            }

            impl Slot for $o_ty {}

            impl<F> IntoSyncHook<$o_ty> for F
            where
                F: for<'b> Fn(&'b mut $o_ty) + Send + Sync + 'static,
            {
                fn into_callback(self) -> SyncCallback<$o_ty> {
                    Arc::new(self)
                }
            }

            impl<H> IntoHookImpl<$o_ty> for H
            where
                H: Hook<$o_ty>,
            {
                fn into_callback(self) -> AsyncCallback<$o_ty> {
                    let hook = Arc::new(self);
                    Arc::new(move |event| {
                        let hook = Arc::clone(&hook);
                        Box::pin(async move { hook.handle(event).await })
                    })
                }
            }
        )*
    };
}

hook_events! {
    borrowed {
        initialized: InitializedEvent, reverse = false;
        before_invocation: BeforeInvocationEvent, reverse = false;
        after_invocation: AfterInvocationEvent, reverse = true;
        message_added: MessageAddedEvent, reverse = false;
        before_model_call: BeforeModelCallEvent, reverse = false;
        after_model_call: AfterModelCallEvent, reverse = true;
        model_stream_update: ModelStreamUpdateEvent, reverse = false;
        content_block: ContentBlockEvent, reverse = false;
        model_message: ModelMessageEvent, reverse = false;
        agent_result: AgentResultEvent, reverse = false;
        interrupt: InterruptEvent, reverse = false;
        before_tools: BeforeToolsEvent, reverse = false;
        after_tools: AfterToolsEvent, reverse = true;
    }
    owned {
        before_tool_call: BeforeToolCallEvent, reverse = false;
        after_tool_call: AfterToolCallEvent, reverse = true;
        tool_stream_update: ToolStreamUpdateEvent, reverse = false;
        tool_result: ToolResultEvent, reverse = false;
    }
}

impl HookRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a sync callback at [`HookOrder::DEFAULT`] (TS `addCallback`). The closure's
    /// argument type selects the event.
    ///
    /// ```
    /// use strands::hooks::{AfterToolCallEvent, HookRegistry};
    ///
    /// let registry = HookRegistry::new();
    /// registry.add_hook(|event: &mut AfterToolCallEvent| {
    ///     if event.error().is_some() {
    ///         event.set_retry(true);
    ///     }
    /// });
    /// ```
    pub fn add_hook<M: Slot, F: IntoSyncHook<M>>(&self, callback: F) -> HookCleanup {
        self.add_hook_with(HookOptions::default(), callback)
    }

    /// Registers a sync callback with options, for example an order.
    pub fn add_hook_with<M: Slot, F: IntoSyncHook<M>>(
        &self,
        options: HookOptions,
        callback: F,
    ) -> HookCleanup {
        self.insert::<M>(options, Callback::Sync(callback.into_callback()))
    }

    /// Registers an async callback at [`HookOrder::DEFAULT`]; name the event with a turbofish and
    /// return a boxed future.
    ///
    /// ```
    /// use strands::hooks::{BeforeModelCallEvent, HookRegistry};
    ///
    /// let registry = HookRegistry::new();
    /// registry.add_async_hook::<BeforeModelCallEvent<'_>>(|event| {
    ///     Box::pin(async move {
    ///         tokio::task::yield_now().await;
    ///         let _ = event.agent().messages().len();
    ///     })
    /// });
    /// ```
    pub fn add_async_hook<E>(
        &self,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup
    where
        E: EventKind,
        E::Kind: Slot,
    {
        self.add_async_hook_with::<E>(HookOptions::default(), callback)
    }

    /// Registers an async callback with options, for example an order.
    pub fn add_async_hook_with<E>(
        &self,
        options: HookOptions,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup
    where
        E: EventKind,
        E::Kind: Slot,
    {
        self.insert::<E::Kind>(options, Callback::Async(Arc::new(callback)))
    }

    /// Registers a [`Hook`] implementation at [`HookOrder::DEFAULT`]. The implemented event
    /// selects the callback list.
    pub fn add_hook_impl<M: Slot, H: IntoHookImpl<M>>(&self, hook: H) -> HookCleanup {
        self.add_hook_impl_with(HookOptions::default(), hook)
    }

    /// Registers a [`Hook`] implementation with options, for example an order.
    pub fn add_hook_impl_with<M: Slot, H: IntoHookImpl<M>>(
        &self,
        options: HookOptions,
        hook: H,
    ) -> HookCleanup {
        self.insert::<M>(options, Callback::Async(hook.into_callback()))
    }

    fn insert<M: Slot>(&self, options: HookOptions, callback: Callback<M>) -> HookCleanup {
        let entry = Entry {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            order: options.order,
            callback,
        };
        M::entries(self).insert(entry)
    }

    /// Runs the callbacks registered for the event in order, awaiting each (TS
    /// `invokeCallbacks`).
    ///
    /// The callback list is copied under a short lock first, so callbacks may register or remove
    /// hooks. Interrupts raised through an event's `interrupt` stay on the event; read them with
    /// its `take_interrupts`.
    ///
    /// The future is boxed so its `Send` bound is proven here, for any event lifetime; rustc
    /// can't prove it through the callbacks' higher-ranked types at an `async fn` call site.
    pub(crate) fn invoke<'e, M: Slot>(&self, event: &'e mut M::Event<'_>) -> BoxFuture<'e, ()> {
        let callbacks = M::entries(self).snapshot(M::REVERSE);
        Box::pin(async move {
            for callback in callbacks {
                match callback {
                    Callback::Sync(callback) => callback(event),
                    Callback::Async(callback) => callback(event).await,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
