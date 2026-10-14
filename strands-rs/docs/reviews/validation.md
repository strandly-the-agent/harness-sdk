# Port validation — TS `strands-ts` → Rust `strands-agents`

Validator run: 2026-10-14, repo `/mnt/workspace/harness-sdk` @ `agent-tasks/strands-rs` (24fd4a68e).
Crate: `strands-rs/crates/strands-agents`. Read-only validation.

## 1. Gates (verbatim)

```
=== fmt ===
$ cargo fmt --check
(no output — clean)

=== clippy ===
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized] target(s) in 0.15s

=== test ===
$ cargo test --all-features
test result: ok. 541 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.61s

=== build --no-default-features ===
    Finished `dev` profile [unoptimized] target(s) in 0.09s

=== build --examples --all-features ===
    Finished `dev` profile [unoptimized] target(s) in 0.11s

=== cargo doc --no-deps --all-features ===
warning: public documentation for `window_size` links to private item `DEFAULT_WINDOW_SIZE`
warning: redundant explicit link target
warning: redundant explicit link target
warning: `strands-agents` (lib doc) generated 3 warnings
```

fmt/clippy/test/builds: **clean**. `cargo doc`: **3 rustdoc warnings** (LOW, cosmetic).

### Test counts

`cargo test --all-features -- --list | grep -c ": test"` → **597**

| module prefix | tests |
|---|---|
| models::bedrock::format | 88 |
| tools::function_tool | 67 |
| conversation_manager::sliding_window | 66 |
| session::session_manager | 39 |
| session::file_storage | 30 |
| agent::tool_caller | 21 |
| retry::backoff | 19 |
| tools::executor | 18 |
| conversation_manager (mod) | 18 |
| retry::default_model_retry_strategy | 15 |
| models::bedrock::parse | 13 |
| session::validation | 12 |
| plugins::registry | 11 |
| models::bedrock (mod) | 11 |
| tools::structured_output | 10 |
| models::model | 10 |
| test_support | 8 |
| types::* | 23 (messages 7, media 5, tool 2, stop_reason 2, system_prompt 1, citations 1, session::types 5) |
| session::agent_snapshot | 5 |
| tools::* (tool/registry/noop) | 14 |
| plugins (mod/model_plugin) | 10 |
| hooks (registry/events) | 6 |
| models::streaming / defaults | 10 |
| logging / error / state_store / agent::* | 13 |
| integration tests (`tests/agent_loop*.rs`) | 53 |
| doctests | 3 (+2 ignored) |

## 2. Matrix audit (implementer-written matrices)

**Structural check (all rows, not a sample):** extracted every row marked `pass` from the six
matrices (462 rows) and resolved the named Rust test against the real set of `fn` names in the
crate. **Every row resolves** — 13 "unresolved" cells were continuation rows whose Rust cell is
prose (`same test`, `covered by the group above`), not a missing test.

| matrix | pass rows | unresolved test names |
|---|---|---|
| agent-loop | 49 | 0 |
| bedrock | 134 | 0 (12 `same test` continuation rows) |
| conversation-manager | 85 | 0 |
| executor | 27 | 0 |
| function-tool | 86 | 0 (1 "covered by group above") |
| session | 81 | 0 |

**Intent check (sampled rows, TS body vs Rust body read side by side):** 7 rows sampled across all
six matrices (seeded random). No row is wrong or missing. Three have *weaker* assertions than TS:

- **LOW** `conversation_manager/mod.rs:496 uses_200k_default_when_context_window_limit_is_undefined_and_logs_warning`
  — TS (`conversation-manager.test.ts:290`) also asserts `warnOnce` was called with
  "contextWindowLimit is not set on the model, using default of 200000". Rust asserts only
  `call_count() == 1`; the warning itself *is* implemented (`models/model.rs:116`) but unasserted,
  despite "and logs warning" in the test name.
- **LOW** `models/bedrock/mod.rs:796 clears_context_window_limit_when_model_id_changes_to_unknown_model`
  — TS asserts the pre-state (`contextWindowLimit === 1_000_000`) before `updateConfig`; Rust only
  asserts the post-state is `None`, so it would also pass if the limit had never resolved.
- **LOW** `tools/executor.rs:678 does_not_retry_after_the_supplied_signal_is_aborted` — TS also
  asserts `agent.cancelSignal.aborted === false` (the execution signal must not leak to the agent);
  Rust asserts only `calls == 1` and one result.

Verified-equivalent samples: bedrock `normalizes_a_hand_placed_ttl_to_the_configured_one` (exact
block order + ttl), sliding-window `removes_all_messages_when_window_size_is_0_on_context_overflow`,
agent-loop `turns_limit_wins_over_token_limits`. Mapping note: function-tool maps TS
`passes instanceof Tool` → `maintains_stable_interface_signature` — a reasonable Rust analog
(`ToolRef` trait object exercised end to end), not a defect.

## 3. Self-built matrices (modules with no implementer matrix)

Method: TS `it(...)`/`test(...)` names from the module's `__tests__/` vs Rust `#[test]` fns in the
corresponding module, token-overlap matched with a crate-wide fallback. **Counts are approximate**
(fuzzy name matching, and the Rust ports deliberately group several TS `it()`s into one test), so
read "unmatched" as "needs a human eye", not "missing".

| module | TS `it()` | Rust tests (module) | unmatched TS names |
|---|---|---|---|
| `retry` | 33 | 34 | 5 |
| `plugins` | 17 | 21 | 2 |
| `types` | 201 | 50 | many (grouped; see note) |
| `models/model.rs` | 41 | 10 | many (grouped; see note) |
| `hooks` | 95 | 6 | 81 (see note) |

