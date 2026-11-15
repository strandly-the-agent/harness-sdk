//! Middleware (TS `middleware/`): handlers around a stage of the agent loop.
//!
//! A stage is an interception point with a context, a result and the events it streams:
//! [`InvokeModelStage`] around each model call, [`ExecuteToolStage`] around each tool call and
//! [`AgentStreamStage`] around each pass of an invocation. Handlers come in three phases, built
//! by the stage's [`input`](MiddlewareStage::input), [`wrap`](MiddlewareStage::wrap) and
//! [`output`](MiddlewareStage::output) constructors, and are registered with
//! [`Agent::add_middleware`](crate::Agent::add_middleware) or, from a plugin, through
//! [`LocalAgent::add_middleware`](crate::types::agent::LocalAgent::add_middleware).
//!
//! A Wrap handler receives the context and the rest of the chain ([`MiddlewareNext`]) and
//! returns a [`MiddlewareStream`]: the stage's events, ending with its result, the same shape as
//! a tool's stream. Hooks fire on the events a model or tool chain yields, after the chain.
//!
//! ```
//! use strands::middleware::{InvokeModelStage, MiddlewareStage};
//! use strands::testing::MockMessageModel;
//! use strands::types::messages::TextBlock;
//! use strands::Agent;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let model = MockMessageModel::new();
//! model.add_turn(vec![TextBlock::new("Hello!").into()]);
//! let mut agent = Agent::builder().model(model).printer(false).build()?;
//! // Sends only the latest message to the model.
//! agent.add_middleware(InvokeModelStage::input(|mut context| async move {
//!     let last = context.messages.split_off(context.messages.len().saturating_sub(1));
//!     context.messages = last;
//!     Ok(context)
//! }));
//! // Logs around every call.
//! agent.add_middleware(InvokeModelStage::wrap(|context, next| {
//!     tracing::debug!(messages = context.messages.len(), "calling the model");
//!     next.run(context)
//! }));
//! assert_eq!(agent.invoke("Hi").await?.to_string(), "Hello!");
//! # Ok::<(), strands::Error>(())
//! # }).unwrap();
//! ```

mod registry;
mod stages;
mod types;

pub use registry::{MiddlewareCleanup, MiddlewareRegistry};
pub(crate) use stages::MiddlewareInterrupt;
pub use stages::{
    AgentStreamContext, AgentStreamResult, AgentStreamStage, ExecuteToolContext, ExecuteToolResult,
    ExecuteToolStage, InvokeModelContext, InvokeModelResult, InvokeModelStage, MiddlewareInterruptResult,
};
pub use types::{
    Middleware, MiddlewareError, MiddlewareItem, MiddlewareNext, MiddlewarePhaseKind, MiddlewareStage,
    MiddlewareStream,
};

#[cfg(test)]
mod tests;
