//! Feasibility of `get_tracer()` (TS config.ts:101-103): a span from the global OpenTelemetry tracer,
//! started inside a `tracing` span exported by the `setup_tracer` subscriber, parents under it.
#![cfg(test)]
use std::sync::{Arc, Mutex};

use opentelemetry::global;
use opentelemetry::trace::{Span as _, Tracer as _};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use strands::telemetry::{DEFAULT_SERVICE_NAME, TracerConfig, setup_tracer};

#[derive(Debug, Clone, Default)]
struct Recorder(Arc<Mutex<Vec<SpanData>>>);
impl SpanExporter for Recorder {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.0.lock().unwrap().extend(batch);
        Ok(())
    }
}

#[test]
fn a_global_tracer_span_parents_under_the_current_tracing_span() {
    let recorder = Recorder::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(recorder.clone()).build();
    setup_tracer(TracerConfig::builder().provider(provider).build()).unwrap();

    tracing::info_span!("execute_tool").in_scope(|| {
        // what `get_tracer()` would return
        let mut span = global::tracer(DEFAULT_SERVICE_NAME).start("my-custom-operation");
        span.end();
    });

    let spans = recorder.0.lock().unwrap().clone();
    let custom = spans.iter().find(|span| span.name == "my-custom-operation").unwrap();
    let outer = spans.iter().find(|span| span.name == "execute_tool").unwrap();
    assert_eq!(custom.parent_span_id, outer.span_context.span_id());
    assert_eq!(custom.span_context.trace_id(), outer.span_context.trace_id());
}
