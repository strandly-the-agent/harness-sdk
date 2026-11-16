use std::sync::{Arc, Mutex};

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use tracing_subscriber::layer::SubscriberExt;

#[derive(Debug, Clone, Default)]
pub struct Recorder(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Recorder {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.0.lock().unwrap().extend(batch);
        Ok(())
    }
}

impl Recorder {
    pub fn spans(&self) -> Vec<SpanData> {
        self.0.lock().unwrap().clone()
    }
}

pub fn recording() -> (Recorder, tracing::subscriber::DefaultGuard) {
    let recorder = Recorder::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(recorder.clone()).build();
    let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("strands-agents"));
    let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    (recorder, guard)
}

pub fn attrs(span: &SpanData, key: &str) -> Vec<String> {
    span.attributes
        .iter()
        .filter(|a| a.key.as_str() == key)
        .map(|a| a.value.to_string())
        .collect()
}

pub fn dump(spans: &[SpanData]) {
    for s in spans {
        println!("SPAN {} status={:?}", s.name, s.status);
        for a in &s.attributes {
            let k = a.key.as_str();
            if k.starts_with("code.") || k.starts_with("thread.") || matches!(k, "busy_ns" | "idle_ns" | "target" | "level") {
                continue;
            }
            println!("   {k} = {}", a.value);
        }
        for e in s.events.iter() {
            println!("   EVENT {}", e.name);
            for a in &e.attributes {
                println!("      {} = {}", a.key, a.value);
            }
        }
    }
}
