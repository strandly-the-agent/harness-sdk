# Phase-3 validation (independent validator)

Repo `/mnt/workspace/harness-sdk`, branch `agent-tasks/strands-rs`, workspace `strands-rs/`.
Read-only on source; no source files modified (`git status --short` clean before and after).

## 1. Gates — verbatim

```
$ cargo fmt --check
(no output, exit 0)

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized] target(s) in 22.85s

$ cargo test --all-features 2>&1 | grep -E "^test result|FAILED|panicked"
test result: ok. 880 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.51s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.22s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 22 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 209 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.02s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo build --no-default-features -p strands-agents 2>&1 | tail -1
error: could not compile `strands-agents` (bin "mcp_echo_server") due to 6 previous errors; 2 warnings emitted

$ cargo doc --no-deps --all-features 2>&1 | grep -cE "^warning"
3
```

Totals: **1241 tests pass, 0 fail**; 39 `#[ignore]`d (live-provider integ).
fmt clean, clippy clean under `-D warnings`.

### BLOCKING: the crate does not build without `--all-features`

`crates/strands-agents/src/bin/mcp_echo_server.rs` is **auto-discovered** — `crates/strands-agents/Cargo.toml`
has no `[[bin]]` section (`grep -c '\[\[bin\]\]'` → 0), so the bin is built in *every* feature
configuration, but it only compiles when the test-only `mcp-echo-server` feature turns on
`rmcp/server`, `rmcp/transport-io`, `rmcp/macros` (`Cargo.toml:25`). Both of these fail today:

```
$ cargo build --no-default-features -p strands-agents
error[E0433]: cannot find module or crate `rmcp` in this scope   (x3)
error[E0432]: unresolved import `rmcp`
error[E0599]: no method named `serve` found for struct `EchoServer` in the current scope
error[E0599]: no method named `waiting` found for type `!` in the current scope
error: could not compile `strands-agents` (bin "mcp_echo_server") due to 6 previous errors

$ cargo build -p strands-agents          # DEFAULT features — also broken
error: could not compile `strands-agents` (bin "mcp_echo_server") due to 5 previous errors
```

So a plain `cargo build` / `cargo install` of the published crate fails for every downstream user.
Fix is two lines in `crates/strands-agents/Cargo.toml`:

```toml
[[bin]]
name = "mcp_echo_server"
required-features = ["mcp-echo-server"]
```

`telemetry.md` records that this bin was "restored … it broke `--all-features` on the integration
branch" — the restore fixed `--all-features` and silently broke every other configuration. The
phase-3 reports' own gate claims were run with `--all-features` only, which is why nobody caught it.

### Doc warnings (3 → 2 real)

- `crates/strands-agents/src/agent/mod.rs:378` — unresolved intra-doc link `Error::DefaultNotConfigured` (no `Error` in scope).
- `crates/strands-agents/src/hooks/events.rs:148` — redundant explicit link target.

## 2. Test counts

Per crate (`cargo test --all-features`): `strands-agents` lib 880 + 209 `strands-harness` lib +
integration binaries below.

Per top-level module (`cargo test --all-features -- --list | grep ": test" | sed 's/::.*//' | sort | uniq -c | sort -rn`):

```
276 models      188 tools       91 session      81 plugins     75 agent
 74 memory       40 storage      34 retry        33 context_manager
 30 sandbox      24 builtin_tools 24 background_tasks 23 mcp
 20 types        16 telemetry    14 middleware   10 interventions
  7 interrupt     7 hooks         4 prompt        4 logging
  3 state_store   3 error         8 test_support
```

## 3. E2E / integration inventory (`crates/*/tests/`)

