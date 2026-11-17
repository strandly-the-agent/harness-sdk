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

/// Held by the tests that use the process's telemetry state.
static PROCESS_STATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The process-wide entry points, in one test because they share the process's state. An
/// unsupported name is warned about once and wires nothing, so no tracer provider is
/// registered in the test process; `shutdown_telemetry` takes the provider setup stored, once.
#[tokio::test]
async fn the_process_wide_setup_warns_once_and_shutdown_takes_the_provider() {
    let _process = PROCESS_STATE.lock().await;
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
    assert!(matches!(*TELEMETRY.state(), State::Unset));

    // Nothing was wired, so there is nothing to shut down and setup may still run.
    shutdown_telemetry().await.unwrap();
    assert!(matches!(*TELEMETRY.state(), State::Unset));

    #[cfg(feature = "otel")]
    {
        *TELEMETRY.state() = State::Configured(Provider::builder().build());
        // Configured: the selector is not read again.
        setup_telemetry(&selector("h12plug-ignored")).unwrap();
        shutdown_telemetry().await.unwrap();
        assert!(matches!(*TELEMETRY.state(), State::ShutDown));
        // Shut down: the selector is not read, and a second call has nothing to do.
        let ((), warnings) = WarnRecorder::capture(|| setup_telemetry(&selector("h12va-unread")).unwrap());
        assert_eq!(warnings, [""; 0]);
        shutdown_telemetry().await.unwrap();
        assert!(matches!(*TELEMETRY.state(), State::ShutDown));
    }
    reset_telemetry();
    assert!(matches!(*TELEMETRY.state(), State::Unset));
}

/// Rust-only (TS has no shutdown): the job loop of create, shutdown, create, shutdown.
#[test]
fn setup_after_a_shutdown_does_not_call_setup_tracer_again() {
    let harness = Harness::default();
    let env = selector("otlp");
    harness.setup(&env);
    assert_eq!(harness.telemetry.take_provider(), Some(()));
    harness.setup(&env);
    assert_eq!(harness.calls().len(), 1);
    assert_eq!(harness.telemetry.take_provider(), None);
}

/// Rust-only. The SDK keeps the provider in a process-wide slot of its own and hands a clone of
/// it to every `setup_tracer` call, as the spy does here; a second shutdown of that provider is
/// an `AlreadyShutdown` error from `opentelemetry_sdk`.
#[cfg(feature = "otel")]
#[tokio::test]
async fn a_shutdown_after_a_harness_created_after_a_shutdown_returns_ok() {
    let telemetry = Telemetry::<Provider>::new();
    let sdk_slot = Provider::builder().build();
    let calls = RefCell::new(0);
    let create_harness = || {
        telemetry
            .setup(
                &selector("console"),
                |_| {},
                |_| {
                    *calls.borrow_mut() += 1;
                    Ok(sdk_slot.clone())
                },
            )
            .unwrap();
    };

    create_harness();
    shutdown(&telemetry).await.unwrap();
    create_harness();
    shutdown(&telemetry).await.unwrap();

    assert_eq!(*calls.borrow(), 1);
    assert!(matches!(*telemetry.state(), State::ShutDown));
}

/// Rust-only: TS flushes on `beforeExit` (`strands-ts/src/telemetry/config.ts`); here the caller
/// does, through `shutdown_telemetry`. The provider batches as the one `setup_tracer` builds for
/// `otlp` does, over an in-memory exporter.
#[cfg(feature = "otel")]
#[tokio::test]
async fn shutdown_telemetry_exports_the_spans_still_in_the_batch() {
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use strands::telemetry::opentelemetry::trace::{Tracer as _, TracerProvider as _};
    use strands::telemetry::opentelemetry_sdk::error::OTelSdkResult;
    use strands::telemetry::opentelemetry_sdk::trace::{
        BatchConfigBuilder, BatchSpanProcessor, SpanData, SpanExporter,
    };

    #[derive(Debug, Clone, Default)]
    struct SpanRecorder(Arc<Mutex<Vec<String>>>);

    impl SpanRecorder {
        fn names(&self) -> Vec<String> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    impl SpanExporter for SpanRecorder {
        async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
            let names = batch.into_iter().map(|span| span.name.into_owned());
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(names);
            Ok(())
        }
    }

    let _process = PROCESS_STATE.lock().await;
    let recorder = SpanRecorder::default();
    // No scheduled export within the test: only a flush empties the batch.
    let batch = BatchConfigBuilder::default()
        .with_scheduled_delay(Duration::from_secs(3600))
        .build();
    let provider = Provider::builder()
        .with_span_processor(
            BatchSpanProcessor::builder(recorder.clone())
                .with_batch_config(batch)
                .build(),
        )
        .build();
    drop(provider.tracer("strands-harness").start("invoke_agent"));
    *TELEMETRY.state() = State::Configured(provider);
    assert_eq!(recorder.names(), Vec::<String>::new());

    shutdown_telemetry().await.unwrap();

    assert_eq!(recorder.names(), ["invoke_agent"]);
    assert!(matches!(*TELEMETRY.state(), State::ShutDown));
    // Nothing is left to shut down.
    shutdown_telemetry().await.unwrap();
    reset_telemetry();
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
