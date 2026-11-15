//! W3C trace context in tool-call arguments (TS `injectTraceContext`).

use crate::types::json::{JsonObject, JsonValue};

/// The `traceparent`/`tracestate` of the current span, or `None` without a valid span.
///
/// With the `otel` feature this reads the OpenTelemetry context of the current `tracing` span
/// and formats it with the W3C trace-context propagator; without it there is no span context.
#[cfg(feature = "otel")]
pub(crate) fn current_trace_context() -> Option<JsonObject> {
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry::trace::TraceContextExt;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let context = tracing::Span::current().context();
    if !context.span().span_context().is_valid() {
        return None;
    }
    let mut carrier = std::collections::HashMap::new();
    TraceContextPropagator::new().inject_context(&context, &mut carrier);
    let mut entries: Vec<(String, String)> = carrier.into_iter().collect();
    entries.sort();
    Some(
        entries
            .into_iter()
            .map(|(key, value)| (key, JsonValue::String(value)))
            .collect(),
    )
}

/// The `traceparent`/`tracestate` of the current span; without the `otel` feature there is none.
#[cfg(not(feature = "otel"))]
pub(crate) fn current_trace_context() -> Option<JsonObject> {
    None
}

/// Merges `carrier` into the arguments' `_meta` object (replacing a non-object `_meta`).
pub(crate) fn inject_trace_context(mut args: JsonObject, carrier: Option<JsonObject>) -> JsonObject {
    let Some(carrier) = carrier else {
        return args;
    };
    let meta = match args.remove("_meta") {
        Some(JsonValue::Object(mut existing)) => {
            existing.extend(carrier);
            existing
        }
        _ => carrier,
    };
    args.insert("_meta".to_owned(), JsonValue::Object(meta));
    args
}
