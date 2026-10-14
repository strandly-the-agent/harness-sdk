# background-tasks — test matrix (pass A)

Rust tests: `src/background_tasks/mod.rs` (17 unit) + `tests/background_tasks.rs` (3 loop-level) = 20 added.

| TS test | Rust test | Status |
| --- | --- | --- |
| `background-tasks.test.ts` → dispatches selected calls through the normal tool pipeline and delivers their results | `tests/background_tasks.rs::dispatches_a_flagged_tool_call_and_delivers_the_result_on_the_next_invoke` | pass |
| applies always and never policies per tool | `always_routes_without_the_flag_and_never_stays_in_the_foreground` | pass |
| rejects conflicting tool policies | `rejects_a_tool_configured_in_two_modes` | pass |
| delivers work that finishes between invocations | `dispatches_a_flagged_tool_call_and_delivers_the_result_on_the_next_invoke` (second `invoke`) | pass |
| fails interrupted work restored from appState | `force_fails_non_terminal_tasks_on_restore` | pass |
| surfaces and resumes interrupts from detached tools | — | skipped (interrupts / `InterruptError` are not in the Rust SDK) |
| `in-process/manager.test.ts` → executes the selected tool with transient invocation state | `tests/background_tasks.rs::dispatches_…` (tool runs via `ToolContext` with the invocation state) | partial |
| deduplicates repeated submissions within one pass | — | skipped (pass-id dedup not ported in pass A) |
| projects tool errors | `delivers_terminal_tasks_as_a_synthetic_tool_pair` (failed task delivered with `status: success`) | partial |
| cancels tool execution | `manage_tool_lists_gets_and_cancels` (cancel token fired + task marked `cancelled`) | pass |
| rejects an invalid wait timeout | `rejects_out_of_range_timer_delays` | pass |
| throws BackgroundTaskTimeoutError when the wait exceeds the timeout | — | skipped (per-task `timeout`/`maxConcurrency` not ported in pass A) |
| requests input and resumes tool interrupts | — | skipped (no interrupts) |
| fails tasks interrupted outside their tool context | — | skipped (no interrupts) |
| `engine-execution.test.ts` (4 tests: concurrency bound, classified failures, timeout capacity, Infinity timeout) | — | skipped (no concurrency/timeout engine in pass A) |
| `engine-execution.test.ts` → prevents task update callbacks from mutating engine-owned state | n/a — snapshots are owned `BackgroundTask` clones by construction | n/a |
| `engine-transitions.test.ts` (5 tests) | partially covered by `manage_tool_lists_gets_and_cancels` (cancel of a running task) | mostly skipped (no engine) |

Extra Rust tests with no 1:1 TS `it` (TS covers these inside the above cases):
`defaults_every_tool_to_agentic`, `drops_the_agentic_wildcard_when_always_or_never_claims_it`,
`prefers_an_exact_entry_over_the_wildcard`, `injects_the_background_property`,
`refuses_composite_and_conflicting_schemas`,
`fails_an_exact_agentic_tool_whose_schema_cannot_take_the_flag`,
`routes_only_when_the_flag_is_set_for_an_agentic_tool`,
`refuses_to_background_a_foreground_only_tool_listed_explicitly`,
`manage_tool_rejects_a_missing_or_unknown_task_id`,
`persisting_an_empty_registry_clears_the_state_key`,
`rejects_snapshot_loads_while_tasks_are_tracked`,
`offers_the_background_flag_only_for_agentic_tools`,
`waits_for_outstanding_tasks_before_the_invocation_finishes`.

## Deviations from TS
- No in-process **engine**: tasks are plain `tokio::spawn`ed futures. `maxConcurrency` and
  `timeout` are therefore not implemented (config fields omitted rather than ignored);
  `assert_timer_delay` is ported for parity.
- No interrupts (`input_required`, `InterruptError`, `resume`) — the Rust SDK has no interrupt
  subsystem, so `BackgroundTaskStatus` omits `input_required` and `_beforeModelCall`'s
  interrupt routing is absent.
- Continuation delivery is a minimal faithful version of `continuations.addInput`:
  `Agent::add_continuation_input(Vec<Message>)` queues messages; the outer loop prepends them to
  the next invocation's input (`agent/event_loop.rs`). No `isCompleteMessageInput` validation,
  no `onAbandoned`; `onAppended` cleanup (task removal + re-persist) happens at enqueue time.
- Delivery fires from a `BeforeInvocationEvent` hook (TS: `BeforeModelCallEvent`) plus the
  `AfterInvocationEvent` hook, because the Rust loop has no per-cycle input seam.
- `wait_for_completion` is kept on the config; with it on, a pending continuation makes the
  invocation resume once (TS does this through `continuations.prepare`).
- Schema injection runs where the loop builds `tool_specs` (TS: `InvokeModelStage.Input`
  middleware). Behaviour — including the composite-schema refusal and the exact-vs-wildcard
  error — is unchanged.
- Dedup of repeated submissions within one pass (TS `passId`) is not ported; every tool use
  creates a task. `BackgroundTaskManager`'s `wait`/`waitForIdle`/`resume`/`remove` surface is
  reduced to `wait_for_idle`, `cancel`, `get`, `list`.
