//! `setup_tracer` takes the process's global `tracing` subscriber slot. An application that installs
//! its own subscriber afterwards (`tracing_subscriber::fmt::init()` is `try_init().expect(..)`) fails.
#![cfg(test)]
use strands::telemetry::{TracerConfig, setup_tracer};

#[test]
fn the_applications_subscriber_after_setup_tracer() {
    setup_tracer(TracerConfig::default()).unwrap();
    let later = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    println!("set_global_default after setup_tracer: {later:?}");
    assert!(later.is_ok());
}
