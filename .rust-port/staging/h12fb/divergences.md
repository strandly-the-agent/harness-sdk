# Divergence rows staged by track h12fb

Row for `harness-rs/docs/DIVERGENCES.md`:

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| Flushing spans at exit | The SDK's `setupTracer` flushes the tracer provider on Node's `beforeExit`; the harness exports nothing for it | The caller calls `shutdown_telemetry().await` once before the process exits; without it the spans still in the OTLP batch are lost. After it, agents export no spans | Rust has no process-exit hook (P-1130) |
