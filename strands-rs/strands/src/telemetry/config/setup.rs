//! OpenTelemetry setup (TS `setupTracer`, `setupMeter` and the baggage span processor of
//! `telemetry/config.ts`).

use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opentelemetry::baggage::{Baggage, BaggageExt as _};
use opentelemetry::propagation::TextMapCompositePropagator;
use opentelemetry::trace::{FutureExt as _, Span as _, TracerProvider as _};
use opentelemetry::{Context, InstrumentationScope, KeyValue, global};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use opentelemetry_sdk::trace::{
    BatchSpanProcessor, SdkTracer, SdkTracerProvider, SimpleSpanProcessor, Span, SpanData, SpanProcessor,
};
use tracing::{Level, Subscriber};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry::LookupSpan;

use super::{EnvLookup, process_env, resource_attributes, service_name};
use crate::error::{Error, Result};

/// Which exporters [`setup_tracer`] and [`setup_meter`] add (TS `exporters`).
///
/// The OTLP exporters send protobuf over HTTP and read the standard `OTEL_EXPORTER_OTLP_*`
/// variables themselves.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct Exporters {
    /// Export over OTLP HTTP.
    #[builder(default)]
    pub otlp: bool,
    /// Print to standard output.
    #[builder(default)]
    pub console: bool,
}

/// Decides whether a baggage key is copied onto spans (TS `BaggageKeyPredicate`).
pub type BaggageKeyPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Which baggage entries the [`BaggageSpanProcessor`] copies onto spans (TS `baggageProcessor`:
/// `true`, one predicate or a list of predicates).
#[derive(Clone)]
#[non_exhaustive]
pub enum BaggageKeys {
    /// Every entry.
    All,
    /// Entries whose key satisfies at least one predicate.
    Matching(Vec<BaggageKeyPredicate>),
}

impl BaggageKeys {
    /// Entries whose key satisfies `predicate`.
    pub fn matching(predicate: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        Self::Matching(vec![Arc::new(predicate)])
    }

    fn allows(&self, key: &str) -> bool {
        match self {
            Self::All => true,
            Self::Matching(predicates) => predicates.iter().any(|predicate| predicate(key)),
        }
    }
}

impl fmt::Debug for BaggageKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("All"),
            Self::Matching(predicates) => write!(f, "Matching({} predicates)", predicates.len()),
        }
    }
}

/// Copies the baggage entries of a span's parent context onto the span as attributes.
#[derive(Debug, Clone)]
pub struct BaggageSpanProcessor {
    keys: BaggageKeys,
}

impl BaggageSpanProcessor {
    /// A processor stamping the entries `keys` selects.
    pub fn new(keys: BaggageKeys) -> Self {
        Self { keys }
    }
}

impl SpanProcessor for BaggageSpanProcessor {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        for (key, (value, _)) in cx.baggage() {
            if self.keys.allows(key.as_str()) {
                span.set_attribute(KeyValue::new(key.clone(), value.clone()));
            }
        }
    }

    fn on_end(&self, _span: SpanData) {}

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
}

/// Options of [`setup_tracer`] (TS `TracerConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct TracerConfig {
    /// A provider the caller built; the exporters and the baggage processor are then ignored.
    pub provider: Option<SdkTracerProvider>,
    /// The exporters to add; none by default.
    #[builder(default)]
    pub exporters: Exporters,
    /// Copy baggage entries onto every span.
    pub baggage_processor: Option<BaggageKeys>,
}

/// Options of [`setup_meter`] (TS `MeterConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct MeterConfig {
    /// A provider the caller built; the exporters are then ignored.
    pub provider: Option<SdkMeterProvider>,
    /// The exporters to add; none by default.
    #[builder(default)]
    pub exporters: Exporters,
}

/// The provider a setup function registered, kept for the process (TS `_provider`).
pub(super) type Slot<P> = Mutex<Option<P>>;

static TRACER_PROVIDER: Slot<SdkTracerProvider> = Mutex::new(None);
static METER_PROVIDER: Slot<SdkMeterProvider> = Mutex::new(None);

