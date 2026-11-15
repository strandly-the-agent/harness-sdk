//! The agent's event stream (TS `agent.stream()`'s async generator).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures::Stream;

use crate::agent::printer::{SharedPrinter, print};
use crate::error::Result;
use crate::types::agent::AgentStreamEvent;

type Slot = Arc<Mutex<Option<AgentStreamEvent>>>;

/// The events of one invocation, ending with [`AgentStreamEvent::AgentResult`].
///
/// The agent loop runs only while the stream is polled, and the stream borrows the agent
/// mutably, so scope it (or drop it) before the next call. An error item ends the stream.
///
/// ```
/// use futures::StreamExt;
/// use strands::testing::MockMessageModel;
/// use strands::types::messages::TextBlock;
/// use strands::{Agent, AgentStreamEvent};
///
/// # tokio_test_block_on(async {
/// let model = MockMessageModel::new();
/// model.add_turn(vec![TextBlock::new("Hello!").into()]);
/// let mut agent = Agent::builder().model(model).build()?;
/// let mut text = String::new();
/// {
///     let mut stream = agent.stream("Hi");
///     while let Some(event) = stream.next().await {
///         if let AgentStreamEvent::ModelStreamUpdate(update) = event? {
///             text.extend(update.text_delta());
///         }
///     }
/// }
/// assert_eq!(text, "Hello!");
/// # Ok::<(), strands::Error>(())
/// # }).unwrap();
/// # fn tokio_test_block_on<F: std::future::Future>(f: F) -> F::Output {
/// #     tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
/// # }
/// ```
#[must_use = "streams do nothing unless polled"]
pub struct AgentStream<'a> {
    inner: Driven<'a, Result<()>>,
}

impl<'a> AgentStream<'a> {
    /// A stream driving `run`, which hands its events to the [`Emitter`] it is given.
    pub(crate) fn new<F>(printer: Option<SharedPrinter>, run: impl FnOnce(Emitter) -> F) -> Self
    where
        F: Future<Output = Result<()>> + Send + 'a,
    {
        Self {
            inner: Driven::new(printer, run),
        }
    }
}

impl Stream for AgentStream<'_> {
    type Item = Result<AgentStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(DrivenItem::Event(event))) => Poll::Ready(Some(Ok(event))),
            Poll::Ready(Some(DrivenItem::Done(Err(error)))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(Some(DrivenItem::Done(Ok(())))) | Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// An item of a [`Driven`] stream.
pub(crate) enum DrivenItem<T> {
    Event(AgentStreamEvent),
    /// The future's output; the last item.
    Done(T),
}

/// A future that emits events through an [`Emitter`], as a stream of its events and then its
/// output.
pub(crate) struct Driven<'a, T> {
    run: Option<Pin<Box<dyn Future<Output = T> + Send + 'a>>>,
    slot: Slot,
}

impl<'a, T> Driven<'a, T> {
    pub(crate) fn new<F>(printer: Option<SharedPrinter>, run: impl FnOnce(Emitter) -> F) -> Self
    where
        F: Future<Output = T> + Send + 'a,
    {
        let slot = Slot::default();
        let emitter = Emitter {
            slot: Arc::clone(&slot),
            printer,
        };
        Self {
            run: Some(Box::pin(run(emitter))),
            slot,
        }
    }

    fn take(&self) -> Option<AgentStreamEvent> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl<T> Stream for Driven<'_, T> {
    type Item = DrivenItem<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // An event left in the slot by the last poll comes before the output.
        if let Some(event) = self.take() {
            return Poll::Ready(Some(DrivenItem::Event(event)));
        }
        let Some(run) = self.run.as_mut() else {
            return Poll::Ready(None);
        };
        match run.as_mut().poll(cx) {
            Poll::Ready(output) => {
                self.run = None;
                Poll::Ready(Some(DrivenItem::Done(output)))
            }
            Poll::Pending => match self.take() {
                Some(event) => Poll::Ready(Some(DrivenItem::Event(event))),
                None => Poll::Pending,
            },
        }
    }
}

impl fmt::Debug for AgentStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentStream")
            .field("done", &self.inner.run.is_none())
            .finish_non_exhaustive()
    }
}

/// Hands events from the agent loop to its [`AgentStream`], one at a time.
pub(crate) struct Emitter {
    slot: Slot,
    printer: Option<SharedPrinter>,
}

impl Emitter {
    /// Yields an event that was printed already (one a middleware chain passes on).
    pub(crate) fn forward(&self, event: AgentStreamEvent) -> impl Future<Output = ()> + Send + '_ {
        Yield {
            slot: &self.slot,
            event: Some(event),
        }
    }

    /// Yields `event` to the stream's consumer and resumes once it was taken.
    ///
    /// The loop must await this directly (never inside a `select!` or `join!`): the returned
    /// future parks without a waker, relying on the stream returning the event to its poller.
    pub(crate) fn emit(&self, event: impl Into<AgentStreamEvent>) -> impl Future<Output = ()> + Send + '_ {
        let event = event.into();
        print(self.printer.as_ref(), &event);
        Yield {
            slot: &self.slot,
            event: Some(event),
        }
    }
}

struct Yield<'s> {
    slot: &'s Slot,
    event: Option<AgentStreamEvent>,
}

impl Future for Yield<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let mut slot = this.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(event) = this.event.take() {
            *slot = Some(event);
            return Poll::Pending;
        }
        if slot.is_some() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
