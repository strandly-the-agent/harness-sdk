//! Telemetry (TS `src/telemetry/`): the local metrics and traces every [`AgentResult`] carries,
//! and with the `otel` feature the OpenTelemetry setup functions.
//!
//! [`AgentResult`]: crate::AgentResult

mod config;
mod json;
pub mod meter;
pub mod tracer;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[cfg(feature = "otel")]
pub(crate) use config::setup::with_baggage;
#[cfg(feature = "otel")]
pub use config::setup::{
    BaggageKeyPredicate, BaggageKeys, BaggageSpanProcessor, Exporters, ExportersBuilder, MeterConfig,
    MeterConfigBuilder, TracerConfig, TracerConfigBuilder, exporters_builder, get_meter, get_tracer,
    meter_config_builder, setup_meter, setup_tracer, tracer_config_builder, tracing_layer,
};
#[cfg(feature = "otel")]
pub use opentelemetry;
#[cfg(feature = "otel")]
pub use opentelemetry_sdk;
#[cfg(feature = "otel")]
pub use tracing_opentelemetry;
#[cfg(feature = "otel")]
pub use tracing_subscriber;

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

#[cfg(test)]
pub(crate) mod tests;
