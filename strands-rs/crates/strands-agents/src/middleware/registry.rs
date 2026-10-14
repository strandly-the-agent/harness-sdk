//! `MiddlewareRegistry` and the handler/next plumbing (`src/middleware/registry.ts`).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures::future::BoxFuture;
use parking_lot::RwLock;

use super::{EventSink, Stage};
use crate::error::Result;

/// Which phase a handler was registered in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseKind {
    /// Transforms the context on the way in; composed outermost.
    Input,
    /// Transforms the result on the way out; composed around [`PhaseKind::Wrap`].
    Output,
    /// Owns the call; composed innermost, closest to the terminal operation.
    Wrap,
}

impl PhaseKind {
    /// Compose order: input (outermost) → output → wrap (innermost).
    fn order(self) -> u8 {
        match self {
            PhaseKind::Input => 0,
            PhaseKind::Output => 1,
            PhaseKind::Wrap => 2,
        }
    }
}

/// A wrap handler: it receives the context, an event sink, and the rest of the chain.
#[async_trait]
pub trait Middleware<S: Stage>: Send + Sync {
    /// Run this handler. Not calling `next` short-circuits the rest of the chain.
    async fn call(
        &self,
        context: S::Context,
        events: &EventSink<S::Event>,
        next: Next<'_, S>,
    ) -> Result<S::Result>;
}

/// The operation at the end of a chain (the real model call / tool call).
#[async_trait]
pub trait Terminal<S: Stage>: Send + Sync {
    /// Perform the operation.
    async fn call(&self, context: S::Context, events: &EventSink<S::Event>) -> Result<S::Result>;
}

#[async_trait]
impl<S, F, Fut> Terminal<S> for F
where
    S: Stage,
    F: Fn(S::Context, EventSink<S::Event>) -> Fut + Send + Sync,
    Fut: Future<Output = Result<S::Result>> + Send,
{
    async fn call(&self, context: S::Context, events: &EventSink<S::Event>) -> Result<S::Result> {
        (self)(context, events.clone()).await
    }
}

/// The remainder of a middleware chain. Cloneable, so a handler may call it more than once.
pub struct Next<'a, S: Stage> {
    handlers: &'a [HandlerRef<S>],
    terminal: &'a dyn Terminal<S>,
}

impl<S: Stage> Clone for Next<'_, S> {
    fn clone(&self) -> Self {
        Self {
            handlers: self.handlers,
            terminal: self.terminal,
        }
    }
}

impl<S: Stage> Next<'_, S> {
    /// Run the rest of the chain, ending at the terminal operation.
    pub async fn run(&self, context: S::Context, events: &EventSink<S::Event>) -> Result<S::Result> {
        match self.handlers.split_first() {
            Some((handler, rest)) => {
                let next = Next {
                    handlers: rest,
                    terminal: self.terminal,
                };
                handler.call(context, events, next).await
            }
            None => self.terminal.call(context, events).await,
        }
    }
}

type HandlerRef<S> = std::sync::Arc<dyn Middleware<S>>;

/// Wraps a closure as a wrap-phase [`Middleware`]; see [`wrap_fn`].
pub struct WrapFn<F>(F);

#[async_trait]
impl<S, F> Middleware<S> for WrapFn<F>
where
    S: Stage,
    F: for<'a> Fn(S::Context, EventSink<S::Event>, Next<'a, S>) -> BoxFuture<'a, Result<S::Result>>
        + Send
        + Sync,
{
    async fn call(
        &self,
        context: S::Context,
        events: &EventSink<S::Event>,
        next: Next<'_, S>,
    ) -> Result<S::Result> {
        (self.0)(context, events.clone(), next).await
    }
}

/// Wrap-phase middleware from a closure: `wrap_fn(|ctx, sink, next| Box::pin(async move { .. }))`.
pub fn wrap_fn<S, F>(f: F) -> WrapFn<F>
where
    S: Stage,
    F: for<'a> Fn(S::Context, EventSink<S::Event>, Next<'a, S>) -> BoxFuture<'a, Result<S::Result>>
        + Send
        + Sync,
{
    WrapFn(f)
}

/// Handle returned by every registration; pass it to
/// [`MiddlewareRegistry::remove`] (the TS cleanup closure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MiddlewareHandle {
    stage: TypeId,
    id: u64,
}

