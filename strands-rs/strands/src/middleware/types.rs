//! Stage tokens, phases and the handler chain (TS `middleware/types.ts`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};

use crate::error::Error;
use crate::interrupt::Interrupt;

/// The stream a stage's chain returns: events, then exactly one [`MiddlewareItem::Result`].
pub type MiddlewareStream<'a, S> = Pin<
    Box<
        dyn Stream<
                Item = Result<
                    MiddlewareItem<<S as MiddlewareStage>::Event, <S as MiddlewareStage>::Result>,
                    MiddlewareError,
                >,
            > + Send
            + 'a,
    >,
>;

/// A Wrap handler as the registry stores it.
pub(crate) type WrapFn<S> = Arc<
    dyn for<'a> Fn(<S as MiddlewareStage>::Context, MiddlewareNext<'a, S>) -> MiddlewareStream<'a, S>
        + Send
        + Sync,
>;

/// Pins a closure to the higher-ranked Wrap handler signature.
fn wrap_fn<S: MiddlewareStage, F>(handler: F) -> WrapFn<S>
where
    F: for<'a> Fn(S::Context, MiddlewareNext<'a, S>) -> MiddlewareStream<'a, S> + Send + Sync + 'static,
{
    Arc::new(handler)
}

/// The innermost layer of a chain, which does the stage's work.
type Terminal<'a, S> =
    Arc<dyn Fn(<S as MiddlewareStage>::Context) -> MiddlewareStream<'a, S> + Send + Sync + 'a>;

/// An interception point of the agent loop (TS `MiddlewareStage`): the context its handlers
/// receive, the result it produces and the events it streams.
///
/// A stage is a type, so two stages are distinct even when they share a name. The phase
/// constructors build the handlers [`MiddlewareRegistry::add`](super::MiddlewareRegistry::add)
/// and `add_middleware` register: [`input`](Self::input) transforms the context, [`wrap`](Self::wrap)
/// runs around the rest of the chain, [`output`](Self::output) transforms the result. Input
/// handlers run first and Output handlers wrap the Wrap handlers, whatever the registration order;
/// within a phase, handlers run in registration order (TS `PHASE_ORDER`).
///
/// ```
/// use strands::middleware::{MiddlewareStage, MiddlewareRegistry, MiddlewareItem};
/// use futures::StreamExt;
///
/// #[non_exhaustive]
/// struct Greeting;
///
/// impl MiddlewareStage for Greeting {
///     type Context = String;
///     type Result = String;
///     type Event = String;
///     const NAME: &'static str = "greeting";
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let registry = MiddlewareRegistry::new();
/// registry.add(Greeting::input(|name: String| async move { Ok(name.to_uppercase()) }));
/// let mut stream = registry.invoke::<Greeting>("world".to_owned(), |name| {
///     Box::pin(futures::stream::iter([Ok(MiddlewareItem::Result(format!("hello {name}")))]))
/// });
/// let item = stream.next().await.unwrap().unwrap();
/// assert!(matches!(item, MiddlewareItem::Result(text) if text == "hello WORLD"));
/// # });
/// ```
pub trait MiddlewareStage: Sized + Send + Sync + 'static {
    /// What the handlers and the terminal receive.
    type Context: Send + 'static;
    /// What the stage produces.
    type Result: Send + 'static;
    /// What the stage streams while it runs.
    type Event: Send + 'static;
    /// The stage name, for diagnostics.
    const NAME: &'static str;

    /// An Input phase handler: transforms the context before the Wrap handlers run (TS
    /// `stage.Input`).
    fn input<F, Fut>(handler: F) -> Middleware<Self>
    where
        F: Fn(Self::Context) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Self::Context, MiddlewareError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let wrap = wrap_fn::<Self, _>(move |context, next| {
            let handler = Arc::clone(&handler);
            Box::pin(
                futures::stream::once(async move { handler(context).await }).flat_map(move |transformed| {
                    match transformed {
                        Ok(context) => next.run(context),
                        Err(error) => Box::pin(futures::stream::iter([Err(error)])),
                    }
                }),
            )
        });
        Middleware::new(MiddlewarePhaseKind::Input, wrap)
    }

    /// A Wrap phase handler: receives the context and the rest of the chain, and returns the
    /// stage's stream (TS `stage` or `stage.Wrap`). It may forward, drop or add events, skip
    /// `next` to short-circuit, or call it again to retry.
    fn wrap<F>(handler: F) -> Middleware<Self>
    where
        F: for<'a> Fn(Self::Context, MiddlewareNext<'a, Self>) -> MiddlewareStream<'a, Self>
            + Send
            + Sync
            + 'static,
    {
        Middleware::new(MiddlewarePhaseKind::Wrap, wrap_fn(handler))
    }

    /// An Output phase handler: transforms the result after the Wrap handlers finish (TS
    /// `stage.Output`).
    fn output<F, Fut>(handler: F) -> Middleware<Self>
    where
        F: Fn(Self::Result) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Self::Result, MiddlewareError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let wrap = wrap_fn::<Self, _>(move |context, next| {
            let handler = Arc::clone(&handler);
            let mut inner = next.run(context);
            // Applied once the inner layers finish, after any code they run past their result.
            Box::pin(async_stream::stream! {
                let mut result = None;
                while let Some(item) = inner.next().await {
                    match item {
                        Ok(MiddlewareItem::Result(done)) if result.is_none() => result = Some(done),
                        other => yield other,
                    }
                }
                if let Some(result) = result {
                    yield handler(result).await.map(MiddlewareItem::Result);
                }
            })
        });
        Middleware::new(MiddlewarePhaseKind::Output, wrap)
    }
}

