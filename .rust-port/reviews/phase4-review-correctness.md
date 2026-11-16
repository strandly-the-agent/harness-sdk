# Phase 4 correctness review (f5e53f220)

Scope: the brief's phase diff `743a1880d..f5e53f220`, with later-phase paths excluded. Baseline: `strands --lib` unit tests built in the review worktree: 2256 passed, 0 failed (`/tmp/review4_correctness_unit.txt`). Each repro below is a throwaway integration-test crate at `/tmp/review4c-scratch`, which uses only the public API with a path dependency on the worktree. Its output is in `/tmp/review4_correctness_scratch{1..5}.txt`. No repository file was edited.

## Findings

1. **`strands-rs/strands/src/agent/agent_as_tool.rs:225` with `strands-rs/strands/src/cancel.rs:116-130`: a parent's deferred cancel (`after_current_tools`) cuts a running sub-agent short.**
   - **What is wrong:** `as_tool` passes the parent's pass signal to the child as `cancel_signal`, and the child's signal joins it as `parent`. `CancelSignal::is_deferred` and `reason()` walk up to that parent. So when the parent calls `cancel({after_current_tools: true})`, which is meant to let the current tools (the sub-agent call included) finish, the child's checkpoint after its own batch (`run.rs:663-666`) also sees the deferral. The child stops after its first tool batch and ends with the cancel message. Because the child's token never fires, `unfinished` classifies the result as success, so the parent receives the cancel text as the sub-agent's answer.
   - **TS behaviour:** a deferred cancel only sets the agent-private `_deferredCancel` and never aborts (`agent/agent.ts:1100-1101`). The child joins only the AbortSignal (`agent.ts:1273-1274`), and its after-batch check reads its own `_deferredCancel` (`agent.ts:1996`). So in TS the child runs to completion.
   - **Repro** (`tests/deferred_subagent.rs`, output in `scratch1.txt`):
     - Setup: the parent calls the child tool. The child calls a `step` tool, whose body calls `parent.cancel_handle().cancel(after_current_tools: true)`, and would then answer `child final answer`.
     - Actual parent history: `toolResult p1 status=success content "Cancelled by user"`, then `assistant "Cancelled by user"`.
     - TS result: `child final answer`.
   - **Same root cause:** an immediate parent cancel with a message makes the child's final message the parent's text. TS uses `Cancelled by user` there.
   - **Not covered by `DIVERGENCES.md:46`:** that row only covers a user-supplied `InvokeOptions::cancel_signal`.
   - **Fix:** forward an abort-only view to children. For example, add `pub(crate) fn abort_only(&self) -> CancelSignal` that builds a signal on `self.shared.token.child_token()` with `parent: None`, and use it at `agent_as_tool.rs:225` (and in the vended `subagent` tool, if it forwards the same way). Add a test where the parent defers while the child is mid-run.

2. **`strands-rs/strands/src/plugins/registry.rs:33`: a dropped first `invoke`/`initialize` permanently skips every plugin after the one that was awaiting.**
   - **What is wrong:** `initialized = true` is set before the plugin loop. If the caller drops the future while an async `init_agent` is pending (for example a `tokio::time::timeout` around the first `invoke`, or dropping the first `AgentStream`), the remaining plugins never initialize. These include later user plugins, background tasks, delegation and `ModelLifecycle`. No `failure` is recorded, so every later call returns `Ok` from the registry and runs without them. `Agent::initialize` (`agent.rs:226`) then fires `InitializedEvent` as if setup were complete.
   - **TS behaviour:** TS caches the promise (`plugins/registry.ts:30-32`, `this._initializationPromise ??= this._initialize(agent)`), and a consumer break cannot cancel an in-flight await. So in TS all plugins always initialize.
   - **Repro** (`tests/plugin_drop.rs`, output in `scratch2.txt`):
     - Setup: plugin `slow` sleeps 5s in `init_agent`, and plugin `counter` registers a `BeforeModelCallEvent` hook. The first `invoke` runs under `timeout(1s)`, and then a second `invoke` runs.
     - Output: `second invoke ok=true counter_hook_calls=0`.
   - **Fix:** don't mark the registry initialized until the loop completes. Keep a `next: usize` cursor and resume from it, and push the name only after `init_agent` succeeds. Otherwise a resumed init of the interrupted plugin hits `plugin already registered`.

3. **`strands-rs/strands/src/agent/run.rs:167`: the deferred hooks of a dropped run are lost if the next call is also dropped while they run.**
   - **What is wrong:** `finish_abandoned_run` does `self.abandoned.take()` before awaiting the `MessageAddedEvent` and `AfterInvocationEvent` hooks (`:173`, `:176`). If that next `invoke`/`stream`/`shutdown` future is dropped mid-hook, the run is gone. It is never retried, and the `Agent` drop warning (`agent.rs:544`) no longer fires because `abandoned` is `None`. P-91 promises these hooks run "once"; this makes it at most once.
   - **Repro** (`tests/abandoned_twice.rs`, output in `scratch3.txt`):
     - Setup: an async `AfterInvocationEvent` hook sleeps 10s on its first run.
     - Call 1 is dropped mid-model-call, then call 2 is dropped during the deferred hook, then call 3 runs normally.
     - Output: `started=2 finished=1`. The abandoned run's `AfterInvocationEvent` never completes.
   - **Fix:** clear `abandoned` only after both hook invocations return. Alternatively, keep the `AbandonedRun` and set `repair = None` once the `MessageAddedEvent` hooks finish, then clear it after `AfterInvocationEvent`.

