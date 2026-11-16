//! TS `setupTracer({ provider })` only registers the provider (config.ts:191-199): "the caller is
//! responsible for their own context manager / propagator setup" (config.ts:170-171).
//! Does Rust `setup_tracer` with a custom provider keep the application's global propagator?
#![cfg(test)]
use opentelemetry::global;
use opentelemetry_sdk::propagation::BaggagePropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use strands::telemetry::{TracerConfig, setup_tracer};

fn fields() -> Vec<String> {
    global::get_text_map_propagator(|propagator| propagator.fields().map(str::to_owned).collect())
}

#[test]
fn a_custom_provider_keeps_the_applications_propagator() {
    // the application's own propagator setup: baggage only
    global::set_text_map_propagator(BaggagePropagator::new());
    assert_eq!(fields(), ["baggage"]);

    let provider = SdkTracerProvider::builder().build();
    setup_tracer(TracerConfig::builder().provider(provider).build()).unwrap();

    assert_eq!(fields(), ["baggage"], "setup_tracer replaced the application's global propagator");
}
