# Port report: `strands-ts` → `strands-agents` (Rust)

Branch: `agent-tasks/strands-rs` on `strandly-the-agent/harness-sdk` (no PR by request).
Source: `strands-ts/src` @ `041a47f68`. Scope: option 1 — the core SDK subset that makes an
agent run; see [DESIGN.md](DESIGN.md) for in/out of scope.

## Status

All gates green at the final merge commit (601 tests: 543 unit + 57 integration + 1 doctest;
~22.4k lines of Rust incl. tests). Independent validation verdict: PASS-WITH-FINDINGS; all 🔴 and
🟡 correctness findings and all 🔴 API findings were fixed and re-verified (see below).

```
$ cargo fmt --check
(clean)
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized] target(s) in 0.13s
$ cargo test --all-features
     Running unittests src/lib.rs (/home/sandbox/cargo-target/debug/deps/strands_agents-c418e986ebb3ece5)
test result: ok. 543 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
     Running tests/agent_loop.rs (/home/sandbox/cargo-target/debug/deps/agent_loop-85ca00193662de09)
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
     Running tests/agent_loop_extra.rs (/home/sandbox/cargo-target/debug/deps/agent_loop_extra-988a39e583b52103)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.58s
$ cargo build --no-default-features
    Finished `dev` profile [unoptimized] target(s) in 0.09s
$ cargo build --examples --all-features
    Finished `dev` profile [unoptimized] target(s) in 0.11s
$ cargo doc --no-deps --all-features
   Generated /home/sandbox/cargo-target/doc/strands_agents/index.html
```


## Structural map

| TS (`strands-ts/src`) | Rust (`crates/strands-agents/src`) | Notes |
|---|---|---|
| `types/messages.ts`, `media.ts`, `citations.ts`, `json.ts` | `types/{messages,media,citations,stop_reason,system_prompt,tool}.rs` | serde enums, externally tagged |
| `tools/types.ts` | `types/tool.rs` | `ToolSpec`, `ToolUse`, `ToolChoice` |
| `errors.ts` | `error.rs` | one `Error` enum; `Error::name()` = TS class name |
| `models/model.ts`, `streaming.ts` | `models/{model,streaming}.rs` | `Model` trait, `stream_aggregated`, token heuristic |
| `models/defaults.ts` | `models/defaults.rs` | context-window table + region-prefix strip |
| `models/bedrock.ts` | `models/bedrock/{mod,format,parse}.rs` | feature `bedrock` (default) |
| `tools/tool.ts` | `tools/tool.rs` | `Tool` trait, `ToolContext`, `create_error_result` |
| `tools/function-tool.ts`, `zod-tool.ts`, `tool-factory.ts` | `tools/function_tool.rs` | builder + `typed_handler` (schemars/jsonschema) |
| `tools/noop-tool.ts`, `structured-output-tool.ts` | `tools/{noop_tool,structured_output}.rs` | |
| `registry/tool-registry.ts` | `tools/registry.rs` | `IndexMap`, fuzzy `resolve` |
| `tools/executors/*` | `tools/executor.rs` | sequential + concurrent |
| `hooks/*` | `hooks/{registry,events}.rs` | `TypeId`-keyed registry, 16 event structs |
| `plugins/*` | `plugins/{mod,registry,model_plugin}.rs` | |
| `retry/*` | `retry/*.rs` | injectable sleeper |
| `logging/*` | `logging.rs` | `tracing` + `warn_once` |
| `state-store.ts` | `state_store.rs` | |
| `conversation-manager/{conversation-manager,null-*,sliding-window-*}.ts` | `conversation_manager/{mod,null_manager,sliding_window}.rs` | UTF-16 slicing for truncation |
| `session/{types,storage,file-storage,validation,session-manager}.ts` | `session/{types,storage,file_storage,validation,session_manager,agent_snapshot}.rs` | snapshot design, wire-compatible files |
| `agent/agent.ts`, `tool-caller.ts`, `continuation.ts` (resume), `types/agent.ts` | `agent/{mod,builder,event_loop,tool_caller,result,invocation_state}.rs` | spawned task + mpsc stream |
| `__fixtures__/mock-message-model.ts`, `model-test-helpers.ts` | `test_support.rs` (feature `test-support`) | |

Not ported (seams kept): interrupts/HITL, checkpoints/snapshots of agent state beyond sessions,
background tasks, middleware, agent-as-tool/delegation, MCP, A2A, multiagent, sandbox, printer,
telemetry, memory manager, summarizing conversation manager, S3 storage, non-Bedrock providers,
`AWS_BEARER_TOKEN_BEDROCK` auth.

## Traceability

Per-module matrices (TS `it(...)` → Rust test → status) are in `docs/matrices/`. Summary:

