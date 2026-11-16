//! Ports `harness-ts/test/telemetry.test.ts`.
//!
//! TS mocks the SDK's `setupTracer` and mutates `process.env`; here each case has its own
//! `Telemetry` state, an environment lookup over a map, and a recording `setup_tracer`, so the
//! cases pin the harness's gating (which exporters are wired, and how often) without touching
//! the process's tracer provider or environment.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::*;
use crate::test_support::WarnRecorder;

fn env(vars: &[(&'static str, &'static str)]) -> EnvLookup {
    let vars: HashMap<&str, &str> = vars.iter().copied().collect();
    Arc::new(move |name| vars.get(name).map(|value| (*value).to_owned()))
}

fn selector(value: &'static str) -> EnvLookup {
    env(&[(TRACES_EXPORTER_ENV_VAR, value)])
}

/// One process's worth of state with a spy in place of the SDK's `setup_tracer`.
#[derive(Default)]
struct Harness {
    telemetry: Telemetry<()>,
    calls: RefCell<Vec<Exporters>>,
    warnings: RefCell<Vec<String>>,
}

impl Default for Telemetry<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    fn setup(&self, env: &EnvLookup) {
        self.telemetry
            .setup(
                env,
                |warning| self.warnings.borrow_mut().push(warning.to_owned()),
                |exporters| {
                    self.calls.borrow_mut().push(exporters);
                    Ok(())
                },
            )
            .unwrap();
    }

    fn calls(&self) -> Vec<Exporters> {
        self.calls.borrow().clone()
    }
}

fn calls_for(env: &EnvLookup) -> Vec<Exporters> {
    let harness = Harness::default();
    harness.setup(env);
    harness.calls()
}

#[test]
fn configures_nothing_without_a_selector() {
    assert_eq!(calls_for(&env(&[])), []);
}

#[test]
fn does_not_enable_tracing_from_an_endpoint_alone() {
    let env = env(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318")]);
    assert_eq!(calls_for(&env), []);
}

#[test]
fn wires_the_otlp_exporter_from_the_selector() {
    assert_eq!(
        calls_for(&selector("otlp")),
        [Exporters {
            otlp: true,
            console: false
        }]
    );
}

#[test]
fn wires_the_console_exporter_from_the_selector() {
    assert_eq!(
        calls_for(&selector("console")),
        [Exporters {
            otlp: false,
            console: true
        }]
    );
}

#[test]
fn wires_both_when_the_selector_lists_them() {
    assert_eq!(
        calls_for(&selector("otlp, console")),
        [Exporters {
            otlp: true,
            console: true
        }]
    );
}

#[test]
fn disables_tracing_when_the_selector_is_none() {
    assert_eq!(calls_for(&selector("none")), []);
}

#[test]
fn warns_and_skips_an_unsupported_exporter() {
    let harness = Harness::default();
    harness.setup(&selector("zipkin"));
    assert_eq!(harness.calls(), []);
    assert_eq!(
        *harness.warnings.borrow(),
        [
            "OTEL_TRACES_EXPORTER names 'zipkin', which the harness does not support; skipping it. Supported values: otlp, console, none."
        ]
    );
}

#[test]
fn still_wires_the_supported_exporters_alongside_an_unsupported_one() {
    assert_eq!(
        calls_for(&selector("zipkin,otlp")),
        [Exporters {
            otlp: true,
            console: false
        }]
    );
}

#[test]
fn runs_once_per_process() {
    let harness = Harness::default();
    let env = selector("otlp");
    harness.setup(&env);
    harness.setup(&env);
    harness.setup(&env);
    assert_eq!(harness.calls().len(), 1);
}

// Rust-only cases.

#[test]
fn the_selector_is_trimmed_lowercased_and_none_wins() {
    assert_eq!(
        calls_for(&selector(" OTLP ,, Console ")),
        [Exporters {
            otlp: true,
            console: true
        }]
    );
    assert_eq!(calls_for(&selector("otlp,NONE")), []);
    assert_eq!(calls_for(&selector(" , ")), []);
}

#[test]
fn a_run_that_wired_nothing_or_failed_does_not_count_as_configured() {
    let harness = Harness::default();
    harness.setup(&env(&[]));
    harness.setup(&selector("none"));
    let failed = harness.telemetry.setup(
        &selector("otlp"),
        |_| {},
        |_| Err(strands::Error::other("no exporter")),
    );
    assert_eq!(failed.unwrap_err().to_string(), "no exporter");
    harness.setup(&selector("console"));
    assert_eq!(
        harness.calls(),
        [Exporters {
            otlp: false,
            console: true
        }]
    );
}

/// The process-wide entry points, in one test because they share the process's state. An
/// unsupported name is warned about once and wires nothing, so no tracer provider is
/// registered in the test process; `shutdown_telemetry` takes the provider setup stored.
#[tokio::test]
async fn the_process_wide_setup_warns_once_and_shutdown_takes_the_provider() {
    let env = selector("h12plug-unsupported");
    let ((), warnings) = WarnRecorder::capture(|| {
        setup_telemetry(&env).unwrap();
        setup_telemetry(&env).unwrap();
    });
    assert_eq!(
        warnings,
        [
            "OTEL_TRACES_EXPORTER names 'h12plug-unsupported', which the harness does not support; skipping it. Supported values: otlp, console, none."
        ]
    );
    assert!(TELEMETRY.provider().is_none());

    #[cfg(feature = "otel")]
    {
        shutdown_telemetry().await.unwrap();
        *TELEMETRY.provider() = Some(Provider::builder().build());
        // Configured: the selector is not read again.
        setup_telemetry(&selector("h12plug-ignored")).unwrap();
        shutdown_telemetry().await.unwrap();
        assert!(TELEMETRY.provider().is_none());
        *TELEMETRY.provider() = Some(Provider::builder().build());
    }
    reset_telemetry();
    assert!(TELEMETRY.provider().is_none());
}

#[test]
fn the_warning_is_the_one_in_the_ts_source() {
    let Some(source) = crate::test_support::ts_source("telemetry.ts") else {
        return;
    };
    assert!(source.contains("TRACES_EXPORTER_ENV_VAR = 'OTEL_TRACES_EXPORTER'"));
    assert!(source.contains("names '${name}', which the harness does not support; skipping it. ` +"));
    assert!(source.contains("'Supported values: otlp, console, none.'"));
}
