//! Middleware (`strands-ts/src/middleware/*`): typed stages with input/wrap/output phases around
//! the model call and each tool call.
//!
//! A stage is a type implementing [`Stage`]; handlers registered for it are keyed by
//! `TypeId::of::<S>()` (the TS registry keys by the frozen stage object's identity, so two stages
//! with the same name stay distinct). A wrap handler owns the call: it may skip [`Next::run`]
//! (short-circuit), call it several times (retry), transform the result, and choose which events
//! inner layers may emit by passing a wrapped [`EventSink`].
//!
//! Note on cancellation: a handler's cleanup after `next` is lost if the invocation future is
//! dropped mid-call. Put must-run cleanup in a `Drop` guard, not in a tail after `.await`.

mod registry;
mod sink;
mod stages;

pub use registry::{
    wrap_fn, Middleware, MiddlewareHandle, MiddlewareRegistry, Next, PhaseKind, Terminal, WrapFn,
};
pub use sink::{EventSink, EventTarget};
pub use stages::{ExecuteToolContext, ExecuteToolStage, InvokeModelContext, InvokeModelStage};

/// A middleware stage: the context it receives, the result it produces, the events it streams.
///
/// Third parties may define their own stages; the SDK keeps no closed set.
pub trait Stage: 'static {
    /// Context handed to the handlers and the terminal operation.
    type Context: Send + 'static;
    /// Value the stage produces.
    type Result: Send + 'static;
    /// Events the stage streams while it runs.
    type Event: Send + 'static;

    /// Stage name, for diagnostics only (identity is the type, not the name).
    fn name() -> &'static str
    where
        Self: Sized;
}