| file | tests | real external process/service? |
|---|---|---|
| `strands-agents/tests/agent_loop.rs` | 33 | no (mock model) |
| `strands-agents/tests/agent_loop_extra.rs` | 24 (1 ignored) | no |
| `strands-agents/tests/agent_as_tool_loop.rs` | 2 | no |
| `strands-agents/tests/background_tasks.rs` | 6 | no |
| `strands-agents/tests/checkpoint_loop.rs` | 8 | no |
| `strands-agents/tests/context_manager_loop.rs` | 1 | no |
| `strands-agents/tests/interrupts_loop.rs` | 7 | no |
| `strands-agents/tests/middleware_loop.rs` | 5 | no |
| `strands-agents/tests/mcp_e2e.rs` | 2 | **yes** — spawns the real `mcp_echo_server` stdio binary via `CARGO_BIN_EXE_mcp_echo_server` (`mcp_e2e.rs:16`) |
| `strands-agents/tests/sandbox_loop.rs` | 7 | **no** — uses an in-test `RecordingSandbox` mock (`sandbox_loop.rs:21-62`), *not* real processes |
| `strands-agents/tests/telemetry.rs` | 4 | no — `InMemorySpanExporter` (`telemetry.rs:7`), as expected |
| `strands-agents/tests/tool_macro.rs` | 10 | no |
| `strands-agents/tests/integ_anthropic.rs` | 4 | live, `#[ignore]`d |
| `strands-agents/tests/integ_bedrock.rs` | 22 | live, `#[ignore]`d |
| `strands-agents/tests/integ_openai.rs` | 2 | live, `#[ignore]`d |
| `strands-harness/tests/integ_harness.rs` | 7 | live, `#[ignore]`d |
| `strands-harness/tests/prompt_parity.rs` | 1 | no |

Correction to the brief: **real-process execution is not exercised by `sandbox_loop.rs`**. It *is*
covered — by the 30 in-module tests in `crates/strands-agents/src/sandbox/`, where
`local.rs:94` spawns a real `sh` via `tokio::process::Command` (e.g.
`runs_code_through_the_named_interpreter`, `local.rs:512`).

### Live-run logs — genuine

Both `strands-rs/docs/reviews/integ-bedrock-run.log` and `integ-harness-run.log` are authentic
cargo harness output (Compiling/Finished lines, deps hash in the binary path, per-test `... ok`
lines, timing):

- `integ-bedrock-run.log`: `running 22 tests` … `test result: ok. 22 passed; 0 failed; 0 ignored; … finished in 12.77s`
  — count matches the 22 `#[ignore]`d tests in `integ_bedrock.rs`.
- `integ-harness-run.log`: `running 7 tests` … `test result: ok. 7 passed; 0 failed; 0 ignored; … finished in 8.79s`
  — matches `integ_harness.rs`'s 7 tests (incl. `interventions_deny_the_shell_tool`, `persists_memory_across_agents`).

## 4. Matrix audit

**Only 5 of 14 phase-3 modules have a matrix.** Stub reports (STATUS/NEXT only, no matrix, no
captured gate output): `middleware.md` (4 lines), `interrupts.md` (8), `snapshots.md` (11),
`telemetry.md` (7), `mcp.md` (7), `anthropic.md` (6), `agent-as-tool.md` (7). `delegation.md`
does not exist (content is in `delegation-matrix.md`). `openai.md`, `sandbox.md`, `memory.md`,
`background-tasks.md`, `harness-wiring.md`, `harness-models-providers.md` are real.

Sampled 18 "pass" rows across `delegation-`, `background-tasks-`, `sandbox-`, `openai-`,
`memory-matrix.md`. **All 18 named Rust test fns exist**; no fabricated rows found.

Intent-checked 4 bodies — all four genuinely assert what the TS row claims:

- `cancels_the_batch_when_a_delegation_tool_is_called_alongside_another_tool` (`src/agent/delegation.rs:284`) — asserts *both* tool results carry the verbatim `DELEGATION_SINGLE_CALL_CANCEL` text and that the model is called twice (retry). Faithful.
- `force_fails_non_terminal_tasks_on_restore` (`src/background_tasks/mod.rs:1613`) — persists a `Working` task, reloads, asserts `Failed` + `ExecutionError`/`RESTORE_FAILURE_MESSAGE`. Faithful.
- `classifies_throttling_before_context_overflow` (`src/models/openai/tests.rs:455`) — asserts 429 and `rate_limit_exceeded` win over context-overflow text, including the mixed `429 + context_length_exceeded` case. Faithful (precedence is the point of the test).
- `search_resolves_max_results_caller_then_store_then_default` (`src/memory/memory_manager.rs:1011`) — asserts store default 7, then caller override 1. Faithful.

