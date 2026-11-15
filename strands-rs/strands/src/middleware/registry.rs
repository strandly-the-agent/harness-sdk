//! The middleware registry (TS `middleware/registry.ts`).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use super::types::{
    Middleware, MiddlewareNext, MiddlewarePhaseKind, MiddlewareStage, MiddlewareStream, WrapFn,
};

#[expect(
    clippy::disallowed_types,
    reason = "handlers of every stage share one list; the public API stays typed by stage"
)]
type ErasedHandler = Arc<dyn std::any::Any + Send + Sync>;

#[expect(
    clippy::disallowed_types,
    reason = "a stage is identified by its type, as TS identifies the stage token by reference"
)]
type StageId = std::any::TypeId;

#[expect(
    clippy::disallowed_types,
    reason = "a stage is identified by its type, as TS identifies the stage token by reference"
)]
fn stage_id<S: MiddlewareStage>() -> StageId {
    std::any::TypeId::of::<S>()
}

struct Entry {
    id: u64,
    stage: StageId,
    phase: MiddlewarePhaseKind,
    handler: ErasedHandler,
}

#[derive(Default)]
struct Inner {
    next_id: AtomicU64,
    entries: RwLock<Vec<Entry>>,
}

/// Stores middleware handlers by stage and composes them into chains (TS `MiddlewareRegistry`).
///
/// A cheap-clone handle: clones share the handlers. A chain sees the handlers registered when it
/// was composed.
#[derive(Clone, Default)]
pub struct MiddlewareRegistry {
    inner: Arc<Inner>,
}

impl MiddlewareRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `middleware`, after the handlers of its stage and phase registered before it.
    pub fn add<S: MiddlewareStage>(&self, middleware: Middleware<S>) -> MiddlewareCleanup {
        let (phase, handler) = middleware.into_parts();
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Entry {
                id,
                stage: stage_id::<S>(),
                phase,
                handler: Arc::new(handler),
            });
        MiddlewareCleanup {
            registry: Arc::downgrade(&self.inner),
            id,
        }
    }

    /// The chain of stage `S` around `terminal`, ordered by phase (Input, Output, Wrap) and then
    /// by registration (TS `compose`).
    pub fn compose<'a, S: MiddlewareStage>(
        &self,
        terminal: impl Fn(S::Context) -> MiddlewareStream<'a, S> + Send + Sync + 'a,
    ) -> MiddlewareNext<'a, S> {
        MiddlewareNext::new(self.handlers::<S>().into(), terminal)
    }

    /// Composes the chain of stage `S` around `terminal` and runs it with `context` (TS
    /// `invoke`).
    pub fn invoke<'a, S: MiddlewareStage>(
        &self,
        context: S::Context,
        terminal: impl Fn(S::Context) -> MiddlewareStream<'a, S> + Send + Sync + 'a,
    ) -> MiddlewareStream<'a, S> {
        self.compose::<S>(terminal).run(context)
    }

    /// Whether stage `S` has any handler.
    pub(crate) fn has<S: MiddlewareStage>(&self) -> bool {
        let stage = stage_id::<S>();
        self.read().iter().any(|entry| entry.stage == stage)
    }

    fn handlers<S: MiddlewareStage>(&self) -> Vec<WrapFn<S>> {
        let stage = stage_id::<S>();
        let entries = self.read();
        let mut handlers: Vec<(MiddlewarePhaseKind, WrapFn<S>)> = entries
            .iter()
            .filter(|entry| entry.stage == stage)
            .filter_map(|entry| {
                let handler = entry.handler.downcast_ref::<WrapFn<S>>()?;
                Some((entry.phase, Arc::clone(handler)))
            })
            .collect();
        drop(entries);
        handlers.sort_by_key(|(phase, _)| phase.rank());
        handlers.into_iter().map(|(_, handler)| handler).collect()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<Entry>> {
        self.inner.entries.read().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for MiddlewareRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MiddlewareRegistry")
            .field("handlers", &self.read().len())
            .finish()
    }
}

/// Removes a registered middleware handler (the TS `addMiddleware` cleanup function).
///
/// Dropping it keeps the handler registered; call [`MiddlewareCleanup::remove`] to unregister.
pub struct MiddlewareCleanup {
    registry: Weak<Inner>,
    id: u64,
}

impl MiddlewareCleanup {
    /// Unregisters the handler. A no-op when it was removed already or the registry is gone.
    pub fn remove(self) {
        let Some(inner) = self.registry.upgrade() else {
            return;
        };
        inner
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|entry| entry.id != self.id);
    }
}

impl fmt::Debug for MiddlewareCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MiddlewareCleanup").finish_non_exhaustive()
    }
}
