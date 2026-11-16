//! Telemetry (TS `src/telemetry/`): the local metrics and traces every [`AgentResult`] carries,
//! and with the `otel` feature the OpenTelemetry setup functions.
//!
//! With `otel`, `setup_tracer` and `setup_meter` register the providers, and the agents built
//! afterwards export their spans and metrics:
//!
//! ```no_run
//! # #[cfg(feature = "otel")]
//! # async fn run() -> strands::Result<()> {
//! use strands::telemetry::{Exporters, MeterConfig, TracerConfig, setup_meter, setup_tracer};
//! use strands::{Agent, Error};
//!
//! let exporters = Exporters::builder().otlp(true).build();
//! let tracer_provider = setup_tracer(TracerConfig::builder().exporters(exporters).build())?;
//! let meter_provider = setup_meter(MeterConfig::builder().exporters(exporters).build())?;
//!
//! let mut agent = Agent::builder().build()?;
//! agent.invoke("Hello").await?;
//!
//! // Flush what is pending before the process exits. `shutdown()` blocks, so keep it off the
//! // runtime threads.
//! let flushed = tokio::task::spawn_blocking(move || {
//!     tracer_provider.shutdown()?;
//!     meter_provider.shutdown()
//! });
//! flushed.await.map_err(Error::other)?.map_err(Error::other)?;
//! # Ok(())
//! # }
//! ```
//!
//! An application with its own OpenTelemetry provider registers it and adds `tracing_layer` to
//! its `tracing` subscriber; the agent's metrics go to the global meter provider:
//!
//! ```no_run
//! # #[cfg(feature = "otel")]
//! # fn run() {
//! use strands::telemetry::opentelemetry::global;
//! use strands::telemetry::opentelemetry_sdk::trace::SdkTracerProvider;
//! use strands::telemetry::tracing_subscriber::layer::SubscriberExt as _;
//! use strands::telemetry::tracing_subscriber::util::SubscriberInitExt as _;
//! use strands::telemetry::{tracing_layer, tracing_subscriber};
//!
//! let provider = SdkTracerProvider::builder().build();
//! global::set_tracer_provider(provider.clone());
//! tracing_subscriber::registry().with(tracing_layer(&provider)).init();
//! # }
//! ```
//!
//! The crate logs through `tracing` as well; install any subscriber to see its warnings.
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