The matrices are honest about gaps: they mark unported rows `skipped` with a reason (e.g.
delegation's two-simultaneous-delegation-tools row, memory's empty-`stores`-array and
dedup/metadata rows) rather than claiming pass.

### Count parity for the matrix-less modules (TS inventory vs Rust)

| module | TS tests (survey3) | Rust tests | gap |
|---|---|---|---|
| middleware | 127 (`m4a`: 62 + 21 + 5 + 15 + 24) | 14 (`middleware`) + 5 (`middleware_loop.rs`) = 19 | ~108 unported |
| MCP client | 219 (`mcp3` 177 + `mcp3b` 42) | 23 (`mcp`) + 2 (`mcp_e2e`) = 25 | ~194 unported — all of `mcp3b` (MCP *tasks*: metadata fetch, legacy polling, abort/refetch, queued input) and client annotations/progress/pagination |
| telemetry | ~211 (`t2`: tracer 97, meter 62, config 6+15, json 15, local-trace 16) | 16 + 4 = 20 | ~191 unported |
| sandbox | 109 (`sb2`) | 30 + 7 = 37 | ~72 unported (docker/browser/e2b variants) |
| interrupts + interventions | `i3a`/`i3b` (5 describe files + HITL/steering/cedar) | 7 (`interrupt`) + 10 (`interventions`) + 7 (`interrupts_loop`) = 24 | partial; multi-agent round-trip and steering/cedar unported |
| anthropic + openai | `p3a` ~60 in the caching group alone + `p3b` chat/responses/mantle/errors | within `models` 276 (openai `tests.rs` 18+, anthropic `tests.rs` 39+) | partial, not individually traceable without a matrix |

Most important missing names (highest behavioural risk): the whole MCP task lifecycle
(`waits for queued input on tasks/result beyond the request timeout without progress`,
`legacy polling spans multiple requests within the overall pollTimeout`,
`refetches the metadata after an aborted fetch`), middleware `copy-on-input` contract (21 its —
the copy/reference semantics are easy to get wrong in Rust and are untested here), and
telemetry `tracer`/`meter` attribute coverage.

These counts are *inventory* counts, not a claim that each TS test must exist 1:1 — Rust table
tests legitimately collapse several. But without a matrix for 7 of 14 modules there is no way to
tell collapse from omission, which is the main reviewability gap in this phase.

## 5. Feature-list check

| feature | verdict | evidence |
|---|---|---|
| interrupts / HITL | PRESENT | `src/interrupt.rs`, `types/interrupt.rs`, `Error::Interrupt`, `src/interventions/` + vended `hitl` (`AskFn`), `tests/interrupts_loop.rs` (7) |
| checkpoints / snapshots | PRESENT | `src/agent/snapshot.rs`, `src/agent/checkpoint.rs`, `AgentBuilder::checkpointing`, `InvokeInput::CheckpointResume`, `tests/checkpoint_loop.rs` (8) |
| background tasks engine | PRESENT | `src/background_tasks/mod.rs` (24 tests) — max_concurrency/timeout/dedup, `rejects_a_tool_configured_in_two_modes`, `rejects_out_of_range_timer_delays`, `rejects_snapshot_loads_while_tasks_are_tracked` (cancel guard), `force_fails_non_terminal_tasks_on_restore` |
| middleware | PARTIAL | `src/middleware/{mod,registry,sink,stages}.rs` + loop/executor wiring present and tested (19), but the TS copy-on-input contract (21 its) is unported |
| agent-as-tool / delegation | PRESENT | `src/agent/{agent_as_tool,delegation,continuation}.rs`, `tests/agent_as_tool_loop.rs`, `delegation-matrix.md` |
| MCP | PARTIAL | rmcp `"3"` (3.5.1) with `client` + `transport-child-process` + `transport-streamable-http-client` (`Cargo.toml:42`); stdio + streamable-http in `src/mcp/config.rs:18-98` (SSE rejected); real-stdio e2e in `tests/mcp_e2e.rs`. Missing: MCP tasks (all of `mcp3b`), annotations, progress, list pagination |
| sandbox | PARTIAL | `src/sandbox/{mod,local}.rs`, real-`sh` execution, `SandboxOption` in the harness; only the local backend (no docker/e2b) |
| telemetry | PARTIAL | `src/telemetry/*` (16), `tests/telemetry.rs` span-tree assertions with `InMemorySpanExporter`, harness env-gated `setup_telemetry()`; TS tracer/meter/json/local-trace suites largely unported |
| memory manager | PRESENT | `src/memory/{memory_manager,file_memory_store}.rs` (74), `memory-matrix.md`, live `persists_memory_across_agents` |
| Anthropic / OpenAI providers | PRESENT | `src/models/anthropic/*`, `src/models/openai/*` (+ `mantle.rs`), shared `models::sse::SseParser`, `integ_anthropic.rs` (4) / `integ_openai.rs` (2) |
| conversation managers removed | PRESENT | `rg -ln "ConversationManager\|SlidingWindow" crates/strands-agents/src` → no hits |
| context manager default Auto | PRESENT | `ContextManagerOption::Auto` is `#[default]` (`src/context_manager/mod.rs:110-113`) |

