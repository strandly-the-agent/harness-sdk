//! P-490: `setup_tracer`/`setup_meter` with the OTLP exporter (blocking reqwest client), called, used,
//! shut down and dropped inside a tokio runtime. Run with OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:9.
#![cfg(test)]
use std::sync::Arc;

use strands::telemetry::{Exporters, MeterConfig, TracerConfig, setup_meter, setup_tracer};
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otlp_setup_use_shutdown_and_drop_inside_a_runtime() {
    let exporters = Exporters { otlp: true, console: false };
    let tracer = setup_tracer(TracerConfig::builder().exporters(exporters).build()).unwrap();
    let meter = setup_meter(MeterConfig::builder().exporters(exporters).build()).unwrap();
    let again = setup_tracer(TracerConfig::default()).unwrap();
    drop(again);

    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("hello").into()]);
    let mut agent = strands::Agent::builder().model(Arc::new(model)).printer(false).build().unwrap();
    agent.invoke("hi").await.unwrap();

    println!("tracer force_flush: {:?}", tracer.force_flush());
    println!("meter force_flush: {:?}", meter.force_flush());
    println!("tracer shutdown: {:?}", tracer.shutdown());
    println!("meter shutdown: {:?}", meter.shutdown());
    drop(tracer);
    drop(meter);
    println!("done without a panic");
}