| Module | TS tests in scope | Ported | Skipped (reason class) |
|---|---|---|---|
| function-tool / zod-tool / factory / noop / structured-output | 78 | 72 | 6 — not portable (JS aliasing, stack traces, type guards) |
| session (file-storage, session-manager, validation) | 79 | 59 + 32 extra | 20 — stash/multi-agent/context-manager not ported |
| bedrock + defaults | 237 | ~130 | ~105 — need live AWS / mocked SdkError / log capture / clientConfig surface |
| conversation-manager (null, sliding window, base) | 87 | 86 | 1 — needs loop (now covered by `tests/agent_loop.rs` overflow test) |
| executors | 3 groups | 3 (18 tests) | interrupt / telemetry / middleware groups |
| agent loop (`agent.test.ts` core, tool-caller, `AgentResult`) | ~120 in scope | ~75 (53 integration + 21 tool caller) | Proxy accessor, `Tool.stream`, out-of-scope features |
| retry, plugins, hooks, types, models/model.rs | — | matrices built by the validator: retry 33/34, plugins 17/21; types/model/hooks covered by grouped tests | 5 retry names to confirm (see validation.md) |

Independent validation verdict: **PASS-WITH-FINDINGS** (wire-compat round-trip of TS fixtures:
3/3 byte-identical; sensitive-surface diff: nothing Rust does that TS doesn't).

## Decision log (deviations from TS and why)

1. **`Agent` is a `Clone` handle** (`Arc<AgentInner>` + `parking_lot` locks); `invoke`/`stream`
   take `&self`. TS mutates a single object from hooks mid-loop; Rust can't hand `&mut Agent` to
   `'static` callbacks. No lock guard crosses an `.await` (reviewed).
2. **Streams end with the result.** `stream()` yields `AgentStreamEvent`s and the last item is
   `AgentResult(..)`; `invoke()` drains it. Mirrors TS where `AgentResultEvent` is the last yield.
   The loop runs in a spawned task feeding an `mpsc` channel; cleanup events are emitted
   explicitly. Dropping the stream cancels the invocation (TS stops at the abandoned `yield`).
3. **`ConcurrentInvocation` arrives as the stream's first `Err`** (and `Err` from `invoke`),
   not a call-time throw, because `stream()` returns the stream, not `Result<_>`.
4. **No middleware/interrupt layers** (`_streamWithMiddleware`/`_streamCore`): hook dispatch is
   inline. Hook errors abort the invocation; TS's cleanup-phase warn-and-swallow is not reproduced.
5. **Tools return `Result<ToolResultBlock>`**; progress via `ctx.progress(v).await`. `Err` is
   reserved for `Cancelled`; tool failures are `Ok(status: error)` exactly as TS.
   `Tool '<name>' did not return a result` is unrepresentable.
6. **Typed tools use `schemars` + `jsonschema`** instead of zod; validation messages are
   `jsonschema`'s (same path/message shape, different wording). `FunctionTool` (untyped) does no
   validation, as in TS.
7. **Direct tool calls** (`agent.tool(name).call(..)`) follow TS `tool-caller.ts`: no executor,
   no Before/AfterToolCall hooks, three recorded messages when `record_direct_tool_call`.
8. **Concurrent executor** uses `FuturesUnordered` (one in-flight step per tool). A tool's
   `Err(Cancelled)` becomes an `EXECUTION_CANCELLED_MESSAGE` error result (matches Sequential/TS).
9. **Session module is snapshot-based** (`FileStorage`, `SnapshotStorage`, `SessionManager`)
   because that is what `strands-ts/src/session` actually is; files are wire-compatible.
10. **Sliding-window truncation slices on UTF-16 code units** to match JS `slice`/`.length`.
11. **Bedrock**: no `clientConfig`/requestHandler surface (region/timeout + `from_client`);
    `context_window_limit` resolves lazily; input/output redaction are two events; reasoning
    deltas are one-of in the Rust SDK so a combined TS delta becomes two; `additionalArgs`
    unsupported (`warn_once`).
12. **Plugin init order** follows TS `agent.ts` (survey §8.2): conversation manager, retry
    strategy, user plugins, session manager, `ModelPlugin`.
13. **`Error::MaxTokens` boxes the partial message** (clippy `result_large_err`).
14. **Logging is `tracing`**; log-assertion tests are dropped (no capture layer).

## Review findings (independent passes)

Full reports in `docs/reviews/`. Summary and disposition:

**Correctness reviewer** (`review-loop.md`) — all fixed with regression tests:
- 🔴 F1 `invoking` flag leaked when the spawned task panicked → `InvokingGuard` drop guard +
  `JoinError` forwarded (`a_panicking_hook_releases_the_invocation_lock`).
- 🟡 F2 dropping `AgentStream` left the invocation running → `Drop` cancels the root token; a
  closed channel surfaces as `Error::Cancelled` (`dropping_the_stream_cancels_the_invocation`).
- 🟡 F3 `cancel()` raced the previous invocation's token → token installed synchronously in
  `stream()` (`cancel_right_after_stream_cancels_the_new_invocation`).
- 🟡 F4 concurrent executor diverged from sequential/TS on a tool `Cancelled` → per-tool error
  result, batch returns `Ok` (`concurrent_converts_a_tool_cancellation_into_an_error_result`).
- 🟡 F5 `stream_aggregated` re-wrapped `Cancelled` as a retryable `Model` error → passthrough.
- 🟢 F6/F7 `# Panics` doc on `stream()`; `select!` on cancellation in `stream_from_model`.

**API bar-raiser** (`review-api.md`) — fixed: typed builder slots
(`conversation_manager(impl ConversationManager)`, `retry_strategy(impl ModelRetryStrategy)`,
`session_manager(..)`), `impl Model/Tool/Plugin + 'static` parameters with `*_arc`/`tool_ref`
variants (plus a blanket `impl Model for Arc<M>`), `#[non_exhaustive]` + `Default`/`with_*` on
option/result structs and growable enums, `stream(input)` / `stream_with(input, options)`,
`#[must_use]` on builders/streams/`HookCleanup`, single `Redaction` type, `JsonSchema` alias
removed from public signatures, Bedrock config types re-exported, `AgentResult: Serialize`,
docs re-aligned. **Open (🟡/🟢, deferred):** `ToolExecutor::execute` out-param signature (#7),
`FunctionToolBuilder` terminal `handler` vs `build()` (#8), `SessionManager::new(&str)` /
`save_snapshot(bool)` (#13), `BedrockModelConfig.max_tokens: i32` vs `u64` (#14), nits #17–22.

**Port validator** (`validation.md`) — PASS-WITH-FINDINGS. All 462 matrix "pass" rows resolve to
real tests; 3 sampled tests assert slightly less than TS (low); 3 rustdoc warnings (fixed);
wire-compat 3/3; sensitive surface identical to TS. Open: written matrix for `hooks`; confirm the
retry "no backoff timer when already cancelled" case.


## Live Bedrock integration run

`tests/integ_bedrock.rs` ports `strands-ts/test/integ/models/bedrock.test.ts` and the Bedrock rows
of `test/integ/agent.test.ts`. Run from the sandbox against us-west-2 (`global.anthropic.claude-sonnet-4-6`,
caching on `us.anthropic.claude-sonnet-4-5-20250929-v1:0`, overflow on `us.anthropic.claude-haiku-4-5-20251001-v1:0`):

```
$ AWS_REGION=us-west-2 cargo test --all-features --test integ_bedrock -- --ignored --test-threads=4
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 16.06s
```

20 tests ran against the service: maxTokens stop reason + usage cap; system-prompt, message and
`cache_config` (auto + anthropic) prompt caching (asserting real `cacheWriteInputTokens` then
`cacheReadInputTokens`); invalid model id → model error; sliding-window history management over
three turns; `Error::MaxTokens` through `NullConversationManager`; oversized prompt →
`Error::ContextWindowOverflow` (real `ValidationException` substring mapping); thinking +
tool use (reasoning block, tool use and tool result recorded); native `count_tokens`; agent
streaming with tool use and multi-turn; JSON-schema tool; metadata (usage + latency) through the
agent stream; multi-turn memory; `ContentBlock`/`Message` inputs; structured output; reasoning
deltas; the non-streaming `Converse` path; `stream_aggregated` with usage metadata.

The 3 guardrail tests (input intervention with trace disabled/enabled/enabled_full) are written
but **skipped**: a tagged `strandly-managed-*` guardrail was created, but the sandbox role is not
granted `bedrock:ApplyGuardrail`, so `Converse` with a `guardrailConfig` is denied. They run when
`STRANDS_TEST_GUARDRAIL_ID` points at a guardrail the caller may apply. Guardrail was deleted.

Not exercised live: throttling (`ModelThrottled`) — cannot be provoked deterministically;
guardrail output intervention / masking / tool-result redaction / `guard_latest_user_message`
(same permission gap); media (audio/PDF/image) — no fixtures ported yet; `AWS_BEARER_TOKEN_BEDROCK`.

## Known gaps / follow-ups

- Bedrock: throttling mapping and guardrail paths untested live (see above); bearer-token auth
  untested; `AWS_BEARER_TOKEN_BEDROCK` not supported; media integration tests not ported.
- Session: stash / multi-agent snapshot parts not ported.
- Retry: confirm "no backoff timer when already cancelled" case (validator flag).
- Hooks: events are compile-checked and exercised by the loop tests, but have no written matrix.
- `projected_input_tokens` uses the heuristic estimator only (no provider `count_tokens`).
- Doc tests: 2 ignored (`add_callback` example needs a runtime).

## How this was built

Foundation (types, errors, `Model`/`Tool`/hook traits, `Agent` handle) was written first on the
integration branch; six modules were then ported in parallel on per-module branches
(`agent-tasks/strands-rs-<module>`, one git worktree each) and merged; the agent loop followed in
three passes; test-gap passes closed sliding-window and Bedrock cache-point coverage; three
independent review passes (correctness reviewer, API bar-raiser, port validator) produced the
findings above, which were fixed on `-fix-loop`/`-fix-api` branches and merged.
