//! Telemetry (TS `src/telemetry/`): the local metrics and traces every [`AgentResult`] carries.
//!
//! [`AgentResult`]: crate::AgentResult

mod config;
mod json;
pub mod meter;
pub mod tracer;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub use config::DEFAULT_SERVICE_NAME;
pub use meter::{AgentLoopMetricsData, AgentMetrics, InvocationMetricsData, ToolMetricsData};
pub use tracer::{AgentTrace, AttributeValue};

/// The agent's meter and local tracer, shared with the tool tasks of a batch.
#[derive(Debug, Clone, Default)]
pub(crate) struct LocalTelemetry {
    inner: Arc<Mutex<Recorders>>,
}

#[derive(Debug, Default)]
pub(crate) struct Recorders {
    pub(crate) meter: meter::Meter,
    pub(crate) tracer: tracer::LocalTracer,
}

impl LocalTelemetry {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Recorders> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(all(test, feature = "otel"))]
mod tests;
