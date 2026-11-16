# p10fixa staged DIVERGENCES.md rows

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| Local trace ids | `AgentTrace.id` is `crypto.randomUUID()` (UUID v4) | UUID v7 | Time-ordered ids from the `uuid` crate, as for message tracking ids; the id is opaque |
| Dropped agent stream, telemetry | A consumer `break` lets the run drain: later cycles still start and close | Dropping the stream stops the run; the open `chat`, `execute_tool`, cycle and agent spans and the meter cycle end at the drop with OK status | A dropped Rust stream can't be driven further; the spans still end once |
| Trace attribute named like a later-set span attribute | Kept on spans that never set the key (for example `gen_ai.usage.total_tokens` on a cycle span) | Dropped on every span: the keys a span sets after its start (start and end time, usage, request duration, tool status, arguments and result, Langfuse type, status) are never taken from `trace_attributes` | An OpenTelemetry attribute can't be replaced once recorded through `tracing`, and exporting a key twice is invalid OTLP |
| Detached memory writes and the caller's span | The detached save runs in the async context of the span that scheduled it | The detached drivers are spawned without the caller's `tracing` span; `memory.extract` and the detached `memory.add` are root spans in both SDKs | A held `tracing` span handle keeps the span and its parents unexported until the save ends |
| `memory.search` content attribute without `end_search` | Under `gen_ai_span_attributes_only`/Langfuse the query is on the span from its start | `content` is written once, when the span ends (the results, or the query on an error) | The second `set_attribute` would export `content` twice |
