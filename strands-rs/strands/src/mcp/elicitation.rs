//! Elicitation callback types (TS `types/elicitation.ts`).

use std::sync::Arc;

use futures::future::BoxFuture;
use rmcp::model::{ElicitRequestParams, ElicitResult};

use crate::cancel::CancelSignal;
use crate::error::BoxError;

/// Context for an [`ElicitationCallback`] (TS `ElicitationContext`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ElicitationContext {
    cancel_signal: CancelSignal,
}

impl ElicitationContext {
    pub(crate) fn new(cancel_signal: CancelSignal) -> Self {
        Self { cancel_signal }
    }

    /// Cancelled when the server cancels the request (TS `mcpReq.signal`).
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel_signal
    }
}

/// Answers a server's elicitation request (TS `ElicitationCallback`): accept with content,
/// decline or cancel. An error becomes a JSON-RPC error response to the server.
pub type ElicitationCallback = Arc<
    dyn Fn(ElicitationContext, ElicitRequestParams) -> BoxFuture<'static, Result<ElicitResult, BoxError>>
        + Send
        + Sync,
>;
