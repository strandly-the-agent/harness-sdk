//! The typed hook registry (TS `hooks/registry.ts`).
//!
//! Each event type has a marker implementing the sealed [`HookEvent`] trait; a `'static` event
//! is its own marker, and an event that borrows the agent (`BeforeInvocationEvent<'a>`) has one
//! in [`kinds`]. [`EventKind`] maps a concrete event type of any lifetime to its marker, so the
//! registry needs no `Any` or downcasting.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use async_trait::async_trait;

use crate::hooks::events::*;
use crate::hooks::types::{HookCleanup, HookOptions, HookOrder};

/// A boxed `Send` future, the return type of [`add_async_hook`](HookRegistry::add_async_hook)
/// callbacks.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) mod sealed {
    pub trait Sealed {}
}

type SyncCallback<M> = Arc<dyn for<'a, 'b> Fn(&'b mut <M as HookEvent>::Event<'a>) + Send + Sync>;
type AsyncCallback<M> =
    Arc<dyn for<'a, 'b> Fn(&'b mut <M as HookEvent>::Event<'a>) -> BoxFuture<'b, ()> + Send + Sync>;

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

/// The callbacks registered for one event type.
#[doc(hidden)]
pub struct HookSlot<M: HookEvent> {
    entries: RwLock<Vec<Entry<M>>>,
}

impl<M: HookEvent> Default for HookSlot<M> {
    fn default() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
        }
    }
}

/// A hookable event's registry key (sealed).
pub trait HookEvent: sealed::Sealed + Sized + Send + Sync + 'static {
    /// The event type callbacks receive.
    type Event<'a>: Send;
    /// Whether same-order callbacks run in reverse registration order (the `After*` events).
    #[doc(hidden)]
    const REVERSE: bool;
    #[doc(hidden)]
    fn slot(slots: &Slots) -> &HookSlot<Self>;
}

/// Maps an event type, at any lifetime, to its [`HookEvent`] marker (sealed).
pub trait EventKind: sealed::Sealed {
    /// The marker.
    type Kind: HookEvent;
}

/// A synchronous hook closure for the event marked `M`; implemented for every
/// `Fn(&mut Event)` closure, which is how `add_hook` infers the event from the argument type.
pub trait IntoSyncHook<M: HookEvent> {
    #[doc(hidden)]
    fn into_callback(self) -> SyncCallback<M>;
}

/// A hook implemented as a type (TS class-based hook callbacks).
///
/// Implement it for the events the hook handles and register it with
/// [`add_hook_impl`](HookRegistry::add_hook_impl). For an event that borrows the agent, name the
/// lifetime: `impl<'a> Hook<BeforeInvocationEvent<'a>> for MyHook`.
#[async_trait]
pub trait Hook<E: Send>: Send + Sync + 'static {
    /// Handles one event.
    async fn handle(&self, event: &mut E);
}

macro_rules! hook_events {
    (
        borrowed { $($kind:ident => $event:ident: $field:ident, $reverse:literal;)* }
        owned { $($oevent:ident: $ofield:ident, $oreverse:literal;)* }
    ) => {
        /// Registry keys for the events that borrow the agent; you never name them directly.
        pub mod kinds {
            $(
                #[doc = concat!("The registry key of [`", stringify!($event), "`](crate::hooks::", stringify!($event), ").")]
                #[derive(Debug)]
                pub struct $kind(());
            )*
        }

        /// One slot per event type.
        #[doc(hidden)]
        #[derive(Default)]
        pub struct Slots {
            $($field: HookSlot<kinds::$kind>,)*
            $($ofield: HookSlot<$oevent>,)*
        }

        $(
            impl sealed::Sealed for kinds::$kind {}
            impl HookEvent for kinds::$kind {
                type Event<'a> = $event<'a>;
                const REVERSE: bool = $reverse;
                fn slot(slots: &Slots) -> &HookSlot<Self> {
                    &slots.$field
                }
            }
            impl sealed::Sealed for $event<'_> {}
            impl EventKind for $event<'_> {
                type Kind = kinds::$kind;
            }
            impl<F> IntoSyncHook<kinds::$kind> for F
            where
                F: for<'a, 'b> Fn(&'b mut $event<'a>) + Send + Sync + 'static,
            {
                fn into_callback(self) -> SyncCallback<kinds::$kind> {
                    Arc::new(self)
                }
            }
        )*
        $(
            impl sealed::Sealed for $oevent {}
            impl HookEvent for $oevent {
                type Event<'a> = $oevent;
                const REVERSE: bool = $oreverse;
                fn slot(slots: &Slots) -> &HookSlot<Self> {
                    &slots.$ofield
                }
            }
            impl EventKind for $oevent {
                type Kind = $oevent;
            }
            impl<F> IntoSyncHook<$oevent> for F
            where
                F: for<'b> Fn(&'b mut $oevent) + Send + Sync + 'static,
            {
                fn into_callback(self) -> SyncCallback<$oevent> {
                    Arc::new(self)
                }
            }
        )*
    };
}

