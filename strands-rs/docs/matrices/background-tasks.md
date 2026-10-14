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

NEXT (pass B):
1. Port the in-process engine pieces: `max_concurrency`, per-task `timeout`
   (`BackgroundTaskFailureType::Timeout`), pass-id dedup of repeated submissions.
2. Interrupt-dependent TS tests are skipped (no interrupt subsystem in Rust) — revisit if/when
   interrupts land; `BackgroundTaskStatus::InputRequired` is intentionally absent.
3. Tighten continuation semantics toward TS `continuation.ts`: `isCompleteMessageInput`
   validation, `on_appended`/`on_abandoned` callbacks, deferred inputs.
4. Consider wiring `assert_can_load_snapshot` into `SessionManager`'s snapshot load path
   (currently a public method nobody calls).
