//! OTEL tracing for a harness agent (`harness-ts/src/telemetry.ts`).
//!
//! The SDK instruments the agent itself; the harness's only job is to turn tracing on when asked.
//! It reads the standard `OTEL_TRACES_EXPORTER` selector (`otlp`/`console`/`none`) and does
//! nothing unless it is set — unset means off, so the harness never silently stands up an
//! exporter pointed at `localhost:4318`.
//!
//! Gap vs TS: the Rust SDK has no `setupTracer` and this crate pulls in no OTLP/console exporter
//! dependency, so a selected exporter cannot be *installed* here. What the harness does wire is
//! [`TelemetryConfig`] (semantic-convention opt-ins and Langfuse detection, read from the
//! environment) onto the agent; the exporter/`TracerProvider` is the embedder's to install on the
//! OpenTelemetry global API.

use std::sync::Mutex;

use strands_agents::telemetry::TelemetryConfig;

use crate::models::warn_once;

/// The standard OTEL exporter selector the harness honors.
pub const TRACES_EXPORTER_ENV_VAR: &str = "OTEL_TRACES_EXPORTER";

/// The exporters the harness recognizes.
pub const SUPPORTED_EXPORTERS: [&str; 2] = ["otlp", "console"];

static CONFIGURED: Mutex<bool> = Mutex::new(false);

/// The exporters named by `OTEL_TRACES_EXPORTER`, in [`SUPPORTED_EXPORTERS`] order.
///
/// An unset selector, or an explicit `none`, means tracing is off. Unrecognized names warn once
/// and are skipped.
pub fn resolve_exporters() -> Vec<&'static str> {
    let selector = std::env::var(TRACES_EXPORTER_ENV_VAR).unwrap_or_default();
    let requested: Vec<String> = selector
        .split(',')
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    if requested.is_empty() || requested.iter().any(|name| name == "none") {
        return Vec::new();
    }
    for name in &requested {
        if !SUPPORTED_EXPORTERS.contains(&name.as_str()) {
            warn_once(format!(
                "{TRACES_EXPORTER_ENV_VAR} names '{name}', which the harness does not support; \
                 skipping it. Supported values: otlp, console, none."
            ));
        }
    }
    SUPPORTED_EXPORTERS
        .into_iter()
        .filter(|name| requested.iter().any(|n| n == name))
        .collect()
}

/// Wire telemetry per the standard OTEL env convention, at most once per process.
///
/// Returns the [`TelemetryConfig`] to put on the agent, or `None` when tracing is off (no
/// exporter selected, or setup already ran).
pub fn setup_telemetry() -> Option<TelemetryConfig> {
    let mut configured = CONFIGURED.lock().expect("telemetry mutex is not poisoned");
    if *configured {
        return None;
    }
    let exporters = resolve_exporters();
    if exporters.is_empty() {
        return None;
    }
    if !exporters.contains(&"otlp") && !exporters.contains(&"console") {
        return None;
    }
    warn_once(format!(
        "{TRACES_EXPORTER_ENV_VAR} selected [{}], but this build ships no span exporter; the \
         agent is configured for tracing and emits spans to whatever TracerProvider is installed \
         on the OpenTelemetry global API.",
        exporters.join(", ")
    ));
    *configured = true;
    Some(TelemetryConfig::from_env())
}

/// Forget that setup ran so it can run again. For tests only.
pub fn reset_telemetry() {
    *CONFIGURED.lock().expect("telemetry mutex is not poisoned") = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OTEL_TRACES_EXPORTER` and the once-flag are process-wide.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with_selector<T>(value: Option<&str>, body: impl FnOnce() -> T) -> T {
        let _guard = env_lock();
        let previous = std::env::var(TRACES_EXPORTER_ENV_VAR).ok();
        match value {
            Some(value) => std::env::set_var(TRACES_EXPORTER_ENV_VAR, value),
            None => std::env::remove_var(TRACES_EXPORTER_ENV_VAR),
        }
        reset_telemetry();
        let out = body();
        match previous {
            Some(previous) => std::env::set_var(TRACES_EXPORTER_ENV_VAR, previous),
            None => std::env::remove_var(TRACES_EXPORTER_ENV_VAR),
        }
        reset_telemetry();
        out
    }

    #[test]
    fn an_unset_selector_means_tracing_is_off() {
        with_selector(None, || {
            assert!(resolve_exporters().is_empty());
            assert!(setup_telemetry().is_none());
        });
    }

    #[test]
    fn none_disables_tracing() {
        with_selector(Some("otlp,none"), || {
            assert!(resolve_exporters().is_empty());
            assert!(setup_telemetry().is_none());
        });
    }

    #[test]
    fn resolves_supported_exporters_case_insensitively() {
        with_selector(Some(" OTLP , console "), || {
            assert_eq!(resolve_exporters(), vec!["otlp", "console"]);
        });
    }

    #[test]
    fn skips_unsupported_exporter_names() {
        with_selector(Some("jaeger,console"), || {
            assert_eq!(resolve_exporters(), vec!["console"]);
        });
    }

    #[test]
    fn returns_a_config_once_per_process() {
        with_selector(Some("otlp"), || {
            assert!(setup_telemetry().is_some());
            assert!(setup_telemetry().is_none());
            reset_telemetry();
            assert!(setup_telemetry().is_some());
        });
    }

    #[test]
    fn reads_semantic_convention_opt_ins_from_the_env() {
        with_selector(Some("console"), || {
            std::env::set_var("OTEL_SEMCONV_STABILITY_OPT_IN", "gen_ai_latest_experimental");
            let config = setup_telemetry().expect("a config");
            std::env::remove_var("OTEL_SEMCONV_STABILITY_OPT_IN");
            assert!(config.use_latest_conventions);
        });
    }
}
