# Phase 10 review dispositions, track p10fixb

| Finding | Disposition | Commit |
|---|---|---|
| api-1 | Fixed: `telemetry::get_tracer()` and `telemetry::get_meter()` added under `otel` and re-exported (P-720); tests `get_tracer_returns_a_tracer_of_the_global_provider`, `get_meter_returns_a_meter_of_the_global_provider`. The stale "being built by another track" notes in `PARITY.md:290` and `reviews/live.md:54` are handled under api-9 | this commit |
| parity-10 (the `telemetry/config*` part) | Fixed: `test-title-map.json` gains the `config.test.node.ts` key (gaps for `:37` and `:268`, `:252` declared as `stamps_baggage_selected_by_true_onto_spans`); the checker prints `ok` for both `telemetry/__tests__/config*` rows (`15/1/2/0`, `6/0/0/0`). The `json`, `local-trace`, `meter`, `tracer.node` and `memory-tracing` entries belong to p10fixa's files | this commit |