/// Registers the process's tracer provider and starts exporting the agent's spans (TS
/// `setupTracer`). Later calls return the first provider.
///
/// The provider carries the resource attributes of the environment (`OTEL_SERVICE_NAME`,
/// `OTEL_SERVICE_NAMESPACE`, `OTEL_DEPLOYMENT_ENVIRONMENT`, `OTEL_RESOURCE_ATTRIBUTES`). It
/// becomes the global OpenTelemetry tracer provider, the W3C trace-context and baggage
/// propagators become the global propagator (not with a custom [`TracerConfig::provider`]: its
/// owner sets up propagation, as in TS), and a `tracing` subscriber with
/// [`tracing_layer`] becomes the global default; it exports spans at `INFO` and above (the
/// agent's and the application's own) and no log lines. Where the application already has a
/// global subscriber, add [`tracing_layer`] to it instead. Call `shutdown()` on the returned
/// provider before the process exits to flush pending spans.
///
/// # Errors
///
/// The OTLP exporter can't be built.
pub fn setup_tracer(config: TracerConfig) -> Result<SdkTracerProvider> {
    setup_tracer_in(&TRACER_PROVIDER, config, &process_env, |provider, built| {
        global::set_tracer_provider(provider.clone());
        if built {
            global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
                Box::new(TraceContextPropagator::new()),
                Box::new(BaggagePropagator::new()),
            ]));
        }
        if tracing::subscriber::set_global_default(subscriber(provider)).is_err() {
            tracing::warn!(
                "a global tracing subscriber is already set | add telemetry::tracing_layer to it to export agent spans"
            );
        }
    })
}

/// The `tracing` layer that exports the agent's spans through `provider`, for applications that
/// build their own subscriber.
///
/// The layer adds none of its own attributes (source location, thread, target, busy and idle
/// time). It exports every span and event the subscriber lets through; filter it
/// (`tracing_subscriber::Layer::with_filter`) to export less.
pub fn tracing_layer<S>(provider: &SdkTracerProvider) -> OpenTelemetryLayer<S, SdkTracer>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    tracing_opentelemetry::layer()
        .with_tracer(provider.tracer(service_name(&process_env)))
        .with_location(false)
        .with_threads(false)
        .with_tracked_inactivity(false)
        .with_target(false)
}

/// The subscriber [`setup_tracer`] installs: [`tracing_layer`] for spans at `INFO` and above (the
/// agent's are), so the debug spans of dependencies aren't exported and log lines don't become
/// span events, as in TS.
pub(super) fn subscriber(provider: &SdkTracerProvider) -> impl Subscriber + Send + Sync + 'static {
    let spans = filter_fn(|metadata| metadata.is_span() && *metadata.level() <= Level::INFO);
    tracing_subscriber::registry().with(tracing_layer(provider).with_filter(spans))
}

/// A tracer of the global tracer provider, named after the service (TS `getTracer`), for custom
/// spans. Its spans are recorded once a provider is registered ([`setup_tracer`] or the
/// application's own).
pub fn get_tracer() -> global::BoxedTracer {
    global::tracer(service_name(&process_env))
}

/// A meter of the global meter provider, named after the service (TS `getMeter`), for custom
/// instruments. They record once a provider is registered ([`setup_meter`] or the application's
/// own).
pub fn get_meter() -> opentelemetry::metrics::Meter {
    global::meter_with_scope(InstrumentationScope::builder(service_name(&process_env)).build())
}

