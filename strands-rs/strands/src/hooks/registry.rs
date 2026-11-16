//! The typed hook registry (TS `hooks/registry.ts`).
//!
//! Each event type has a marker implementing [`HookEvent`]: a `'static` event is its own marker,
//! and an event borrowing the agent (`BeforeInvocationEvent<'a>`) uses its `'static`
//! instantiation. [`EventKind`] maps an event type of any lifetime to its marker, so callers name
//! events the natural way: `add_async_hook::<BeforeInvocationEvent>(..)`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use futures::future::BoxFuture;

use crate::error::{Error, Result};
use crate::interrupt::Interrupt;

use super::events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, ContentBlockEvent,
    InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent, ModelStreamUpdateEvent,
    ToolResultEvent, ToolStreamUpdateEvent,
};
use super::types::{HookCallbackOptions, HookCleanup, HookOrder};
use slot::Slot;

/// A synchronous hook callback for marker `M`.
pub type SyncHookFn<M> = Arc<dyn for<'a, 'b> Fn(&'b mut <M as HookEvent>::Event<'a>) + Send + Sync>;

/// An asynchronous hook callback for marker `M`.
pub type AsyncHookFn<M> =
    Arc<dyn for<'a, 'b> Fn(&'b mut <M as HookEvent>::Event<'a>) -> BoxFuture<'b, ()> + Send + Sync>;

pub(crate) mod sealed {
    use super::{Error, HookEvent, Interrupt, Slot, Slots};

    /// Seals [`super::EventKind`].
    pub trait SealedKind {}

    /// Seals [`super::IntoSyncHook`].
    pub trait SealedHook<M: HookEvent> {}

    /// Registry plumbing behind [`super::HookEvent`]; implemented only in this crate.
    pub trait Sealed: Sized + 'static {
        /// Same-order callbacks run in reverse registration order (TS `_shouldReverseCallbacks`).
        const REVERSE: bool;

        #[doc(hidden)]
        fn slot(slots: &Slots) -> &Slot<Self>
        where
            Self: super::HookEvent;

        #[doc(hidden)]
        fn interrupts<'e>(_event: &'e <Self as super::HookEvent>::Event<'_>) -> &'e [Interrupt]
        where
            Self: super::HookEvent,
        {
            &[]
        }

        /// The error a callback failed the event with (interventions with `on_error: throw`).
        #[doc(hidden)]
        fn take_failure(_event: &mut <Self as super::HookEvent>::Event<'_>) -> Option<Error>
        where
            Self: super::HookEvent,
        {
            None
        }
    }
}

/// The registry marker of one hook event type. Sealed: the 17 SDK events implement it.
pub trait HookEvent: sealed::Sealed {
    /// The event type, at any borrow lifetime.
    type Event<'a>: Send;
}

/// Maps a concrete event type, at any lifetime, to its [`HookEvent`] marker. Sealed.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a hook event",
    note = "name one of the event types in `strands::hooks`, for example `add_async_hook::<BeforeToolCallEvent>(..)`"
)]
pub trait EventKind: sealed::SealedKind + Send {
    /// The marker.
    type Kind: HookEvent;
}

/// A callback type [`HookRegistry::add_hook`] accepts for marker `M`: a closure taking
/// `&mut Event`. The event type is inferred from the closure's annotated argument. Sealed.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a hook callback for this event",
    note = "annotate the event: `|event: &mut BeforeToolCallEvent| ..`"
)]
pub trait IntoSyncHook<M: HookEvent>: sealed::SealedHook<M> {
    #[doc(hidden)]
    fn into_hook_fn(self) -> SyncHookFn<M>;
}

/// A hook implemented as a type (TS `HookProvider`-style objects).
///
/// Impls on events that borrow the agent name the lifetime:
///
/// ```
/// use strands::hooks::{BeforeInvocationEvent, Hook};
///
/// struct Audit;
///
/// #[strands::async_trait]
/// impl<'x> Hook<BeforeInvocationEvent<'x>> for Audit {
///     async fn handle(&self, event: &mut BeforeInvocationEvent<'x>) {
///         let _ = event.agent().id();
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Hook<E: Send>: Send + Sync + 'static {
    /// Handles the event.
    async fn handle(&self, event: &mut E);
}

