# Phase 3 review — strands-rs event loop & friends

Branch `agent-tasks/strands-rs` @ 409b68a5d. Started 20:42 UTC, 15-min budget. No `spawn` tool in this run — single-pass, read by hand.
Findings appended incrementally; tiers 🔴 block / 🟡 should-fix / 🟢 checked-ok.

## Findings

🔴 `models/anthropic/mod.rs:394`, `models/openai/mod.rs:113` — **UTF-8 split across HTTP chunks silently corrupts model text.** Each body chunk is decoded independently with `String::from_utf8_lossy(&chunk)` before being fed to `SseParser::push(&str)`. reqwest chunk boundaries are arbitrary bytes; any multi-byte char (é, CJK, emoji) straddling a boundary becomes two U+FFFD. The JSON still parses, so the user gets a wrong answer with no error. Reachable from any non-ASCII model output; deterministic given the boundary (`from_utf8_lossy` contract). Fix: make `SseParser` buffer `Vec<u8>` and `push(&[u8])`, or keep a carry: `std::str::from_utf8(&bytes)` → on `Utf8Error` with `error_len().is_none()`, consume `valid_up_to()` and stash the tail for the next chunk.

🟡 `models/sse.rs:26-27` — **CRLF split across chunks forges an event boundary.** `chunk.replace("\r\n","\n").replace('\r',"\n")` runs per chunk, so `...\r` | `\n...` becomes `\n\n` → the block is cut early. For Anthropic framing (`event: x\r\n` | `\ndata: {...}`) this yields an `event`-only event plus a `data`-only event, which `StreamState::handle` won't match. Fix: normalise after appending to `self.buffer` (and hold back a trailing `\r`), not per chunk. Fold into the byte-buffer fix above.

🟢 `models/anthropic/mod.rs:410-422` — `pause_turn` loop is bounded by `MAX_PAUSE_TURN_CONTINUATIONS` and appends the partial assistant message before re-posting; cancel checked per chunk. OK.

🟡 `sandbox/local.rs:122-127` — **stdin write runs before the cancel/timeout `select!` and before stdout/stderr collectors start.** `pipe.write_all(stdin).await` with >64 KiB of stdin against a child that fills its own stdout pipe first (nobody is reading it yet) deadlocks both sides; `options.timeout`/`cancel` aren't armed until line 134, so the call hangs forever. Fix: start the collectors first and move the stdin write into a future raced inside the `select!` (or `tokio::spawn` it and drop the handle on exit).

### Repro (scratch test, run 20:47 UTC, file removed afterwards)
`cargo test --all-features -p strands-agents --test scratch_review -- --nocapture` → 0 passed, 2 failed:
- `utf8_char_split_across_http_chunks_is_corrupted`: Anthropic mock transport, body split between the two bytes of `é` → decoded text `"caf��"` (expected `"café"`). Confirms the 🔴.
- `crlf_split_across_chunks_forges_event_boundary`: `push("event: content_block_delta\r")` + `push("\ndata: {\"a\":1}\r\n\r\n")` → `[SseEvent{event:Some("content_block_delta"), data:""}, SseEvent{event:None, data:"{\"a\":1}"}]` (expected one event). Confirms the 🟡.

🟡 `tools/executor.rs:238-239` — **tool-call span and meter never closed when a tool interrupts.** `let span = start_tool_call_span(..); let outcome = run_tool_chain(..).await?;` — `Error::Interrupt` from the tool (the HITL path) returns through `?` before `end_tool_call_span`/`meter().end_tool_call`. `SpanHandle` has no `Drop` (`telemetry/tracer.rs:19`), so the OTel span is never ended/exported and the local trace tree keeps an open span for every interrupted tool. Fix: bind `outcome` without `?`, end the span (error text = interrupt), then `return Err(..)` for control-flow errors.

🟡 `agent/mod.rs:310-314` (`take_continuation_input`) — **`on_appended` fires before the messages are appended, and inputs can be lost.** `prepare()` is followed immediately by `mark_appended()`; the messages only reach history later inside `run_cycles` (`event_loop.rs:649-653`). TS calls `continuations.markAppended` *after* pushing to `this.messages` (`strands-ts/src/agent/agent.ts:2673-2695`). Consequence: the callback observes a history that doesn't yet contain the message; and if `run_cycles_inner` bails before appending (e.g. `INTERRUPT_RESUME_REQUIRED` at `event_loop.rs:610-613`), the queued inputs were already drained and reported "appended" — they are silently dropped instead of abandoned (spec m4b: "abandons follow-up input when the stream closes before it is appended"). Fix: call `mark_appended()` from `run_cycles_inner` right after the `pending_input` append loop; route early-exit paths through `continuations.abandon(..)`.

🟡 `agent/event_loop.rs:472-477` — **continuation + prompt produce consecutive same-role messages.** `messages.extend(current.into_messages())` just concatenates; TS `_appendMessages` merges a message into the trailing message of the same role (`agent.ts:2673-2691`). Two adjacent `user` messages are rejected by Bedrock Converse ("conversation must alternate") — the Anthropic API tolerates it, Bedrock does not. Fix: merge same-role neighbours in `append_message` (or in this block) as TS does. Not run against Bedrock; based on reading TS + the Converse contract.

