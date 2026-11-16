# Phase 11c review: correctness (commit 34ae5dfeb)

Status: complete. Scope: the correctness fixes in `824647bf6..34ae5dfeb` and the library behaviour changes other fixes made.

## Probes rerun at 34ae5dfeb (scratch crate `/tmp/reviewsdk3_correctness/probe`, path dep on the review worktree)

All first-pass and second-pass correctness probes that have a fix now show the fixed behaviour:
`after_invocation_drop`, `agent_as_tool_reset`, `bash_restart`, `direct_tool_after_drop`, `session_restore_background`, `skills_same_id` (pass 1); `cancel_stalled_model`, `mcp_connect_race::concurrent_first_calls_share_one_connection` (3000 rounds: 0 doubled, 0 failed), `mcp_drop` (3 cases), `otel_drop::dropped_mid_memory_search` (`memory.search`/`memory.inject` end), `examples/hitl_stale` (`delete_everything ran=false`), `examples/macro_cfg`, `macro_cfg_fn`, `macro_ident`, `macro_ident2` (spanned errors, no panic). Two probes needed an import/constructor update for API renames (`InMemoryStorage::default()`, `vended_interventions::hitl::…`).

## Findings

1. `strands-rs/strands/src/tools/tool.rs:329-331` (rustdoc of `ToolError::from_boxed`, commit `30cdc45aa`, api-b1) and `:285-287`: the doc says an `anyhow::Error` passed to `from_boxed` "stays the source, so `downcast_ref` finds it". It does not. `anyhow::Error: Into<Box<dyn Error + Send + Sync>>` goes through `into_boxed_dyn_error`, whose box has anyhow's `ErrorImpl<E>` as its concrete type; anyhow documents that it "can no longer downcast" (`anyhow-1.0.104/src/error.rs:597`). Evidence: `/tmp/reviewsdk3_correctness/probe/examples/x_anyhow.rs` prints `anyhow: downcast_ref::<ParseIntError> = None` and `box: downcast_ref::<ParseIntError> = Some(ParseIntError { kind: InvalidDigit })`. The doctest only covers the `BoxError` case, and `strands-macros/tests/ui/pass/boxed_error.rs` has no anyhow case. Fix: say in the `from_boxed`/`downcast_ref` rustdoc that the downcast works for a `Box<dyn Error>` holding the error itself, and that an `anyhow::Error` must be converted with `reallocate_into_boxed_dyn_error()` (or downcast before conversion) for `downcast_ref` to find the typed error; or drop the anyhow claim.

(Probe note) The Anthropic retry-sleep probe the correctness-b5 disposition says was not rerun passes at this commit: `anthropic_transport::cancel_during_the_retry_after_sleep` prints `cancel at 0.3s: returned after 0.30s stop=Cancelled requests=1`. The lib test binary already in the shared target dir (built from this worktree by another reviewer, 4368 tests) passes in full.

2. `strands-rs/strands/src/mcp/tests/robustness.rs:484-505` and `:507-534` (`a_dropped_call_cancels_the_request_and_frees_its_progress_route`, `a_dropped_invocation_cancels_its_mcp_tool_call`, commit `c2d68f35e`, correctness-b1): both run on `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` with the clock running and wait through `server::eventually` (`mcp/tests/server.rs:444-452`: `tokio::time::sleep(1 ms)` up to 5000 times), so they sleep in real time and a regression costs 5 s of wall clock per wait. `strands-rs/docs/TESTING.md:29`: "Never sleep for real … use `tokio::time::pause()`". Nothing in either test needs a second worker: the `OpenCall` drop spawns on the current handle. Evidence: in a scratch copy of the crate (`/tmp/reviewsdk3_correctness/ws`) with both attributes changed to `#[tokio::test(start_paused = true)]`, `cargo test -p strands --lib --no-default-features --features mcp -- a_dropped_call_cancels a_dropped_invocation_cancels` gives `2 passed … finished in 0.01s`. Fix: `#[tokio::test(start_paused = true)]` on both (the form `agent_shutdown_does_not_disconnect_the_mcp_client` at `:438` already uses with the same server).

3. `strands-rs/strands/src/vended_tools/bash/tests.rs:508-515` (`restart_stops_a_running_command`, commit `d77992e14`, correctness-5): the test asserts `started.elapsed() < Duration::from_secs(4)` on `std::time::Instant` around a real `sleep 5`, a wall-clock bound that fails on a stalled runner for a reason unrelated to the code (TESTING.md:29). The bound is also redundant: with the pre-fix `bash.rs` (`git show d77992e14^:…/vended_tools/bash.rs`) and the elapsed assertion removed, the test still fails, at `running.unwrap_err()` (`tests.rs:518` at this commit) (`called Result::unwrap_err() on an Ok value: {"output": "", "error": ""}`, after 5.0 s), because a restart that waits lets the command finish. Fix: delete the `Instant`/`elapsed` assertion (lines 508 and 512-515); `running.unwrap_err()` already proves the command was stopped.

