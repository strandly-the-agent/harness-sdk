# Review: strands-agents (Rust crate) — agent loop correctness
Date: 2026-10-14 · Repo: harness-sdk @ agent-tasks/strands-rs · Crate: strands-rs/crates/strands-agents
Scope: event_loop.rs, builder.rs, agent/mod.rs, tools/executor.rs, models/model.rs (stream_aggregated), hooks/registry.rs

## Findings (appended incrementally)

### 🔴 F1 — `invoking` flag never reset if the spawned invocation task panics
`src/agent/event_loop.rs:276-282`: `tokio::spawn(async move { let outcome = run_invocation(..).await; ...; agent.inner.invoking.store(false) })`.
A panic anywhere in the task (hook callback, tool body, model impl, `expect` in `estimate_input_tokens`) aborts the task before the
`store(false)`; the agent is permanently stuck — every later `invoke`/`stream` returns `ConcurrentInvocation`, and the pending
`invoke_with` gets "agent stream ended without a result" instead of the panic. Fix: reset in a drop guard
(`struct InvokingGuard(Arc<AgentInner>); impl Drop { store(false) }`) created right after `swap(true)`; also consider forwarding the
`JoinError` into `tx` so the caller sees the panic.

### 🟡 F2 — dropping an `AgentStream` does not stop the invocation (TS generator stops at the yield)
`event_loop.rs:276-283` + `emit` (`:190`): `let _ = tx.send(..)` ignores closed-channel errors, so after the consumer drops the stream
the spawned task keeps calling the model, running tools and appending to `agent.messages`, with `is_invoking() == true` until it finishes
(forever, if the provider stream hangs). TS `for await` abandonment stops at the current `yield`. Fix: in `emit`, treat `tx.send` Err as
`Error::Cancelled` (or `cancel.cancel()` on `tx.closed()`), and/or hold a `CancellationToken` in `AgentStream` and cancel in `Drop`.
Needs DESIGN.md sanction either way; today it is undocumented.
  **F1 proven** (scratch `#[tokio::test]`, removed afterwards): hook `panic!("boom")` on `BeforeInvocationEvent` →
  `invoke` returns "agent stream ended without a result", `is_invoking() == true` 50ms later, second `invoke` →
  `Error::ConcurrentInvocation`. Test asserting `!is_invoking()` FAILED.

### 🟡 F3 — `Agent::cancel()` can cancel the *previous* invocation's token
`src/agent/mod.rs:228-232` (`if is_invoking() { inner.cancel.read().cancel() }`) vs `event_loop.rs:266,287-298`: `invoking` is set in
`stream()` but the fresh token is only installed after `agent.initialize().await` (plugins + `InitializedEvent` hooks) inside the
spawned task. A `cancel()` in that window cancels the stale token; the new invocation runs to completion. Fix: create and install
the token in `stream()` (before `swap(true)` returns) and pass it into `run_invocation`; the `resume` loop can re-install per iteration.

### 🟡 F4 — Cancel mid-tool-batch: Concurrent vs Sequential executors produce different histories, and Concurrent diverges from TS
`tools/executor.rs:73-84` (Sequential: skipped tools get `Tool execution cancelled` error results, returns `Ok`) vs `:111-132`
(Concurrent: a tool returning `Err(Error::Cancelled)` → whole batch `Err(Cancelled)`, finished siblings' results and already-emitted
`ToolResult` events are discarded). In `event_loop.rs:497-500` the `Err(Cancelled)` path skips appending the assistant toolUse message
and the toolResult message, so Concurrent leaves `[user, assistant("Cancelled by user")]` while Sequential/TS (`agent.ts:1724-1747`,
`sequential.ts:51`) leave `[user, assistant(toolUse), user(toolResults…cancelled), assistant("Cancelled by user")]`. No orphan toolUse
either way, but the model loses the tool calls it made. Fix: Concurrent should check `input.cancel.is_cancelled()` and convert
`Err(Cancelled)` from a tool into an `EXECUTION_CANCELLED_MESSAGE` error result (return `Ok`), matching Sequential; reserve the
`Err(Cancelled)` batch abort for nothing (or document it in DESIGN.md).

### 🟡 F5 — provider `Err(Error::Cancelled)` is re-wrapped as `Error::Model` and becomes retryable
`models/model.rs:328-329` (`stream_aggregated`): any non-`is_model_error()` error, including `Error::Cancelled`, is wrapped in
`Error::Model{source}`. `event_loop.rs:838-842` only recovers it when the loop's own token is cancelled; a provider that aborts on
its own (HTTP client abort, parent token) surfaces as a model error → `AfterModelCallEvent.error` is `Model`, so
`DefaultModelRetryStrategy`/hooks may retry a cancellation. Fix: pass `Error::Cancelled` through unchanged (`Err(Error::Cancelled) => { yield Err(Error::Cancelled); return; }`).

### 🟢 F6 — `Agent::stream()` panics outside a tokio runtime
`event_loop.rs:276` `tokio::spawn` with no `Handle::try_current()` check; `stream()` is sync and looks callable anywhere. Document
or return the error through the channel (`Handle::try_current().map_err(..)`).

### 🟢 F7 — `stream_from_model` only observes cancellation on the next provider event
`event_loop.rs:810-814`: a provider that ignores `StreamOptions.cancel` and stalls can never be cancelled. Parity with TS
(`for await`), so a nit; `tokio::select!` on `cancel.cancelled()` around `stream.next()` would make the loop robust to such providers.