## 6. Harness default parity re-check (vs `survey2/h1-agent-defaults.md`)

Three of phase 2's DIFFs are now fixed; nothing regressed.

| default | phase 2 | now | verdict |
|---|---|---|---|
| `interventions` default none | MATCH on default, but resolved vec bound to `_interventions` and never wired | `.interventions(interventions)` (`crates/strands-harness/src/agent.rs:294`); `InterventionsOption::Off` is `#[default]` (`interventions.rs:36-39`); live `interventions_deny_the_shell_tool` | **MATCH — fixed since phase 2** |
| `mcpServers` default none | DIFF — non-empty → `Error::other("MCP not supported yet")` | `load_mcp_tools(options.mcp_servers.as_ref())` (`agent.rs:239`), `McpServersOption` path-or-inline | **MATCH — fixed since phase 2** |
| `await setupTelemetry()` on every create | DIFF — no equivalent call | `crate::telemetry::setup_telemetry()` (`agent.rs:295`), env-gated on `OTEL_TRACES_EXPORTER` (`telemetry.rs:60-71`), returns `None` when unset → no provider installed | **MATCH (env-gated) — fixed since phase 2** |
| sandbox default local | DIFF by design — "no sandbox concept in Rust" | `SandboxOption::Default` registers `LocalSandbox` as the process default when none is registered (`agent.rs:302-309`); `Off` → `no_sandbox()`; `Instance` → explicit | **MATCH — new since phase 2** |
| `DEFAULT_MODEL = 'bedrock/global.anthropic.claude-opus-5'` | MATCH | `crates/strands-harness/src/defaults.rs:4`, identical string | MATCH |
| builtin plugins `['todos','environment']` | MATCH | `defaults.rs:23` | MATCH |

Still open from phase 2 (unchanged, previously documented, not phase-3 regressions):
`programmatic_tool_caller` not implemented (dropped with a warning), and a `web_fetch` builtin
config does not reach the memory extractor (`web_fetch_model: None`).

## Verdict: **PASS-WITH-FINDINGS**

Phase-3 features are all present and the full suite is green (1241 pass / 0 fail) with clean
fmt and clippy. One blocking item and two non-blocking ones.

**Blocking**

1. `cargo build -p strands-agents` fails with default features *and* with `--no-default-features`
   — the auto-discovered `mcp_echo_server` bin needs
   `[[bin]] name = "mcp_echo_server" / required-features = ["mcp-echo-server"]` in
   `crates/strands-agents/Cargo.toml`. Two lines; must land before any PR. Add a
   non-`--all-features` build to the gate set so this cannot recur.

**Non-blocking, should fix**

2. 7 of 14 phase-3 reports are stubs with no traceability matrix (`middleware`, `interrupts`,
   `snapshots`, `telemetry`, `mcp`, `anthropic`, `agent-as-tool`; `delegation.md` missing). Those
   modules cannot be reviewed for port completeness — and their inventory gaps are the largest
   (MCP ~194, telemetry ~191, middleware ~108, sandbox ~72 TS tests with no Rust counterpart).
   The PR body must declare MCP tasks / telemetry tracer+meter / middleware copy-on-input /
   non-local sandbox backends as explicitly out of scope, or port them.
3. 2 intra-doc-link warnings (`agent/mod.rs:378`, `hooks/events.rs:148`).
