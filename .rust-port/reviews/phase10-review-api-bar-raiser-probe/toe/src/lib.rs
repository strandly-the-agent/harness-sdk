//! Every `tracing_opentelemetry` item strands uses (`layer`, `OpenTelemetryLayer`, `OpenTelemetrySpanExt`)
//! with the crate's default features (`tracing-log`, `metrics`) off.
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use tracing_opentelemetry::{OpenTelemetryLayer, OpenTelemetrySpanExt as _};
use tracing_subscriber::registry::LookupSpan;

pub fn layer<S>(provider: &SdkTracerProvider) -> OpenTelemetryLayer<S, SdkTracer>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    tracing_opentelemetry::layer().with_tracer(provider.tracer("strands-agents"))
}

pub fn span_ext(span: &tracing::Span) {
    span.set_attribute("key".to_owned(), "value");
    span.add_event("event", vec![KeyValue::new("k", "v")]);
    let _ = span.context();
    span.add_link(opentelemetry::trace::SpanContext::empty_context());
}
