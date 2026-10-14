//! The event channel middleware handlers write to (`src/middleware/types.ts`, event side).

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::error::Result;

/// Destination for stage events. Implement this to intercept a stage's event stream.
#[async_trait]
pub trait EventTarget<E>: Send + Sync {
    /// Deliver one event.
    async fn send(&self, event: E) -> Result<()>;
}

/// Handle a middleware handler writes stage events to.
///
/// Event *filtering* works by interposition: an outer handler passes a wrapped sink (see
/// [`EventSink::filter`] / [`EventSink::null`]) to [`super::Next::run`], so events produced by
/// inner layers and the terminal flow through the wrapper.
pub struct EventSink<E> {
    target: Arc<dyn EventTarget<E>>,
}

impl<E> Clone for EventSink<E> {
    fn clone(&self) -> Self {
        Self {
            target: Arc::clone(&self.target),
        }
    }
}

impl<E: Send + 'static> EventSink<E> {
    /// Sink backed by `target`.
    pub fn new(target: Arc<dyn EventTarget<E>>) -> Self {
        Self { target }
    }

    /// Sink that drops every event (suppresses the inner event stream).
    pub fn null() -> Self {
        Self::new(Arc::new(NullTarget))
    }

    /// Sink that collects events into a shared vector, for tests and buffering handlers.
    pub fn collector() -> (Self, Arc<Mutex<Vec<E>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Self::new(Arc::new(CollectTarget {
            events: Arc::clone(&events),
        }));
        (sink, events)
    }

    /// Emit one event.
    pub async fn send(&self, event: E) -> Result<()> {
        self.target.send(event).await
    }

    /// Sink that forwards to `self` only the events matching `predicate`.
    pub fn filter(&self, predicate: impl Fn(&E) -> bool + Send + Sync + 'static) -> Self {
        Self::new(Arc::new(FilterTarget {
            inner: self.clone(),
            predicate: Box::new(predicate),
        }))
    }

    /// Sink that forwards every event through `f` before delivering it to `self`.
    pub fn map(&self, f: impl Fn(E) -> E + Send + Sync + 'static) -> Self {
        Self::new(Arc::new(MapTarget {
            inner: self.clone(),
            f: Box::new(f),
        }))
    }
}

struct NullTarget;

#[async_trait]
impl<E: Send + 'static> EventTarget<E> for NullTarget {
    async fn send(&self, _event: E) -> Result<()> {
        Ok(())
    }
}

struct CollectTarget<E> {
    events: Arc<Mutex<Vec<E>>>,
}

#[async_trait]
impl<E: Send + 'static> EventTarget<E> for CollectTarget<E> {
    async fn send(&self, event: E) -> Result<()> {
        self.events.lock().push(event);
        Ok(())
    }
}

struct FilterTarget<E> {
    inner: EventSink<E>,
    predicate: Box<dyn Fn(&E) -> bool + Send + Sync>,
}

#[async_trait]
impl<E: Send + 'static> EventTarget<E> for FilterTarget<E> {
    async fn send(&self, event: E) -> Result<()> {
        if (self.predicate)(&event) {
            self.inner.send(event).await?;
        }
        Ok(())
    }
}

struct MapTarget<E> {
    inner: EventSink<E>,
    f: Box<dyn Fn(E) -> E + Send + Sync>,
}

#[async_trait]
impl<E: Send + 'static> EventTarget<E> for MapTarget<E> {
    async fn send(&self, event: E) -> Result<()> {
        self.inner.send((self.f)(event)).await
    }
}