Unmatched names worth a look:

- `retry`: `applies full jitter by default (Math.random() * raw)`, `clamps to maxMs`,
  `rejects non-integer attempts`, `is a ModelRetryStrategy`,
  `does not schedule a backoff timer when cancellation was already requested`. The first three are
  likely folded into the 19 `retry::backoff` tests; the cancellation one is behavioural and worth
  confirming explicitly.
- `plugins`: `has a no-op default when not overridden`, `is optional — plugins without getTools are
  valid` — both are trait-default-method cases that Rust gets structurally, plausibly skippable.
- `hooks`: the 95 TS tests are overwhelmingly `events.test.ts` constructor/field/
  `_shouldReverseCallbacks` assertions. Rust covers the same ground structurally (field access is
  compile-checked) plus `reverse_events_run_backwards_within_order`,
  `runs_in_order_then_registration_order`, `errors_stop_the_chain`, and the 53 integration tests in
  `tests/agent_loop*.rs` exercise every event's cancel/resume/replace path. **No gap found, but
  `hooks` is the one module with no written matrix and a 95→6 ratio — ask the implementer to produce
  one rather than taking my fuzzy match as proof.**
- `models/model.rs`: TS has 41 fine-grained `estimateTokens`/`estimateUtilization` cases; Rust has
  10 (`heuristic_text`, `heuristic_json`, `estimate_content_block_tokens`,
  `token_heuristic_counts_text_tools_and_system_prompt`, `estimate_utilization`, …) that assert the
  same heuristics in grouped form. Spot-checked `estimate_utilization` + the 200k default: present.
- `types`: 201 → 50 with the same grouping pattern; wire-shape coverage is additionally proven by
  §4 below.

## 4. Wire-compat spot check

Scratch test `crates/strands-agents/tests/scratch_validate.rs` (deleted after the run; copy kept at
`reports/wire-compat-scratch.rs.txt`), deserialize → re-serialize → assert JSON equality against
TS-shaped fixtures:

```
running 3 tests
test file_storage_snapshot_file_roundtrips ... ok
test message_with_tool_use_tool_result_reasoning_and_image_roundtrips ... ok
test model_stream_event_sequence_roundtrips ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**PASS** — all three round-trip byte-identically:
1. `Message` with `text` + `toolUse` + `toolResult` (json **and** text content) + `reasoning`
   (`redactedContent` base64) + `image` (`source.bytes` base64) + `metadata.usage`.
2. A `snapshot_latest.json` as `strands-ts` `FileStorage` writes it (`scope`/`schemaVersion`/
   `createdAt`/`data{messages,state,systemPrompt}`/`appData`) — and the nested messages re-parse as
   `Message`.
3. A 10-event `ModelStreamEvent` sequence (messageStart → blockStart/textDelta×2/blockStop →
   toolUseStart/toolUseInputDelta/blockStop → messageStop(`tool_use`) → metadata).

Process note: my first stream fixture used `delta: {text: …}` and failed; the Rust shape was right
and **my** fixture was wrong — TS tags nested deltas too (`streaming.ts:383,418,434`:
`toolUseStart` / `textDelta` / `toolUseInputDelta`). Corrected fixture passes. No Rust defect.

## 5. Sensitive-surface diff

`rg -n "std::env|env::var|tokio::fs|std::fs|TcpStream|reqwest|Command::new|process::"` over
`src/` + `examples/`:

| capability | Rust | TS equivalent | verdict |
|---|---|---|---|
| Filesystem | **only** `session/file_storage.rs` — `read`, `write`, `create_dir_all`, `rename` (tmp→final atomic write, `:103-104`), `read_dir`, `remove_dir_all` | `session/file-storage.ts` does the same, incl. the tmp+rename write (`:174`) | match |
| Network / credentials | `models/bedrock/mod.rs:323` `aws_config::defaults(BehaviorVersion::latest()).load()`, then the `aws-sdk-bedrockruntime` client | `models/bedrock.ts:469` `new BedrockRuntimeClient({…})`, same default credential/region chain | match |
| Env vars | no `env::var` / `std::env` anywhere in `src/`; env is read only indirectly by `aws_config`'s default chain | TS likewise reads env only via the AWS SDK default chain | match |
| Process spawn | none (`Command::new` / `process::` absent) | none | match |

Both AWS crates are behind the optional, default-on `bedrock` feature; `--no-default-features`
builds clean, so the no-network/no-credentials configuration is real. **Nothing the Rust side does
that TS doesn't.**

## Verdict

**PASS-WITH-FINDINGS** — no blocking items.

Gates green (fmt, clippy `-D warnings`, 597 tests / 0 failures, default + `--no-default-features` +
examples build). All 462 "pass" matrix rows resolve to real tests; sampled intent checks match.
Wire-compat round-trips byte-identical on all three fixtures. Sensitive surface is a subset-or-equal
of TS.

Non-blocking, for the implementer:
1. **LOW** three sampled tests assert less than their TS originals (200k-default warning, bedrock
   pre-state, executor `cancelSignal` non-leak) — §2.
2. **LOW** `cargo doc` emits 3 rustdoc warnings (`window_size` → private `DEFAULT_WINDOW_SIZE`,
   2× redundant explicit link target).
3. **LOW/process** `hooks` has no traceability matrix and a 95 TS → 6 Rust test ratio. I found no
   actual gap, but this is the one module where I can't hand you row-level proof — ask for a matrix.
4. **LOW** confirm `retry`'s "does not schedule a backoff timer when cancellation was already
   requested" is covered.