4. **`strands-rs/strands/src/agent/run.rs:166-177, 199, 206`: the deferred `AfterInvocationEvent` of a dropped run sees an uncancelled signal, so background tasks block the next call, and the cancel handle cannot stop it.**
   - **What is wrong:**
     - `abandon` cancels the pass signal, then `end_cancel()` (`:199`) installs a fresh `CancelSignal::joining(None)` and detaches the handle.
     - `finish_abandoned_run` runs at `:206`, before `begin_cancel` (`:223`).
     - So the deferred hooks see a signal that is not cancelled and cannot be cancelled.
     - The background-task `AfterInvocationEvent` hook (`background_tasks.rs:517-534`) then waits for every non-terminal task (`wait_for_completion` defaults to true) at the start of the next `invoke`, and `agent.cancel_handle().cancel()` is a no-op during that wait.
   - **Contradiction with P-91:** P-91 treats every drop as a cancel. TS skips this wait when cancelled (`background-tasks.ts:266-272`, `!agent.cancelSignal.aborted`).
   - **Repro** (`tests/abandoned_bg_wait.rs`, output in `scratch4.txt`):
     - Setup: `always: ["slow"]`, where `slow` is a gated tool that is never released. Call 1 is dropped during the second model call. Call 2 runs with `handle.cancel()` after 1s.
     - Output: `second invoke still blocked after 30s (cancel_handle ignored)`.
   - **Fix:** run the deferred hooks under a cancelled signal. In `finish_abandoned_run`: `self.local.begin_cancel(None); self.local.cancel(CancelOptions::default());` before the hooks, and `self.local.end_cancel()` after them (also on the error path). Add a drop-guard test with a pending background task.

## Checked and found consistent with TS or the decisions
- **Cancel checkpoints:** all of these match the TS points (`agent.ts:1735, 1849-1883, 1932, 1996-2013, 2052-2069, 2724, 2743, 2797-2810`):
  - top of cycle, before the limit check and the input append;
  - per non-final stream item, and a stream error while cancelled;
  - top of each model attempt;
  - after a tool-use response, with synthetic `Tool execution cancelled` results;
  - pre-launch in `execute_tools`;
  - a skipped resume with a pending batch continues;
  - after the batch, ahead of the structured-output return;
  - the open-user-turn check;
  - the cancel message is the last non-empty message, else `Cancelled by user`.
- **Retry backoff:** the sleep wakes early on cancel (`model_retry_strategy.rs:222-230`), the loop re-checks before the next attempt, and a cancelled model call is never retried.
- **Middleware:** phase ordering is a stable sort of input, output, wrap (`registry.ts:121-126`). Interrupt ids are `middleware:executeTool:<id>`, `middleware:agentStream` (with a snapshot) and `middleware:<taskId>`, and response precedence matches `middleware/interrupt.ts:22-43`. Model state commits only on chain success.
- **Snapshots:** field resolution (preset, then include, then exclude, with the empty-set error) matches `snapshot.ts:185-215`. Parse-before-apply, a non-object `state` resetting to `{}` (as `state-store.ts:156-162`) and the `toISOString` millisecond format also match.
- **Conversation managers:**
  - Sliding window: the trim and anchor rules, UTF-16 truncation lengths and the overflow-only truncation match `sliding-window-conversation-manager.ts:217-285, 294-302`.
  - Summarizing: the ratio clamp and preserve-recent logic match `summarizing-conversation-manager.ts:91, 146-179`.
  - Proactive compression: `>= threshold` with a 0.7 default.
  - Reactive errors are swallowed per P-96 (documented).
- **Background engine:** cancel, timeout and finish only touch `working` tasks; the timer is cleared; `Timed out after <ms>ms` (`engine.ts:104-120, 248-298`).
- **Lock and panic lints:** `await_holding_lock`, `unwrap_used`, `expect_used` and `panic` are denied workspace-wide. A grep found no slice indexing in the non-test Phase 4 sources.
- **Send:** the `invoke`, `stream`, `initialize` and `shutdown` futures are `Send`, and `CancelHandle`/`CancelSignal` are `Send + Sync` (compile check in `scratch5.txt`).

Verdict: changes requested. Four verified bugs: deferred-cancel leakage into sub-agents, half-initialized plugins after a dropped first call, lost deferred hooks on a second drop, and an uncancellable background-task wait after a drop.