hook_events! {
    borrowed {
        Initialized => InitializedEvent: initialized, false;
        BeforeInvocation => BeforeInvocationEvent: before_invocation, false;
        AfterInvocation => AfterInvocationEvent: after_invocation, true;
        MessageAdded => MessageAddedEvent: message_added, false;
        BeforeModelCall => BeforeModelCallEvent: before_model_call, false;
        AfterModelCall => AfterModelCallEvent: after_model_call, true;
        ModelStreamUpdate => ModelStreamUpdateEvent: model_stream_update, false;
        ContentBlock => ContentBlockEvent: content_block, false;
        ModelMessage => ModelMessageEvent: model_message, false;
        AgentResult => AgentResultEvent: agent_result, false;
        Interrupt => InterruptEvent: interrupt, false;
        BeforeTools => BeforeToolsEvent: before_tools, false;
        AfterTools => AfterToolsEvent: after_tools, true;
    }
    owned {
        BeforeToolCallEvent: before_tool_call, false;
        AfterToolCallEvent: after_tool_call, true;
        ToolResultEvent: tool_result, false;
        ToolStreamUpdateEvent: tool_stream_update, false;
    }
}

struct Inner {
    slots: Slots,
    next_id: AtomicU64,
}

/// The hooks registered on an agent (TS `HookRegistry`). Clones share the registrations.
///
/// Three registration forms, each with a `_with` variant taking [`HookOptions`]:
///
/// - [`add_hook`](Self::add_hook): a synchronous closure; the event is inferred from the
///   closure's argument type.
/// - [`add_async_hook`](Self::add_async_hook): a closure returning a boxed future; name the
///   event with a turbofish.
/// - [`add_hook_impl`](Self::add_hook_impl): a type implementing [`Hook`].
///
/// ```
/// use strands::hooks::{BeforeInvocationEvent, BeforeToolCallEvent, HookRegistry};
///
/// let hooks = HookRegistry::new();
/// hooks.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
///     event.agent_mut().app_state().set("seen", true).ok();
/// });
/// hooks.add_async_hook::<BeforeToolCallEvent>(|event| {
///     Box::pin(async move {
///         let _name = event.tool_use().name.clone();
///     })
/// });
/// ```
#[derive(Clone)]
pub struct HookRegistry {
    inner: Arc<Inner>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: Slots::default(),
                next_id: AtomicU64::new(0),
            }),
        }
    }
}

impl HookRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a synchronous hook; the event comes from the closure's argument type.
    pub fn add_hook<M: HookEvent, F: IntoSyncHook<M>>(&self, callback: F) -> HookCleanup {
        self.add_hook_with(HookOptions::default(), callback)
    }

    /// [`add_hook`](Self::add_hook) with options.
    pub fn add_hook_with<M: HookEvent, F: IntoSyncHook<M>>(
        &self,
        options: HookOptions,
        callback: F,
    ) -> HookCleanup {
        self.insert::<M>(options.order, Callback::Sync(callback.into_callback()))
    }

    /// Registers an asynchronous hook for the event `E`.
    pub fn add_async_hook<E: EventKind>(
        &self,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.add_async_hook_with::<E>(HookOptions::default(), callback)
    }

    /// [`add_async_hook`](Self::add_async_hook) with options.
    pub fn add_async_hook_with<E: EventKind>(
        &self,
        options: HookOptions,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.insert::<E::Kind>(options.order, Callback::Async(Arc::new(callback)))
    }

    /// Registers a [`Hook`] implementation for the event `E`.
    pub fn add_hook_impl<E, H>(&self, hook: H) -> HookCleanup
    where
        E: EventKind,
        H: for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    {
        self.add_hook_impl_with::<E, H>(HookOptions::default(), hook)
    }

    /// [`add_hook_impl`](Self::add_hook_impl) with options.
    pub fn add_hook_impl_with<E, H>(&self, options: HookOptions, hook: H) -> HookCleanup
    where
        E: EventKind,
        H: for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    {
        let hook = Arc::new(hook);
        let callback: AsyncCallback<E::Kind> = Arc::new(move |event| {
            let hook = Arc::clone(&hook);
            Box::pin(async move { hook.handle(event).await })
        });
        self.insert::<E::Kind>(options.order, Callback::Async(callback))
    }

    fn insert<M: HookEvent>(&self, order: HookOrder, callback: Callback<M>) -> HookCleanup {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut entries = M::slot(&self.inner.slots)
                .entries
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            let at = entries
                .iter()
                .position(|entry| entry.order > order)
                .unwrap_or(entries.len());
            entries.insert(at, Entry { id, order, callback });
        }
        let inner: Weak<Inner> = Arc::downgrade(&self.inner);
        HookCleanup::new(move || {
            if let Some(inner) = inner.upgrade() {
                M::slot(&inner.slots)
                    .entries
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|entry| entry.id != id);
            }
        })
    }

    fn callbacks<M: HookEvent>(&self) -> Vec<Callback<M>> {
        let entries = M::slot(&self.inner.slots)
            .entries
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if !M::REVERSE {
            return entries.iter().map(|entry| entry.callback.clone()).collect();
        }
        let mut reversed: Vec<(HookOrder, Callback<M>)> = entries
            .iter()
            .rev()
            .map(|entry| (entry.order, entry.callback.clone()))
            .collect();
        reversed.sort_by_key(|(order, _)| *order);
        reversed.into_iter().map(|(_, callback)| callback).collect()
    }

    /// Runs the callbacks for `M` on `event`, in order. The callback list is snapshotted first,
    /// so a callback that registers or removes hooks affects only later events.
    pub(crate) async fn fire<M: HookEvent>(&self, event: &mut M::Event<'_>) {
        for callback in self.callbacks::<M>() {
            match callback {
                Callback::Sync(callback) => callback(event),
                Callback::Async(callback) => callback(event).await,
            }
        }
    }
}

impl fmt::Debug for HookRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookRegistry").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