enum Callback<M: HookEvent> {
    Sync(SyncHookFn<M>),
    Async(AsyncHookFn<M>),
}

impl<M: HookEvent> Clone for Callback<M> {
    fn clone(&self) -> Self {
        match self {
            Self::Sync(f) => Self::Sync(Arc::clone(f)),
            Self::Async(f) => Self::Async(Arc::clone(f)),
        }
    }
}

struct Entry<M: HookEvent> {
    id: u64,
    order: HookOrder,
    callback: Callback<M>,
}

mod slot {
    use super::{Callback, Entry, HookEvent, PoisonError, RwLock};

    /// The callbacks of one event, sorted by order (same order keeps registration order).
    pub struct Slot<M: HookEvent> {
        entries: RwLock<Vec<Entry<M>>>,
    }

    impl<M: HookEvent> std::fmt::Debug for Slot<M> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Slot").finish_non_exhaustive()
        }
    }

    impl<M: HookEvent> Default for Slot<M> {
        fn default() -> Self {
            Self {
                entries: RwLock::new(Vec::new()),
            }
        }
    }

    impl<M: HookEvent> Slot<M> {
        pub(super) fn insert(&self, entry: Entry<M>) {
            let mut entries = self.entries.write().unwrap_or_else(PoisonError::into_inner);
            let at = entries
                .iter()
                .position(|e| e.order > entry.order)
                .unwrap_or(entries.len());
            entries.insert(at, entry);
        }

        pub(super) fn remove(&self, id: u64) {
            self.entries
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|e| e.id != id);
        }

        pub(super) fn callbacks(&self) -> Vec<Callback<M>> {
            let entries = self.entries.read().unwrap_or_else(PoisonError::into_inner);
            if !M::REVERSE {
                return entries.iter().map(|e| e.callback.clone()).collect();
            }
            let mut reversed: Vec<&Entry<M>> = entries.iter().rev().collect();
            reversed.sort_by_key(|e| e.order);
            reversed.into_iter().map(|e| e.callback.clone()).collect()
        }

        pub(super) fn len(&self) -> usize {
            self.entries.read().unwrap_or_else(PoisonError::into_inner).len()
        }
    }
}

