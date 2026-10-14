# background-tasks (strands-rs) — pass A

STATUS: done & committed on `agent-tasks/strands-rs-background-tasks` (2 commits + final).
All three gates clean: fmt, clippy -D warnings, `cargo test --all-features` (567+33+24+3+10 pass).
Delivered: `src/background_tasks/mod.rs` (config, `resolve_policy`, wildcard/never set, task
registry + tokio-spawned tasks, `add_background_selection`, `strands_manage_background_task`,
synthetic-pair delivery, state persistence with force-fail-on-restore), executor routing in
`src/tools/executor.rs`, continuation drain + spec rewrite in `src/agent/event_loop.rs`,
`Agent::{background_tasks,add_continuation_input,take_continuation_input}`,
`AgentBuilder::background_tasks(impl Into<BackgroundTasksConfig>)` (`bool` works),
`tests/background_tasks.rs` (3 loop-level tests). Matrix: reports/background-tasks-matrix.md.

## Pass B — engine remainder (`agent-tasks/strands-rs-bg-engine2`)

Per `survey3/bt2-engine.md` + `reports/review-phase2-loop.md`. All in
`src/background_tasks/mod.rs` unless noted.

| bt2 delta / review item | TS source | Rust | Test |
|---|---|---|---|
| Outcome commit guarded on `Working` (a cancel/timeout wins, late outcome dropped) | `in-process/engine.ts:260` | `TaskRegistry::update_working`/`update_if` | `a_completion_never_overwrites_a_cancelled_task` |
| `max_concurrency` → queue beyond the cap (`Queued` → `Working` when a slot frees) | `engine.ts` `_startQueuedTasks` | `BackgroundTasksConfig::max_concurrency` + `tokio::sync::Semaphore` | `max_concurrency_queues_tasks_beyond_the_cap`, loop: `max_concurrency_one_runs_background_calls_sequentially` |
| Constructor validation, verbatim `maxConcurrency must be a positive finite integer, got {n}` / `assert_timer_delay("timeout", …)` | `engine.ts:30-33` | `BackgroundTasks::new` | `rejects_a_zero_max_concurrency_and_an_out_of_range_timeout` |
| Per-task `timeout` → `Failed` + `{ type: timeout, message: "Timed out after {ms}ms" }`, no result kept | `engine.ts` `_timeoutTask` | `tokio::time::timeout` in the spawned task | `a_task_that_outlives_its_timeout_fails_with_the_timeout_message`, loop: `a_timed_out_background_task_is_delivered_as_failed` |
| Pass-id dedup on `(pass_id, tool_use_id)`; a repeat returns the existing task | `in-process/manager.ts:77-93` | `TaskRegistry::submissions` + `submit_tool_call(.., pass_id)`; `ExecuteToolsInput::pass_id` (new, generated per pass in `tools/executor.rs`) | `the_same_pass_and_tool_use_returns_the_existing_task` |
| `AfterToolCallEvent` for the background run (hooks/telemetry see background results) | `executor.ts` comment; review 🟡 | `agent.hooks().invoke_callbacks` on settlement (only when the outcome applied) | `the_background_run_fires_after_tool_call` |
| Keep the `JoinHandle` so `cancel` aborts the running task | review 🟢 | `TaskRegistry::handles` | `cancel_aborts_the_running_task` |

Deviations: `max_concurrency`/`timeout` are `Option` and default to `None` (unbounded / no
timeout), not TS's manager defaults of 4 / `Infinity`-with-a-gate — `None` keeps the existing
spawn-immediately behaviour, and with no gate a task is `Working` from submit (TS always reports
`queued` first). `BeforeToolCallEvent` is still not re-fired for the background run (it already
fired in the foreground pass, before routing). Still open from bt2: re-queue-on-still-`queued`
(§6, resume race — needs interrupts), `onTaskUpdated` notifications, per-task `wait`,
`remove(task_ids)`, and the whole interrupt round-trip.

NEXT (pass C):
2. Interrupt-dependent TS tests are skipped (no interrupt subsystem in Rust) — revisit if/when
   interrupts land; `BackgroundTaskStatus::InputRequired` is intentionally absent.
3. Tighten continuation semantics toward TS `continuation.ts`: `isCompleteMessageInput`
   validation, `on_appended`/`on_abandoned` callbacks, deferred inputs.
4. Consider wiring `assert_can_load_snapshot` into `SessionManager`'s snapshot load path
   (currently a public method nobody calls).
