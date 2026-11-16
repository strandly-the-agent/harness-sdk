//! OpenTelemetry tracing for a harness agent (TS `telemetry.ts`).
//!
//! The SDK already instruments the agent; the harness only wires an exporter, and only when
//! asked: it reads the standard `OTEL_TRACES_EXPORTER` selector (`otlp`, `console`, `none`) and
//! does nothing unless it is set. Unset means off: the harness does not honor OpenTelemetry's
//! default of `otlp`, so it never stands up an exporter nobody asked for. Endpoint, headers and
//! protocol come from the usual `OTEL_EXPORTER_OTLP_*` variables, which the exporter reads.
//!
//! Tracing is process-global, so setup runs at most once per process: building several agents,
//! or a subagent per call, cannot stack exporters onto one trace.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::env::EnvLookup;
use crate::error::Result;
use crate::logging::warn_once;
use crate::text::js_trim;

pub(crate) const TRACES_EXPORTER_ENV_VAR: &str = "OTEL_TRACES_EXPORTER";

/// The exporters the SDK's `setup_tracer` has a flag for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Exporters {
    otlp: bool,
    console: bool,
}

/// The exporters to wire, from the `OTEL_TRACES_EXPORTER` selector. An unset selector, or one
/// that names `none`, means tracing is off.
fn resolve_exporters(env: &EnvLookup, mut warn: impl FnMut(&str)) -> Option<Exporters> {
    let selector = env(TRACES_EXPORTER_ENV_VAR).unwrap_or_default();
    let requested: Vec<String> = selector
        .split(',')
        .map(|name| js_trim(name).to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    if requested.iter().any(|name| name == "none") {
        return None;
    }
    for name in &requested {
        if name != "otlp" && name != "console" {
            warn(&format!(
                "{TRACES_EXPORTER_ENV_VAR} names '{name}', which the harness does not support; skipping it. Supported values: otlp, console, none."
            ));
        }
    }
    let exporters = Exporters {
        otlp: requested.iter().any(|name| name == "otlp"),
        console: requested.iter().any(|name| name == "console"),
    };
    (exporters.otlp || exporters.console).then_some(exporters)
}

/// Whether setup ran, and the provider it registered (TS `configured`).
struct Telemetry<P> {
    provider: Mutex<Option<P>>,
}

impl<P> Telemetry<P> {
    const fn new() -> Self {
        Self {
            provider: Mutex::new(None),
        }
    }

    fn provider(&self) -> MutexGuard<'_, Option<P>> {
        self.provider.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Calls `setup_tracer` with the selected exporters, unless setup already ran or the
    /// selector asks for none. A failed setup leaves it to run again.
    fn setup(
        &self,
        env: &EnvLookup,
        warn: impl FnMut(&str),
        setup_tracer: impl FnOnce(Exporters) -> strands::Result<P>,
    ) -> Result<()> {
        let mut provider = self.provider();
        if provider.is_some() {
            return Ok(());
        }
        if let Some(exporters) = resolve_exporters(env, warn) {
            *provider = Some(setup_tracer(exporters)?);
        }
        Ok(())
    }
}

#[cfg(feature = "otel")]
type Provider = strands::telemetry::opentelemetry_sdk::trace::SdkTracerProvider;
#[cfg(not(feature = "otel"))]
type Provider = ();

static TELEMETRY: Telemetry<Provider> = Telemetry::new();

#[cfg(feature = "otel")]
fn setup_tracer(exporters: Exporters) -> strands::Result<Provider> {
    use strands::telemetry::{Exporters as SdkExporters, TracerConfig};

    strands::telemetry::setup_tracer(TracerConfig {
        exporters: SdkExporters {
            otlp: exporters.otlp,
            console: exporters.console,
        },
        ..TracerConfig::default()
    })
}

#[cfg(not(feature = "otel"))]
fn setup_tracer(_exporters: Exporters) -> strands::Result<Provider> {
    warn_once(&format!(
        "{TRACES_EXPORTER_ENV_VAR} is set, but strands-harness was built without the `otel` feature; tracing stays off."
    ));
    Ok(())
}

/// Wires the SDK's span exporters once per process, from `OTEL_TRACES_EXPORTER` as `env` reads
/// it (TS `setupTelemetry`). Not async, unlike TS: the SDK's `setup_tracer` is not.
pub(crate) fn setup_telemetry(env: &EnvLookup) -> Result<()> {
    TELEMETRY.setup(env, warn_once, setup_tracer)
}

/// Exports the spans still pending and shuts the exporters down. Call it once, before the
/// process exits.
///
/// With `OTEL_TRACES_EXPORTER=otlp`, [`create_harness`](crate::create_harness) wires a batching
/// exporter, and a process that exits right after an invocation loses the spans still in the
/// batch. The TypeScript harness flushes them when Node exits; Rust has no exit hook, so the
/// caller does it:
///
/// ```no_run
/// # async fn run() -> strands_harness::Result<()> {
/// use strands_harness::{HarnessAgentOptions, create_harness, shutdown_telemetry};
///
/// let mut agent = create_harness(HarnessAgentOptions::default()).await?;
/// agent.invoke("Hello").await?;
/// shutdown_telemetry().await?;
/// # Ok(())
/// # }
/// ```
///
/// Does nothing when no exporter was wired (`OTEL_TRACES_EXPORTER` unset or `none`, or a build
/// without the `otel` feature). Agents that run after it export no spans.
///
/// # Errors
///
/// An exporter fails to flush or times out.
pub async fn shutdown_telemetry() -> Result<()> {
    #[cfg(feature = "otel")]
    {
        let Some(provider) = TELEMETRY.provider().take() else {
            return Ok(());
        };
        // `shutdown()` blocks until the exporters flush or time out.
        tokio::task::spawn_blocking(move || provider.shutdown())
            .await
            .map_err(strands::Error::other)?
            .map_err(strands::Error::other)?;
    }
    Ok(())
}

/// Forgets that setup ran, so it can run again. For tests only (TS `resetTelemetry`).
#[cfg(test)]
pub(crate) fn reset_telemetry() {
    *TELEMETRY.provider() = None;
}

#[cfg(test)]
mod tests;
