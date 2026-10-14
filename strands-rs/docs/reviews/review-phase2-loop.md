# Phase-2 Rust correctness review (strands-rs) — incremental findings

Started: Wed Oct 14 16:05:07 UTC 2026
Repo: /mnt/workspace/harness-sdk @ agent-tasks/strands-rs

## Findings (ranked; appended as found)

### 🟡 background_tasks: completion overwrites `Cancelled` (diverges from TS engine guard)
`crates/strands-agents/src/background_tasks/mod.rs:598-621` — the spawned closure does `registry.update(&task_id, |task| ...)` unconditionally after `tool.invoke()` returns. `cancel()` (mod.rs:474-494, and manage-tool `cancel` at :806-825) marks the task `Cancelled` immediately, but cancellation is cooperative; when the tool finishes anyway the status flips `Cancelled → Completed/Failed`, `task.error` is left as "Background task cancelled" next to `status: Completed`, and `deliver_ready` then delivers the result of a task the model was told was cancelled. TS guards this: `in-process/engine.ts:260 if (record.status !== 'working') return false`.
Fix: in the closure, early-return if `task.status != Working` (and clear `error` on success). Reachable: model calls `manage{mode:cancel}` on a running shell/web_fetch task.

### 🟢 background_tasks: dead code
`mod.rs:744-745` `let cancels = Arc::clone(&self.registry); let _ = &cancels;` — remove.
### 🔴 executor: BeforeToolCall `cancel` is bypassed when a call routes to the background
`crates/strands-agents/src/tools/executor.rs:199-215` routes/dispatches to `background.submit_tool_call(...)` and `return`s **before** the hook-cancel check at `:217` (`before.cancel.message_or(HOOK_CANCELLED_MESSAGE)`). TS does the opposite: `strands-ts/src/tools/executors/executor.ts:141-161` returns the cancel result first, and only then (`:163`) consults `routeToolCall`. Consequence: any BeforeToolCall hook that denies a tool (interventions / permission plugins, harness `interventions.rs`) is silently skipped when the model sets `_background_execution: true` (default policy is `agentic: ['*']`) or when the tool is in `always` mode — the tool runs anyway in the background. Reachable from model output alone.
Fix: move the `before.cancel` match above the background routing block (mirror TS ordering), and keep emitting `AfterToolCallEvent` for the cancelled path.

### 🟡 executor/background: no `AfterToolCallEvent` for the background run, `Route::Error` skips AfterToolCall
`executor.rs:201-203` returns the route error directly; TS feeds it through `_executeToolAttempt` so `AfterToolCallEvent` fires (executor.ts:176-182). `background_tasks/mod.rs:597` runs `tool.invoke(context)` bare — no Before/AfterToolCall hooks, no middleware for the background execution, whereas TS comments "the background run later emits the matching AfterToolCallEvent". Hook-based observers (telemetry, memory, interventions post-checks) never see background tool results. Not sanctioned in DESIGN.md/PLAN_PHASE2.md. Fix: run the background invocation through the same executor path (or at least emit AfterToolCallEvent on settlement).

### 🟢 background_tasks: spawned `JoinHandle` dropped (`mod.rs:596`)
Tasks outlive the agent (they hold `agent.clone()`), never joined/aborted on drop. Acceptable per TS fire-and-forget semantics, but there's no `abort_all` on drop — consider storing handles or cancelling tokens in `Drop`.
### 🔴 context_manager: proactive compaction never reaches the current model call
`crates/strands-agents/src/agent/event_loop.rs:750` snapshots `messages: agent.messages()` into `BeforeModelCallEvent` **before** hooks run, and `:798` streams `&before.messages` to the model. The context-manager hook (`context_manager/mod.rs:371-379` → `run_strategies` → `agent.set_messages(messages)` at `:294`) only writes the compacted history back to the agent and never touches `event.messages`. So the compacted context is sent one call late; the call that triggered compaction (utilization ≥ threshold) still goes out uncompacted and relies on the overflow→`AfterModelCallEvent` retry path (which does see fresh `agent.messages()` on the next loop iteration). The sibling hooks do it right (`memory/memory_manager.rs:710`, `plugins/vended/context_injector.rs:178` edit `event.messages`). TS has no such split because `agent.messages` is a live array (`context-manager.ts:208`).
Fix: in the BeforeModelCall callback, operate on `event.messages` (or after `run_strategies` acted, `event.messages = agent.messages()`), keeping `agent.set_messages` for durability. Note ordering: if the injector/memory hooks already appended per-call blocks to `event.messages`, a blind overwrite drops them — so compact `event.messages` in place and mirror to the agent instead.
### 🔴 shell tool: timeout is not enforced and the call hangs while any grandchild holds the pipe (proven)
`crates/strands-agents/src/tools/vended/shell.rs:146-170` — `child.kill()` SIGKILLs only `sh`, not its process group; then `stdout.read().await` / `stderr.read().await` (`:155-156`, `:165-166`) await the collector tasks (`:276-290`), which only finish at pipe EOF. Any forked grandchild that inherited the pipes keeps them open. Scratch test (run, then removed):
- `execute("sleep 3 &", Some(10.0))` → `sh` exits at once, tool returns after **3.00 s**.
- `execute("sleep 4 & sleep 10", Some(0.5))` → tool returns after **10.00 s** with `is_err=true` — a 0.5 s timeout became 10 s; with `sleep 1000` the tool call blocks ~17 min and the child is orphaned (zombie/leak). Reachable from model output (`cmd &`, servers, `tee`, any compound command). TS kills the whole tree / detaches; DESIGN.md does not sanction this.
Fix: `cmd.process_group(0)` (std `CommandExt`) and on timeout `libc::killpg(pid, SIGKILL)`; bound the drain after exit/kill: `tokio::time::timeout(Duration::from_millis(250), stdout.read())` → fall back to the partial buffer (and `abort()` the collector tasks).