🟢 Checked, no finding: `middleware/registry.rs:29-35,224-236` phase order Input→Output→Wrap + stable sort matches TS `PHASE_ORDER`; `execute_tools` resume path replays assistant msg + restores completed results (`event_loop.rs:655-680`), BeforeTools-hook interrupt stores pending with `restored` (`1353-1356`); executor defers interrupts until siblings finish and keeps their results (`executor.rs:144-178`); background route skips AfterToolCall exactly like TS `executor.ts:155-165`; `background_tasks/mod.rs:697-786` permit released on abort, Cancelled guard via `update_working`, timeout drops the tool future; `sandbox/local.rs` `process_group(0)` + `GroupKill` drop guard + `kill_group` on every exit; `snapshot.rs:163` `messages: null` errors — TS throws too; `mcp/client.rs:338-345` disconnect idempotent and cancels the rmcp service; no parking_lot guard held across `.await` found in scope (all `lock()` sites inspected are sync blocks); `interrupt.rs:82-92` id formats match `tool:{id}:{name}` / `hook:beforeToolCall:{id}:{name}` / `hook:beforeTools:{name}`.

### Not covered in the 15 min
`interventions/` + `vended/hitl.rs` decision rules/±90 order, `agent/checkpoint.rs` resume positions, `telemetry/meter.rs` lock discipline beyond grep, `mcp/tool.rs` error mapping, OpenAI `pause_turn`-equivalent.

### Repo state
Scratch test removed. `git status` shows `strands-rs/AGENTS.md`, `docs/DESIGN.md`, `docs/PORTING.md` modified at 20:42:34 UTC — not by me (I only read source; another validator is working in the tree). Left untouched.

## Fixes applied

Branch `agent-tasks/strands-rs-loop-fix3` (worktree `/home/sandbox/wt/loop-fix`), 4 commits `2378d6099..f2d561e17`.
Each fix has a regression test that was confirmed red against the pre-fix code where a repro was cited.

1. 🔴 **UTF-8 split across chunks** — `models/sse.rs`: `SseParser` now takes bytes (`push(&[u8])`), decodes
   with `std::str::from_utf8` and holds back an incomplete trailing sequence (`valid_up_to()` /
   `error_len() == None`); truly invalid bytes still become U+FFFD. `anthropic/mod.rs:394` feeds the raw
   chunk; `OpenAITransport::send` now yields `Result<Vec<u8>>` (was `Result<String>`, lossy-decoded in
   `ReqwestTransport`) and `decode` takes a byte stream — **public API change on `OpenAITransport`**.
   Tests: `models::sse::tests::multi_byte_char_split_across_chunks_is_preserved`,
   `finish_flushes_incomplete_utf8_tail_lossily`, `invalid_utf8_bytes_become_replacement_chars`,
   `models::anthropic::tests::multi_byte_text_split_across_body_chunks_is_not_corrupted` (body split
   between the two bytes of `é` *and* inside a CRLF, through the transport).
2. 🟡 **CRLF split across chunks** — normalisation moved after the append to `self.buffer`, holding back a
   trailing `\r` until the next chunk (`finish()` normalises unconditionally).
   Test: `models::sse::tests::crlf_split_across_chunks_keeps_one_event` (exact split from the report).
3. 🟡 **Tool span leaked on interrupt** — `tools/executor.rs`: `run_tool_chain` is bound without `?`; the
   error arm ends the tool span (error text = the interrupt) and records `end_tool_call` before
   `return Err(..)`. Test: `interrupts_loop::an_interrupted_tool_still_ends_its_tool_span` (asserts the
   `Tool: approve` local trace has an `end_time`; fails pre-fix with `end_time: None`).
4. 🟡 **Continuation inputs resolved too early** — `Agent::take_continuation_input` only `prepare()`s now;
   `mark_appended()` moved into `run_cycles_inner` right after the `pending_input` append loop, and
   `run_cycles` calls `continuations.abandon(..)` after the inner call so any pass that never appended
   (e.g. `INTERRUPT_RESUME_REQUIRED`) abandons instead of dropping silently.
   Tests: `agent_loop_extra::continuation_input_merges_with_the_prompt_and_reports_after_appending`,
   `agent_loop_extra::continuation_input_is_abandoned_when_the_pass_never_appends`.
5. 🟡 **Consecutive same-role messages** — `event_loop.rs` `merge_same_role` folds each message into the
   previous one of the same role when continuation input is concatenated with the prompt (TS
   `_appendMessages`). Covered by the merge test above (two user inputs → one user message, 2 blocks).
6. 🟡 **Sandbox stdin deadlock** — `sandbox/local.rs`: the stdin pipe is taken before the collectors start
   and the `write_all`/`shutdown` now runs as a `feed_stdin` future raced inside the cancel/timeout
   `select!` (it ends in `pending()`, so it never wins the race).
   Test: `sandbox_loop::large_stdin_against_a_full_stdout_pipe_returns_promptly` (1 MiB to `cat`, 2 s
   sandbox timeout, 10 s outer guard; pre-fix it hits the outer guard at 10.00 s).

Gates from `/home/sandbox/wt/loop-fix/strands-rs` (last lines verbatim):
- `cargo fmt` → (no output)
- `cargo clippy --all-targets --all-features -p strands-agents -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 21.11s`
- `cargo test --all-features` → `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` (last of the per-target lines; totals across targets: 1210 passed, 0 failed, 39 ignored)
