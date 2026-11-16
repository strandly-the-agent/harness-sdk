# Observed results (commit d4b516c45, 2026-11-16)

`./run.sh <file> [--features otel]` checks `scratch/`; `./runu.sh <file> [test ...]` checks or tests `usage/`.
Both build into the shared `/tmp/ct-review-r10`. The scratch crates' own build outputs were removed from it
after the runs (the root disk reached 100%); the sources here rebuild them.

- `builders_unnameable.rs` (`--features otel`): E0425 `cannot find type TracerConfigBuilder in module strands::telemetry`, E0425 `... MeterConfigBuilder ...`.
- `missing_exports.rs` (`--features otel`): E0433 for `opentelemetry_sdk`, `opentelemetry`, `tracing_opentelemetry`, `tracing_subscriber` in `telemetry`; E0425 for `get_tracer` and `get_meter`.
- `trace_attrs.rs`: E0433 `cannot find indexmap in strands` and `in telemetry`; E0308 at the two array-of-pairs setter calls; `ok()` compiles.
- `bonfix/`: compiles (`#[builder(default, with = FromIterator::from_iter)]`).
- `layer_output.rs` (`test -- --nocapture`): every agent span lists `["code.file.path", "code.module.name", "code.line.number", "thread.id", "thread.name", "target", "busy_ns", "idle_ns"]`; `SPAN "dependency_internal_span"` is exported with `events: ["dependency log line"]`.
- `layer_output_fixed.rs`: `non-TS attribute keys: []` on every agent span; no `dependency_internal_span`; agent spans and their events unchanged.
- `custom_provider_propagator.rs` (`test`): FAILED, `left: ["traceparent", "tracestate", "baggage"] right: ["baggage"]`.
- `get_tracer_feasible.rs` (`test`): ok.
- `subscriber_order.rs` (`test -- --nocapture`): `set_global_default after setup_tracer: Err(SetGlobalDefaultError("a global default trace dispatcher has already been set"))`.
- `otlp_in_runtime.rs` (`OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:9`, `test -- --nocapture`): ok, "done without a panic"; flush errors are `HTTP export failed: network error`.
- `predicates.rs`: compiles (all four forms).
- `string_trim.rs`: `expected u8, found String`.
- `toe/`: compiles; `cargo tree -e normal` has no `tracing-log`.
