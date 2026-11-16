//! TS `telemetry/index.ts:36` exports setupTracer, getTracer, setupMeter, getMeter.
pub fn a() { let _ = strands::telemetry::get_tracer(); }
pub fn b() { let _ = strands::telemetry::get_meter(); }
// The crates whose types are in the signatures of the otel API, as `strands::mcp::{rmcp, reqwest, regex}`
// and `strands::models::bedrock::{aws_config, aws_sdk_bedrockruntime}` are re-exported.
pub fn c() -> strands::telemetry::opentelemetry_sdk::trace::SdkTracerProvider { todo!() }
pub fn d() -> strands::telemetry::opentelemetry::KeyValue { todo!() }
pub fn e(_: strands::telemetry::tracing_opentelemetry::OpenTelemetryLayer<(), ()>) {}
pub fn f<S: strands::telemetry::tracing_subscriber::registry::LookupSpan<'static>>() {}
// control
pub fn ok() -> strands::Result<()> {
    let provider = strands::telemetry::setup_tracer(strands::telemetry::TracerConfig::default())?;
    let _ = provider.shutdown();
    let _ = strands::telemetry::DEFAULT_SERVICE_NAME;
    Ok(())
}