macro_rules! hook_events {
    ($( $field:ident: $marker:ty => $event:ident $(<$lt:lifetime>)?, reverse: $reverse:literal $(, interrupts: $interrupts:ident)? $(, failure: $failure:ident)? ; )*) => {
        mod slots {
            use super::*;

            /// Per-event callback lists.
            #[derive(Debug, Default)]
            pub struct Slots {
                pub(super) next_id: AtomicU64,
                $( pub(super) $field: Slot<$marker>, )*
            }
        }

        use slots::Slots;

        $(
            impl sealed::Sealed for $marker {
                const REVERSE: bool = $reverse;

                fn slot(slots: &Slots) -> &Slot<Self> {
                    &slots.$field
                }

                $(
                    fn interrupts<'e>(event: &'e <Self as HookEvent>::Event<'_>) -> &'e [Interrupt] {
                        event.$interrupts()
                    }
                )?

                $(
                    fn take_failure(event: &mut <Self as HookEvent>::Event<'_>) -> Option<Error> {
                        event.$failure()
                    }
                )?
            }

            impl HookEvent for $marker {
                type Event<'a> = $event $(<$lt>)?;
            }

            impl<'a> sealed::SealedKind for $event $(<$lt>)? {}

            impl<'a> EventKind for $event $(<$lt>)? {
                type Kind = $marker;
            }

            impl<F> sealed::SealedHook<$marker> for F
            where
                F: for<'a, 'b> Fn(&'b mut $event $(<$lt>)?) + Send + Sync + 'static,
            {
            }

            impl<F> IntoSyncHook<$marker> for F
            where
                F: for<'a, 'b> Fn(&'b mut $event $(<$lt>)?) + Send + Sync + 'static,
            {
                fn into_hook_fn(self) -> SyncHookFn<$marker> {
                    Arc::new(self)
                }
            }
        )*

        impl Slots {
            fn total(&self) -> usize {
                0 $( + self.$field.len() )*
            }
        }
    };
}

hook_events! {
    initialized: InitializedEvent<'static> => InitializedEvent<'a>, reverse: false;
    before_invocation: BeforeInvocationEvent<'static> => BeforeInvocationEvent<'a>, reverse: false, failure: take_failure;
    after_invocation: AfterInvocationEvent<'static> => AfterInvocationEvent<'a>, reverse: true;
    message_added: MessageAddedEvent<'static> => MessageAddedEvent<'a>, reverse: false;
    before_tool_call: BeforeToolCallEvent => BeforeToolCallEvent, reverse: false, interrupts: collected_interrupts, failure: take_failure;
    after_tool_call: AfterToolCallEvent => AfterToolCallEvent, reverse: true, failure: take_failure;
    before_model_call: BeforeModelCallEvent<'static> => BeforeModelCallEvent<'a>, reverse: false, failure: take_failure;
    after_model_call: AfterModelCallEvent<'static> => AfterModelCallEvent<'a>, reverse: true, failure: take_failure;
    model_stream_update: ModelStreamUpdateEvent<'static> => ModelStreamUpdateEvent<'a>, reverse: false;
    content_block: ContentBlockEvent<'static> => ContentBlockEvent<'a>, reverse: false;
    model_message: ModelMessageEvent<'static> => ModelMessageEvent<'a>, reverse: false;
    tool_result: ToolResultEvent => ToolResultEvent, reverse: false;
    tool_stream_update: ToolStreamUpdateEvent => ToolStreamUpdateEvent, reverse: false;
    agent_result: AgentResultEvent<'static> => AgentResultEvent<'a>, reverse: false;
    interrupt: InterruptEvent<'static> => InterruptEvent<'a>, reverse: false;
    before_tools: BeforeToolsEvent<'static> => BeforeToolsEvent<'a>, reverse: false, interrupts: collected_interrupts;
    after_tools: AfterToolsEvent<'static> => AfterToolsEvent<'a>, reverse: true;
}

/// The agent's hook callbacks (TS `HookRegistry`).
///
/// A cheap-clone handle; clones share the same callbacks. Registration takes `&self`, so hooks
/// can be added while the agent is shared. Callbacks are snapshotted under a short lock and run
/// outside it.
///
/// ```
/// use strands::hooks::{BeforeToolCallEvent, HookCallbackOptions, HookOrder, HookRegistry};
///
/// let registry = HookRegistry::new();
/// registry.add_hook(|event: &mut BeforeToolCallEvent| {
///     if event.tool_use().name() == "shell" {
///         event.set_cancel("shell is disabled");
///     }
/// });
/// let cleanup = registry.add_async_hook_with::<BeforeToolCallEvent>(
///     HookCallbackOptions { order: HookOrder::SDK_FIRST },
///     |event| Box::pin(async move { let _ = event.tool_use(); }),
/// );
/// cleanup.remove();
/// ```
#[derive(Clone, Default)]
pub struct HookRegistry {
    slots: Arc<Slots>,
}

impl HookRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a synchronous callback; the event type comes from the closure's argument type.
    pub fn add_hook<M: HookEvent, F: IntoSyncHook<M>>(&self, callback: F) -> HookCleanup {
        self.add_hook_with(HookCallbackOptions::default(), callback)
    }

    /// [`Self::add_hook`] with options.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its options by value like every other options parameter"
    )]
    pub fn add_hook_with<M: HookEvent, F: IntoSyncHook<M>>(
        &self,
        options: HookCallbackOptions,
        callback: F,
    ) -> HookCleanup {
        self.insert::<M>(options.order, Callback::Sync(callback.into_hook_fn()))
    }

    /// Registers an asynchronous callback for event `E`, returning a boxed future:
    /// `add_async_hook::<BeforeToolCallEvent>(|event| Box::pin(async move { .. }))`.
    pub fn add_async_hook<E: EventKind>(
        &self,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.add_async_hook_with::<E>(HookCallbackOptions::default(), callback)
    }

    /// [`Self::add_async_hook`] with options.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its options by value like every other options parameter"
    )]
    pub fn add_async_hook_with<E: EventKind>(
        &self,
        options: HookCallbackOptions,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.insert::<E::Kind>(options.order, Callback::Async(Arc::new(callback)))
    }

    /// Registers a [`Hook`] implementation for event `E`.
    pub fn add_hook_impl<E: EventKind>(
        &self,
        hook: impl for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    ) -> HookCleanup {
        self.add_hook_impl_with::<E>(HookCallbackOptions::default(), hook)
    }

    /// [`Self::add_hook_impl`] with options.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its options by value like every other options parameter"
    )]
    pub fn add_hook_impl_with<E: EventKind>(
        &self,
        options: HookCallbackOptions,
        hook: impl for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    ) -> HookCleanup {
        let hook = Arc::new(hook);
        let callback: AsyncHookFn<E::Kind> = Arc::new(move |event| {
            let hook = Arc::clone(&hook);
            Box::pin(async move { hook.handle(event).await })
        });
        self.insert::<E::Kind>(options.order, Callback::Async(callback))
    }

    /// Whether no callbacks are registered.
    pub fn is_empty(&self) -> bool {
        self.slots.total() == 0
    }

    #[cfg(test)]
    pub(crate) fn has_callbacks<M: HookEvent>(&self) -> bool {
        !M::slot(&self.slots).callbacks().is_empty()
    }

    #[cfg(test)]
    pub(crate) fn callback_count<M: HookEvent>(&self) -> usize {
        M::slot(&self.slots).callbacks().len()
    }

    fn insert<M: HookEvent>(&self, order: HookOrder, callback: Callback<M>) -> HookCleanup {
        let id = self.slots.next_id.fetch_add(1, Ordering::Relaxed);
        M::slot(&self.slots).insert(Entry { id, order, callback });
        let slots: Weak<Slots> = Arc::downgrade(&self.slots);
        HookCleanup::new(move || {
            if let Some(slots) = slots.upgrade() {
                M::slot(&slots).remove(id);
            }
        })
    }

    /// Runs the callbacks of marker `M` in order (TS `invokeCallbacks`).
    ///
    /// Interrupts raised by the callbacks stay on the event.
    ///
    /// # Errors
    ///
    /// Two raised interrupts share a name (TS text
    /// `interrupt_names=<a, b> | duplicate interrupt names`).
    ///
    /// Boxed so callers' futures stay provably `Send`: rustc can't check auto traits through the
    /// `Event<'a>` projection of a borrowing event inside an `async fn`.
    pub(crate) fn invoke<'a, 'e: 'a, M: HookEvent>(
        &'a self,
        event: &'a mut M::Event<'e>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            for callback in M::slot(&self.slots).callbacks() {
                match callback {
                    Callback::Sync(f) => f(event),
                    Callback::Async(f) => f(event).await,
                }
                if let Some(error) = M::take_failure(event) {
                    return Err(error);
                }
            }
            check_duplicate_interrupts(M::interrupts(event))
        })
    }
}

fn check_duplicate_interrupts(interrupts: &[Interrupt]) -> Result<()> {
    let mut seen: Vec<&str> = Vec::new();
    let mut duplicates: Vec<&str> = Vec::new();
    for interrupt in interrupts {
        let name = interrupt.name();
        if seen.contains(&name) && !duplicates.contains(&name) {
            duplicates.push(name);
        }
        seen.push(name);
    }
    if duplicates.is_empty() {
        return Ok(());
    }
    Err(Error::other(format!(
        "interrupt_names=<{}> | duplicate interrupt names",
        duplicates.join(", ")
    )))
}

impl fmt::Debug for HookRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookRegistry")
            .field("callbacks", &self.slots.total())
            .finish()
    }
}

#[cfg(test)]
mod tests;