/// The phase of a handler (TS `MiddlewarePhaseKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MiddlewarePhaseKind {
    /// Transforms the context (TS `'input'`).
    Input,
    /// Wraps the rest of the chain (TS `'wrap'`).
    Wrap,
    /// Transforms the result (TS `'output'`).
    Output,
}

impl MiddlewarePhaseKind {
    /// Compose order: Input outermost, then Output, then Wrap next to the terminal.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Input => 0,
            Self::Output => 1,
            Self::Wrap => 2,
        }
    }
}

/// A handler for stage `S`, built by [`MiddlewareStage::input`], [`MiddlewareStage::wrap`] or
/// [`MiddlewareStage::output`].
pub struct Middleware<S: MiddlewareStage> {
    phase: MiddlewarePhaseKind,
    handler: WrapFn<S>,
}

impl<S: MiddlewareStage> Middleware<S> {
    fn new(phase: MiddlewarePhaseKind, handler: WrapFn<S>) -> Self {
        Self { phase, handler }
    }

    /// The handler's phase.
    pub fn phase(&self) -> MiddlewarePhaseKind {
        self.phase
    }

    pub(crate) fn into_parts(self) -> (MiddlewarePhaseKind, WrapFn<S>) {
        (self.phase, self.handler)
    }
}

impl<S: MiddlewareStage> Clone for Middleware<S> {
    fn clone(&self) -> Self {
        Self {
            phase: self.phase,
            handler: Arc::clone(&self.handler),
        }
    }
}

impl<S: MiddlewareStage> fmt::Debug for Middleware<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Middleware")
            .field("stage", &S::NAME)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

/// An item of a [`MiddlewareStream`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MiddlewareItem<E, R> {
    /// An event the stage streams.
    Event(E),
    /// The stage's result; the stream's last item.
    Result(R),
}

/// Why a stage's chain stopped without a result: a failure, or an interrupt a handler raised
/// through its context's `interrupt()` (TS `InterruptError`). `?` converts both.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MiddlewareError {
    /// A failure, which ends the invocation (or, under cancellation, its cancel).
    #[error(transparent)]
    Failed(#[from] Error),
    /// An interrupt, which stops the agent with stop reason `interrupt`.
    #[error("interrupt {}", .0.name())]
    Interrupt(Box<Interrupt>),
}

impl From<Interrupt> for MiddlewareError {
    fn from(interrupt: Interrupt) -> Self {
        Self::Interrupt(Box::new(interrupt))
    }
}

/// The rest of a chain, which a Wrap handler calls (TS `MiddlewareNext`).
pub struct MiddlewareNext<'a, S: MiddlewareStage> {
    handlers: Arc<[WrapFn<S>]>,
    index: usize,
    terminal: Terminal<'a, S>,
}

impl<'a, S: MiddlewareStage> MiddlewareNext<'a, S> {
    pub(crate) fn new(
        handlers: Arc<[WrapFn<S>]>,
        terminal: impl Fn(S::Context) -> MiddlewareStream<'a, S> + Send + Sync + 'a,
    ) -> Self {
        Self {
            handlers,
            index: 0,
            terminal: Arc::new(terminal),
        }
    }

    /// Runs the next layer with `context`; each call runs it again.
    pub fn run(&self, context: S::Context) -> MiddlewareStream<'a, S> {
        match self.handlers.get(self.index) {
            Some(handler) => handler(
                context,
                Self {
                    handlers: Arc::clone(&self.handlers),
                    index: self.index.saturating_add(1),
                    terminal: Arc::clone(&self.terminal),
                },
            ),
            None => (self.terminal)(context),
        }
    }
}

impl<S: MiddlewareStage> Clone for MiddlewareNext<'_, S> {
    fn clone(&self) -> Self {
        Self {
            handlers: Arc::clone(&self.handlers),
            index: self.index,
            terminal: Arc::clone(&self.terminal),
        }
    }
}

impl<S: MiddlewareStage> fmt::Debug for MiddlewareNext<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MiddlewareNext")
            .field("stage", &S::NAME)
            .field("remaining", &self.handlers.len().saturating_sub(self.index))
            .finish_non_exhaustive()
    }
}

/// The error of a chain that ended without its result.
pub(crate) fn ended_without_result<S: MiddlewareStage>() -> Error {
    Error::other(format!("the {} middleware chain ended without a result", S::NAME))
}