pub(super) fn setup_tracer_in(
    slot: &Slot<SdkTracerProvider>,
    config: TracerConfig,
    env: EnvLookup<'_>,
    register: impl FnOnce(&SdkTracerProvider, bool),
) -> Result<SdkTracerProvider> {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(provider) = slot.as_ref() {
        tracing::warn!("tracer provider already initialized, returning existing provider");
        return Ok(provider.clone());
    }
    let built = config.provider.is_none();
    let provider = match config.provider {
        Some(provider) => {
            if config.baggage_processor.is_some() {
                tracing::warn!(
                    "baggage_processor=<ignored> | custom provider is owned by the caller, add BaggageSpanProcessor to it"
                );
            }
            provider
        }
        None => {
            let mut builder = SdkTracerProvider::builder().with_resource(resource(env));
            if let Some(keys) = config.baggage_processor {
                tracing::info!("enabling baggage span processor");
                builder = builder.with_span_processor(BaggageSpanProcessor::new(keys));
            }
            if config.exporters.otlp {
                let mut exporter = opentelemetry_otlp::SpanExporter::builder().with_http();
                if let Some(endpoint) = otlp_endpoint(env, "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "v1/traces")
                {
                    exporter = exporter.with_endpoint(endpoint);
                }
                let exporter = exporter.build().map_err(Error::other)?;
                builder = builder.with_span_processor(BatchSpanProcessor::builder(exporter).build());
            }
            if config.exporters.console {
                let exporter = opentelemetry_stdout::SpanExporter::default();
                builder = builder.with_span_processor(SimpleSpanProcessor::new(exporter));
            }
            builder.build()
        }
    };
    register(&provider, built);
    *slot = Some(provider.clone());
    Ok(provider)
}

/// Registers the process's meter provider, which the agents built afterwards record their
/// metrics to (TS `setupMeter`). Later calls return the first provider.
///
/// Call `shutdown()` on the returned provider before the process exits to flush pending metrics.
///
/// # Errors
///
/// The OTLP exporter can't be built.
pub fn setup_meter(config: MeterConfig) -> Result<SdkMeterProvider> {
    setup_meter_in(&METER_PROVIDER, config, &process_env, |provider| {
        global::set_meter_provider(provider.clone());
    })
}

pub(super) fn setup_meter_in(
    slot: &Slot<SdkMeterProvider>,
    config: MeterConfig,
    env: EnvLookup<'_>,
    register: impl FnOnce(&SdkMeterProvider),
) -> Result<SdkMeterProvider> {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(provider) = slot.as_ref() {
        tracing::warn!("meter provider already initialized, returning existing provider");
        return Ok(provider.clone());
    }
    let provider = match config.provider {
        Some(provider) => provider,
        None => {
            let mut builder = SdkMeterProvider::builder().with_resource(resource(env));
            if config.exporters.otlp {
                let mut exporter = opentelemetry_otlp::MetricExporter::builder().with_http();
                if let Some(endpoint) =
                    otlp_endpoint(env, "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "v1/metrics")
                {
                    exporter = exporter.with_endpoint(endpoint);
                }
                let exporter = exporter.build().map_err(Error::other)?;
                builder = builder.with_periodic_exporter(exporter);
            }
            if config.exporters.console {
                builder = builder.with_periodic_exporter(opentelemetry_stdout::MetricExporter::default());
            }
            builder.build()
        }
    };
    register(&provider);
    *slot = Some(provider.clone());
    Ok(provider)
}

/// The signal's OTLP HTTP endpoint from the environment: the signal's own variable as it is, or
/// `OTEL_EXPORTER_OTLP_ENDPOINT` with the signal path appended. `None` leaves the exporter default.
fn otlp_endpoint(env: EnvLookup<'_>, signal_variable: &str, signal_path: &str) -> Option<String> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    set(signal_variable).or_else(|| {
        set("OTEL_EXPORTER_OTLP_ENDPOINT").map(|base| format!("{}/{signal_path}", base.trim_end_matches('/')))
    })
}

/// TS `getOtelResource`.
pub(super) fn resource(env: EnvLookup<'_>) -> Resource {
    Resource::builder_empty()
        .with_attributes(
            resource_attributes(env)
                .into_iter()
                .map(|(key, value)| KeyValue::new(key, value)),
        )
        .build()
}

/// Runs `future` with `entries` merged into the active baggage (TS `Agent._withBaggage`).
pub(crate) async fn with_baggage<F: Future>(entries: &[(String, String)], future: F) -> F::Output {
    if entries.is_empty() {
        return future.await;
    }
    let active = Context::current();
    let mut baggage = Baggage::new();
    for (key, (value, metadata)) in active.baggage() {
        baggage.insert_with_metadata(key.clone(), value.clone(), metadata.clone());
    }
    for (key, value) in entries {
        baggage.insert(key.clone(), value.clone());
    }
    future.with_context(active.with_baggage(baggage)).await
}

#[cfg(test)]
mod tests;
