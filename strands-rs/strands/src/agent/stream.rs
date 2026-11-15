//! The stream an invocation returns (design: `AgentStream`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures::future::BoxFuture;
use futures::{Stream, StreamExt};

use crate::agent::stream_event::AgentStreamEvent;
use crate::error::{DataError, Result};
use crate::types::agent::AgentResult;

/// The events of one invocation, in the order the agent produces them (TS the async generator
/// `Agent.stream` returns).
///
/// The agent loop runs as the stream is polled: it pauses at each event until the next poll, and
/// dropping the stream stops it. The stream borrows the agent mutably, so scope it (or drop it)
/// before using the agent again. The last item of a completed invocation is usually
/// [`AgentStreamEvent::AgentResult`]; a failed one ends with the error.
///
/// ```
/// use futures::StreamExt;
/// use strands::{Agent, AgentStreamEvent, ContentBlock};
/// use strands::testing::MockModel;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), strands::Error> {
/// let model = MockModel::new();
/// model.add_turn(vec![ContentBlock::from("Hello!")]);
/// let mut agent = Agent::builder().model(model).printer(false).build()?;
/// let mut text = String::new();
/// {
///     let mut stream = agent.stream("Hi");
///     while let Some(event) = stream.next().await {
///         if let AgentStreamEvent::ModelStreamUpdate(update) = event? {
///             text.push_str(update.text_delta().unwrap_or_default());
///         }
///     }
/// }
/// assert_eq!(text, "Hello!");
/// # Ok(())
/// # }
/// ```
#[must_use = "streams do nothing unless polled"]
pub struct AgentStream<'a> {
    inner: Pin<Box<dyn Stream<Item = Result<AgentStreamEvent>> + Send + 'a>>,
    result: Arc<Mutex<Option<AgentResult>>>,
}

impl<'a> AgentStream<'a> {
    /// Runs `run` as a generator: each [`Yielder::emit`] becomes one stream item, and the result
    /// it returns is kept for [`AgentStream::into_result`].
    pub(crate) fn generate<F>(run: impl FnOnce(Yielder) -> F) -> Self
    where
        F: Future<Output = Result<AgentResult>> + Send + 'a,
    {
        let yielder = Yielder::default();
        let result = Arc::new(Mutex::new(None));
        let generator = Generator {
            slot: Arc::clone(&yielder.slot),
            result: Arc::clone(&result),
            future: Some(Box::pin(run(yielder))),
        };
        Self {
            inner: Box::pin(generator),
            result,
        }
    }

    /// Drains the stream and returns the invocation's result (TS `invoke`).
    pub(crate) async fn into_result(mut self) -> Result<AgentResult> {
        while let Some(item) = self.inner.next().await {
            item?;
        }
        self.result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| DataError::new("the agent stream ended without a result").into())
    }
}

impl Stream for AgentStream<'_> {
    type Item = Result<AgentStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl fmt::Debug for AgentStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentStream").finish_non_exhaustive()
    }
}

type Slot = Arc<Mutex<Option<AgentStreamEvent>>>;

/// Hands events from the agent loop to its [`AgentStream`].
#[derive(Clone, Default)]
pub(crate) struct Yielder {
    slot: Slot,
}

impl Yielder {
    /// Hands `event` to the stream and waits until the consumer polls for the next one.
    pub(crate) async fn emit(&self, event: AgentStreamEvent) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(event);
        YieldOnce(false).await;
    }
}

/// Pending once, then ready: the stream returns the emitted event in between.
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        Poll::Pending
    }
}

struct Generator<'a> {
    slot: Slot,
    result: Arc<Mutex<Option<AgentResult>>>,
    future: Option<BoxFuture<'a, Result<AgentResult>>>,
}

impl Stream for Generator<'_> {
    type Item = Result<AgentStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(future) = self.future.as_mut() else {
            return Poll::Ready(None);
        };
        let poll = future.as_mut().poll(cx);
        if let Some(event) = self.slot.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Poll::Ready(Some(Ok(event)));
        }
        match poll {
            Poll::Pending => Poll::Pending,
            Poll::Ready(outcome) => {
                self.future = None;
                match outcome {
                    Ok(result) => {
                        *self.result.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
                        Poll::Ready(None)
                    }
                    Err(error) => Poll::Ready(Some(Err(error))),
                }
            }
        }
    }
}
