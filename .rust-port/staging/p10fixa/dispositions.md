# p10fixa dispositions

| Finding | Disposition | Commit |
| --- | --- | --- |
| correctness-1 | Fixed: `Agent::abandon` ends the open model span, the meter cycle (`Meter::end_open_cycle`), the cycle span and the agent span (OK status, end time, accumulated usage), and `ToolTerminal::stream` holds an `OpenToolSpan` guard that ends the `execute_tool` span and the tool metric when the stream or its task is dropped. Tests `telemetry::tests::agent_tracer::{ends_the_loop_span_once_when_the_public_stream_iterator_is_closed_mid_cycle, ends_the_loop_span_once_when_the_consumer_cancels_and_breaks_during_model_streaming, ends_the_model_invoke_span_once_when_the_consumer_breaks_mid_model_response, a_dropped_stream_ends_the_span_of_the_running_tool}`. No drop guard was added to `ModelTerminal::stream`: the model span lives in `LocalTracer` and `abandon` ends it. | this commit |
| parity-2 | Fixed: same change as correctness-1 (three of the four TS cases ported; `closes cycle telemetry for every started cycle when the consumer breaks during tool execution` not ported: a dropped Rust stream stops the run, it doesn't drain it). | this commit |