## Fix commits read and checked for new defects (none found beyond the findings above)

- `d85628419` (correctness-1, P-740): `in_pass` is already false when `after_invocation` sets `abandoned`, so `RunGuard::drop` -> `abandon()` keeps the entry; cleared before the emit. Probe passes.
- `f5433fff5` (correctness-4, P-743), `c8063d2e1` (correctness-3, P-742), `04e94d413` (correctness-2, P-741): probes pass; the `Weak<()>` identity cannot be reused while an entry holds the weak.
- `d77992e14` (correctness-5, P-744): the token is outside the session mutex; the stopped arms drop the local `process`, whose `Drop` kills it; a queued call fails before spawning.
- `c2d68f35e`, `bff63d7d8` (correctness-b1, P-797): no lock is held across an await; `OpenCall::drop` takes the `progress` std mutex only for the `remove`; the id is cleared on every path that got a response or sent its own cancel, so no path sends two cancels except a drop during rmcp's own timeout cancel (harmless). Probes pass.
- `860095f9e` (correctness-b6, P-800): lock order is `connecting` -> `state` only; `disconnect` and `settle` never hold `state` while taking `connecting`. 3000-round probe: 0 doubled, 0 failed.
- `a4224f2f6` (correctness-b2, P-798): probe `hitl_stale` prints `delete_everything ran=false`.
- `c8cf6ec5f`, `012247c17` (correctness-b5, P-799): the only `stream_aggregated` implementation in the crate (`models/model.rs:417-437`) ends synchronously after `Done`, so a ready `Done` still wins over a fired signal; the cancel arm goes through the existing cancelled branch (model span ended once).
- `e2aa9e839`, `2c499cd5e` (correctness-b8, P-801): `OpenSpan::end()` sets `ended` before every `end_*` call; no path ends a span twice.
- `8346c4f2e` (b9), `d7971a084`, `d5678d1e3`, `1516f1e46` (b3, b4): extra scratch cases compile: `#[tool]` before `#[cfg]`, two cfg-exclusive methods with one name, every tool cfg'd off under `#![deny(warnings)]`, `#[tools]` above an impl-level `#[cfg(any())]`. `#[cfg_attr(.., cfg(..))]` fails with `E0599` as P-817 declares.
- `d4eba3961` (P-1000): `Tracer::end_cycle` takes the span, so the `abandon()` call after a drop inside the `InterruptEvent` hooks is a no-op; TS `_createInterruptResult` (`agent/agent.ts:2236-2252`) attaches metrics and traces for every interrupt, as the Rust result now does.
- `094bfdf87`, `568159abe` (P-1004): matches TS `_defaultBuilder` (`multiagent/spec.ts:460-501`); `tool_agent()` takes the telemetry lock at statement level only.
- `a3c5efce4`, `deabb08a4` (P-791, P-792), `2b136ac63` (P-745), `8a8b0db43`, `77992f408` (P-804), `b1af221c6` (P-803), `4aab79f7d`, `46e49640b`: read; nothing found.

## Verdict

The correctness fixes hold: every reviewer probe now shows the fixed behaviour and I found no new library defect in the fix diff; the three findings are one false rustdoc claim (`ToolError::from_boxed` with `anyhow::Error`) and two test-rule violations (real-time waits).

## Not reached

- `concurrent_reconnects` (two racing `connect(true)` still open two connections and fail in-flight calls: `rounds=1500 rounds_with_a_failed_call=1442 connections_opened=4500`); the second-pass report lists it as matching TS and the fix did not claim it, so I did not compare the failure rate with the pre-fix commit.
- The pre-fix failure of each new in-crate test (only the bash one was rerun against pre-fix code).
- `e1e0aa589`/`c361cfffe` fixture and manifest tests (correctness-b7), the conformance generator, and the `test(...)`/`docs(...)` commits of the range.
- API-only commits (`f025164c1`, `26f4a31c7`, `7d11c11fd`, `e631a20d7`, `0282d63d8`, `a9a53b99d`, `891b3dde5`, `a7472b7dc`, `83406ccf0`, `f22a3a62c`, `2de4dc8cf`) beyond a skim; `strands-harness`; `--all-features` and `--no-default-features` builds of the whole crate (I ran the probes with `mcp,otel,anthropic,openai`, and the lib test binary another reviewer had built from this worktree in the shared target dir, features unknown).
- `phase-10.md` rows other than correctness-7 and parity-9.