## Verified OK (no finding)
- Locks: all `parking_lot` guards on `AgentInner` are scoped to closures/expressions (`with_messages`, `with_tool_registry`, `model()`,
  `cancel.write()` at `event_loop.rs:298`); no guard crosses an `.await` or a hook callback. `HookRegistry::invoke_callbacks`
  (`hooks/registry.rs:168-172`) snapshots callbacks under the mutex, then releases it before awaiting — hooks may add/remove hooks safely.
- Hook ordering/control fields: BeforeInvocation.cancel → assistant msg + AfterInvocation + result (TS parity); BeforeModelCall.cancel →
  synthetic `endTurn` + AfterModelCall with `retry` honoured; `AfterModelCallEvent.retry` honoured on success/denied/error paths with
  `attempt_count` incremented; `Error::Cancelled` from the model is never retried (`event_loop.rs:~740` check precedes `after.retry`);
  BeforeToolCall `cancel`/`selected_tool`/renamed `tool_use.name`, AfterToolCall `result`/`retry` (retry suppressed when cancelled),
  AfterTools `end_turn` (`true`/non-empty text/non-empty blocks; empty → continue) all match `agent.ts:1822-1831`, `executor.ts:141-190`.
- Tool results: USER message, order == tool_use order (Concurrent collects by index), cancelled-after-model path emits error results for
  every toolUse (`event_loop.rs:475-495`); `cancelled_result` appends "Cancelled by user" only on an open user turn.
- Retry loop termination on the error path is hook-bounded only — same as TS.
- `stream_aggregated`: empty stream / missing MessageStop / bad tool JSON → `Error::Model`, `MaxTokens` carries partial message (tests
  exist and pass); no index panics in `Aggregator`. `expect("anchor message has usage")` (`event_loop.rs:782`) is unreachable-false by
  construction.
- No `block_on`/`std::thread`/`Box::leak` in scope; `'static` is satisfied by cloning the `Arc<AgentInner>` handle into the task.
- Not fully checked in the time box: `run_invocation`'s `resume` loop (`event_loop.rs:330-395`), `tool_caller.rs`, `builder.rs`.

Baseline: `cargo test --all-features` → 541 + 29 + 24 + 1 passed, 0 failed (2 ignored). Tree left clean (`git status` empty; a transient `tests/scratch_validate.rs` from another process appeared and vanished during the run — not mine).

## Fixes applied (2026-10-14, branch `agent-tasks/strands-rs-fix-loop`)

- **F1** `event_loop.rs`: `struct InvokingGuard(Arc<AgentInner>)` created in `stream()` right after
  `invoking.swap(true)` and moved into a *watcher* task that `await`s the invocation `JoinHandle`,
  drops the guard, then forwards a `JoinError` as `Error::other("agent invocation task failed: …")`.
  The flag is now reset on panic/abort/runtime-shutdown and the caller sees the panic instead of
  "agent stream ended without a result".
  Test: `tests/agent_loop.rs::a_panicking_hook_releases_the_invocation_lock`.
- **F2** `AgentStream` holds the invocation's root `CancellationToken` and cancels it in `Drop`
  (`None` on the `ConcurrentInvocation` early-return path, so it never cancels someone else's run);
  `emit` now maps a closed channel to `Err(Error::Cancelled)` so the task unwinds instead of running on.
  Test: `tests/agent_loop.rs::dropping_the_stream_cancels_the_invocation`.
- **F3** `stream()` creates the root token (child of `options.cancel` when given) and installs it in
  `inner.cancel` *synchronously* before returning; `run_invocation` takes that root and derives a
  child per resume iteration, so a `cancel()` during `initialize()` cascades into the new invocation.
  Test: `tests/agent_loop.rs::cancel_right_after_stream_cancels_the_new_invocation`.
- **F4** `ConcurrentToolExecutor` converts a tool's `Err(Error::Cancelled)` into an
  `EXECUTION_CANCELLED_MESSAGE` error result for *that* tool and returns `Ok` with all results
  (Sequential/TS parity); the event loop then takes the normal path → assistant toolUse message +
  user tool-results message + `cancelled_result()` ("Cancelled by user", `StopReason::Cancelled`).
  Tests: `executor.rs::tests::concurrent_converts_a_tool_cancellation_into_an_error_result`
  (the old `concurrent_isolates_a_sibling_failure` encoded the buggy behaviour and was rewritten) and
  `tests/agent_loop.rs::a_tool_cancellation_leaves_results_for_every_tool_use`.
- **F5** `stream_aggregated` passes `Err(Error::Cancelled)` through unchanged (checked before the
  `is_model_error()` wrap); everything else keeps TS behaviour.
  Test: `models/model.rs::tests::cancellation_from_the_provider_is_not_wrapped_as_a_model_error`.
- **F6** `Agent::stream()` documents the `# Panics` (spawns, so it needs a tokio runtime).
- **F7** `stream_from_model` now `tokio::select!`s (`biased`) on `cancel.cancelled()` vs the next
  provider item, so a provider that ignores `StreamOptions.cancel` can still be cancelled. No test
  (would need a stalling model impl in the integration harness).
- Rustdoc: `sliding_window.rs` `window_size` no longer links the unexported `DEFAULT_WINDOW_SIZE`;
  `lib.rs` lines 3-4 use `[`models::Model`]` / `[`tools::Tool`]` (redundant explicit targets removed).

Gates: `cargo fmt` clean · `cargo clippy --all-targets --all-features -- -D warnings` clean ·
`cargo test --all-features` 542 + 33 + 24 + 1 passed, 0 failed (2 ignored) ·
`cargo doc --no-deps --all-features` 0 warnings.
Note: `--doc` tests are flaky in the shared target dir (`extern crate r#strands_agents` fails to
link while a parallel worker rebuilds the lib); they pass on re-run.
