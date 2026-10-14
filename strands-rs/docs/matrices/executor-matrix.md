# Tool executors + test support — TS → Rust test matrix

Module: `strands-rs/crates/strands-agents/src/tools/executor.rs`, `src/test_support.rs`
Branch: `agent-tasks/strands-rs-executor`. Scope: §6.9–6.11 + §11 of `survey/02-agent-hooks-tools.md`
**minus** interrupts, telemetry, middleware and background tasks.

## TS `tools/executors/__tests__/executor.cancel-signal.test.ts`

| TS test | Rust test | Status |
| --- | --- | --- |
| keeps middleware and tools on the supplied execution signal (not a fresh agent signal) | `tools::executor::tests::keeps_tools_on_the_supplied_execution_signal` | pass (tool half only — middleware is out of scope, so the "middleware sees the signal" assertion is dropped) |
| uses the supplied signal between sequential tool executions | `tools::executor::tests::uses_the_supplied_signal_between_sequential_tool_executions` | pass |
| does not retry after the supplied signal is aborted | `tools::executor::tests::does_not_retry_after_the_supplied_signal_is_aborted` | pass |

That file is the only `executors/__tests__/*` file in the TS tree; the remaining behaviour in
§6.9–6.11 is covered by agent-level tests (`agent/__tests__/agent.{test,concurrent.test}.ts`,
wave 2) and is pinned here with per-behaviour unit tests instead:

| Spec behaviour (§) | Rust test | Status |
| --- | --- | --- |
| one result per tool use, model order (§6.10) | `returns_one_result_per_tool_use_in_model_order` | pass |
| unknown tool → `Tool '<name>' not found in registry`, no `error` on the after-event (§6.9 `_executeToolCore` 2) | `unknown_tool_produces_the_not_found_error_result` | pass |
| before → after → result event order (§6.9) | `emits_before_after_and_result_events_per_tool` | pass |
| hook renames `toolUse.name` → re-resolution, id forced back (§6.9 steps 2–3) | `before_hook_rename_re_resolves_the_tool` | pass |
| `selectedTool` wins over registry lookup (§6.9 step 3) | `selected_tool_overrides_the_registry_lookup` | pass |
| `cancel` short-circuits: tool never runs, message used (§6.9 step 4) | `hook_cancel_short_circuits_the_tool` | pass |
| bare `cancel` → `Tool cancelled by hook` (§6.9 step 4) | `hook_cancel_without_a_message_uses_the_default_text` | pass |
| `AfterToolCallEvent.result` is replaceable (§6.9 `_executeToolAttempt`) | `after_hook_can_replace_the_result` | pass |
| `retry` re-runs the tool (§6.9 step 7) | `after_hook_retry_runs_the_tool_again` | pass |
| `_normalizeToolResultId` (§6.9) | `normalizes_a_result_id_rewritten_by_a_hook` | pass |
| tool progress → `ToolStreamUpdateEvent` (§6.9 `_executeToolCore` 3) | `forwards_tool_progress_as_tool_stream_updates` | pass |
| thrown tool error → error result with the **bare** message (§6.9 `_executeToolCore` 4) | `a_failing_tool_becomes_an_error_result_with_the_bare_message` | pass |
| cancel re-checked before every sequential tool (§6.10) | `uses_the_supplied_signal_between_sequential_tool_executions` | pass |
| concurrent: results in source order even though completion order differs (§6.11 `finally`) | `concurrent_collects_results_in_input_order` | pass |
| concurrent: per-tool event order preserved (§6.11) | `concurrent_preserves_per_tool_event_order` | pass |
| concurrent: partial results survive a cancelled sibling (§6.11) | `concurrent_isolates_a_sibling_failure` | pass |

## Skipped (out of scope)

| TS behaviour | Reason |
| --- | --- |
| interrupt tests (`executeTool` rethrow, `_storePendingToolExecution`, `completedToolResults` resume, "Tool execution interrupted" filler) | interrupts not ported (no `InterruptError`/`_interruptState` in the Rust crate yet) |
| telemetry (`tracer.startToolCallSpan` / `meter.endToolCall`, "emits telemetry even when the tool interrupts") | telemetry not ported |
| middleware (`ExecuteToolStage`, deep-copied `toolUse`, `ctx.tool` swap, `toolGuard`) | middleware not ported |
| `executeBackground` / `backgroundTasks.routeToolCall` | background tasks not ported |
| `Tool '<name>' did not return a result` | unrepresentable: Rust `Tool::invoke` returns `Result<ToolResultBlock>`, never "nothing" |
| `executeTools` §11 steps 1–5 (`BeforeToolsEvent`, empty-tool-use error, batch cancel) | belong to the agent event loop (wave 2), not the executor |

## `src/test_support.rs` (TS `__fixtures__/mock-message-model.ts`, `model-test-helpers.ts`)

The fixtures carry no TS tests of their own; these pin the replay contract.

| Behaviour | Rust test | Status |
| --- | --- | --- |
| message → full event sequence | `test_support::tests::replays_a_text_message_as_a_stream_event_sequence` | pass |
| tool use → `toolUseStart` + input as JSON text delta + inferred `toolUse` stop reason | `infers_the_tool_use_stop_reason_and_streams_input_as_json_text` | pass |
| records `(messages, options)` per call; multi-turn advances and then fails | `records_every_call_and_advances_through_multiple_responses` | pass |
| single queued response is reused indefinitely (TS single-turn behaviour) | `reuses_a_single_response_indefinitely` | pass |
| queued error is streamed as an error | `queued_errors_are_streamed_as_errors` | pass |
| `usage` → trailing metadata event | `emits_trailing_metadata_when_usage_is_supplied` | pass |
| `TestModelProvider` replays an exact event list | `test_model_provider_replays_the_exact_sequence` | pass |
| `empty_model()` streams nothing | `empty_model_streams_nothing` | pass |

Counts: 26 new Rust tests (18 executor, 8 test support), all passing; 3/3 in-scope TS `it(...)`
ported (one partially, middleware half dropped); 6 TS behaviour groups skipped as out of scope.
