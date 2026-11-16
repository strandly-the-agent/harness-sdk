//! What does the subscriber `setup_tracer` installs (setup.rs:172: `registry().with(tracing_layer(provider))`)
//! export for a mock agent run? Prints every exported span: name, attribute keys the SDK did not set, events.
#![cfg(test)]
use std::sync::{Arc, Mutex};

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer as _;

#[derive(Debug, Clone, Default)]
struct Recorder(Arc<Mutex<Vec<SpanData>>>);
impl SpanExporter for Recorder {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.0.lock().unwrap().extend(batch);
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn what_the_setup_tracer_subscriber_exports() {
    let recorder = Recorder::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(recorder.clone()).build();
    let subscriber = tracing_subscriber::registry().with(strands::telemetry::tracing_layer(&provider).with_location(false).with_threads(false).with_tracked_inactivity(false).with_target(false).with_filter(tracing_subscriber::filter::LevelFilter::INFO));
    let _guard = tracing::subscriber::set_default(subscriber);

    let model = MockMessageModel::new();
    // the model asks for a tool the agent does not have, so the SDK logs inside its own spans
    model.add_turn(vec![ToolUseBlock::new("missing_tool", "tooluse_1", serde_json::json!({})).into()]);
    model.add_turn(vec![TextBlock::new("hello").into()]);
    let mut agent = strands::Agent::builder().model(Arc::new(model)).printer(false).build().unwrap();
    agent.invoke("hi").await.unwrap();
    // a dependency's trace-level span and log line, as hyper/h2/aws-smithy emit them
    tracing::trace_span!("dependency_internal_span").in_scope(|| tracing::trace!("dependency log line"));

    for span in recorder.0.lock().unwrap().iter() {
        let extra: Vec<_> = span
            .attributes
            .iter()
            .map(|kv| kv.key.as_str().to_owned())
            .filter(|key| !key.starts_with("gen_ai.") && key != "system_prompt" && key != "agent_loop.cycle_id" && key != "name")
            .collect();
        let events: Vec<_> = span.events.iter().map(|event| event.name.to_string()).collect();
        println!("SPAN {:?}\n  non-TS attribute keys: {extra:?}\n  events: {events:?}", span.name);
    }
}