struct Entry {
    id: u64,
    phase: PhaseKind,
    handler: Box<dyn Any + Send + Sync>,
}

/// Middleware handlers for every stage, keyed by stage type.
#[derive(Default)]
pub struct MiddlewareRegistry {
    entries: RwLock<HashMap<TypeId, Vec<Entry>>>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for MiddlewareRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareRegistry")
            .field("stages", &self.entries.read().len())
            .finish()
    }
}

impl MiddlewareRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a wrap-phase handler (the bare-stage TS overload).
    pub fn add<S: Stage>(&self, handler: impl Middleware<S> + 'static) -> MiddlewareHandle {
        self.push::<S>(PhaseKind::Wrap, std::sync::Arc::new(handler))
    }

    /// Register an input-phase handler: transforms the context before inner layers see it.
    pub fn add_input<S: Stage>(
        &self,
        handler: impl Fn(S::Context) -> Result<S::Context> + Send + Sync + 'static,
    ) -> MiddlewareHandle {
        self.push::<S>(PhaseKind::Input, std::sync::Arc::new(InputAdapter(handler)))
    }

    /// Register an output-phase handler: transforms the result on the way out.
    pub fn add_output<S: Stage>(
        &self,
        handler: impl Fn(S::Result) -> Result<S::Result> + Send + Sync + 'static,
    ) -> MiddlewareHandle {
        self.push::<S>(PhaseKind::Output, std::sync::Arc::new(OutputAdapter(handler)))
    }

    /// Remove a previously registered handler. A no-op if it is already gone.
    pub fn remove(&self, handle: MiddlewareHandle) {
        let mut entries = self.entries.write();
        if let Some(stage) = entries.get_mut(&handle.stage) {
            if let Some(index) = stage.iter().position(|e| e.id == handle.id) {
                stage.remove(index);
            }
        }
    }

    /// Whether any handler is registered for `S`.
    pub fn has<S: Stage>(&self) -> bool {
        self.entries
            .read()
            .get(&TypeId::of::<S>())
            .is_some_and(|v| !v.is_empty())
    }

    /// Run `terminal` through the handlers registered for `S`.
    pub async fn invoke<S: Stage>(
        &self,
        context: S::Context,
        events: &EventSink<S::Event>,
        terminal: &dyn Terminal<S>,
    ) -> Result<S::Result> {
        let handlers = self.compose::<S>();
        let next = Next {
            handlers: &handlers,
            terminal,
        };
        next.run(context, events).await
    }

    /// The handlers for `S` in execution order (outermost first).
    fn compose<S: Stage>(&self) -> Vec<HandlerRef<S>> {
        let entries = self.entries.read();
        let Some(stage) = entries.get(&TypeId::of::<S>()) else {
            return Vec::new();
        };
        let mut ordered: Vec<&Entry> = stage.iter().collect();
        ordered.sort_by_key(|e| e.phase.order());
        ordered
            .into_iter()
            .filter_map(|e| e.handler.downcast_ref::<HandlerRef<S>>().cloned())
            .collect()
    }

    fn push<S: Stage>(&self, phase: PhaseKind, handler: HandlerRef<S>) -> MiddlewareHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.entries
            .write()
            .entry(TypeId::of::<S>())
            .or_default()
            .push(Entry {
                id,
                phase,
                handler: Box::new(handler),
            });
        MiddlewareHandle {
            stage: TypeId::of::<S>(),
            id,
        }
    }
}

struct InputAdapter<F>(F);

#[async_trait]
impl<S, F> Middleware<S> for InputAdapter<F>
where
    S: Stage,
    F: Fn(S::Context) -> Result<S::Context> + Send + Sync,
{
    async fn call(
        &self,
        context: S::Context,
        events: &EventSink<S::Event>,
        next: Next<'_, S>,
    ) -> Result<S::Result> {
        next.run((self.0)(context)?, events).await
    }
}

struct OutputAdapter<F>(F);

#[async_trait]
impl<S, F> Middleware<S> for OutputAdapter<F>
where
    S: Stage,
    F: Fn(S::Result) -> Result<S::Result> + Send + Sync,
{
    async fn call(
        &self,
        context: S::Context,
        events: &EventSink<S::Event>,
        next: Next<'_, S>,
    ) -> Result<S::Result> {
        (self.0)(next.run(context, events).await?)
    }
}

#[path = "tests.rs"]
mod tests_impl;
