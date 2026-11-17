# Rows for `harness-rs/docs/DIVERGENCES.md` (track h12va)

Replaces the row `Flushing spans at exit` (correctness-b-1):

| Flushing spans at exit | The SDK's `setupTracer` flushes the tracer provider on Node's `beforeExit`; the harness exports nothing for it | The caller calls `shutdown_telemetry().await` once before the process exits; without it the spans still in the OTLP batch are lost. After it tracing stays off for the rest of the process: agents that run after it export no spans, a harness created after it is not traced, and a later `shutdown_telemetry` returns `Ok(())` | Rust has no process-exit hook (P-1130, P-1210) |