### 🟡 shell tool: `ctx.cancel` path leaks the child
`shell.rs:222-226` `select!` returns `Err(Error::Cancelled)` and drops `run`; `kill_on_drop(true)` covers `sh` only, grandchildren survive (same root cause as above) and the collector tasks keep running until EOF.

## Checked, no finding
- `storage/mod.rs:63-79 normalize_key` rejects `\`, `..`, empty; leading `/` stripped → key cannot escape `base_dir` (symlinks inside the root are followed, same as TS).
- `background_tasks::wait_for_idle` (`mod.rs:691-707`): `Notified` created before the re-check → no lost wakeup.
- `context_manager::run_strategies` clones `agent.messages()` and writes back via `set_messages` — no parking_lot guard held across `.await`, no nested `with_messages`.
- Overflow retry counter (`context_manager/mod.rs:384-415`) caps at `MAX_OVERFLOW_RETRIES` and resets on success / give-up; tests cover it.
- Continuation drain order (`event_loop.rs:454-459`) matches TS `continuation.ts combine` (`[...continuation, ...publicMessages]`).

## Not reached in the 15-min budget
`memory/**` (flush termination, FileMemoryStore atomic writes), `plugins/vended/{skills,context_injector}.rs` idempotence, `strands-harness/src/agent.rs` create_harness algorithm, `strands-harness/src/tools/{file_tools,web_fetch,web_search,subagent}.rs`.

## Repo state
Scratch test `tests/scratch_review.rs` created, run, removed. `git status` shows `strands-rs/AGENTS.md` modified — not by me (clean at start; `strands-harness/src/plugins/environment.rs` references AGENTS.md, so possibly a test side-effect or a concurrent writer). Left untouched rather than reverting someone else's change.

## Fixes applied (branch `agent-tasks/strands-rs-fix-sdk2`)
- 🔴 `tools/executor.rs`: `before.cancel` is now evaluated **before** background routing (TS
  `executor.ts:141-163` order); a denying hook short-circuits `_background_execution: true` and
  `always`-policy calls, and still emits `AfterToolCallEvent`.
  Test `tests/background_tasks.rs::a_denying_before_tool_call_hook_beats_background_routing`.
- 🔴 `context_manager/mod.rs`: new `run_strategies_on(agent, &mut messages, ..)` compacts a message
  list in place; the `BeforeModelCall` hook runs it on `event.messages` (mirroring to the agent only
  while the event still matches the history, so per-call injected blocks never leak in), so
  compaction reaches the call that triggered it. `run_strategies` delegates and keeps writing back.
  Test `tests/context_manager_loop.rs::truncates_an_oversized_tool_result_in_the_call_that_triggers_it`.
- 🔴 `tools/vended/shell.rs`: the command runs in its own process group (`Command::process_group(0)`)
  and the group is SIGKILLed on every exit path — normal exit, timeout, and a dropped (cancelled)
  call via a `GroupKill` drop guard. Pipe drains are bounded by `DRAIN_GRACE` (250 ms) and the
  collector tasks are aborted (`Collected::read(grace)` + `Drop`), so a grandchild holding the
  inherited pipes can no longer stretch a 0.5 s timeout into 10 s. Uses `nix::sys::signal::killpg`
  rather than `libc` because the workspace lints `forbid` unsafe code.
  Tests `shell::tests::{a_timeout_kills_the_whole_process_group, a_cancelled_call_kills_the_whole_process_group}`.
