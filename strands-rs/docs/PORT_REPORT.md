# Port report: `strands-ts` → `strands-agents` (Rust)

Branch: `agent-tasks/strands-rs` on `strandly-the-agent/harness-sdk` (no PR by request).
Source: `strands-ts/src` @ `041a47f68`. Scope: option 1 — the core SDK subset that makes an
agent run; see [DESIGN.md](DESIGN.md) for in/out of scope and [FEATURE_MATRIX.md](FEATURE_MATRIX.md)
for per-feature availability against the Python and TypeScript SDKs.

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

## Comparison with `arronbailiss/harness-sdk` `feat/strands-rs`

A maintainer's earlier Rust vertical slice (Aug 2026, ~3k lines: types, `Model`, Bedrock,
`FunctionTool`, `#[tool]` proc macro, minimal owned `Agent`) was compared shape-by-shape
(`docs/reviews/compare-arron.md`, 24 rows). Adopted from it: the `#[tool]` attribute macro
(new crate `strands-agents-macros`, feature `macros`, default on; schema derived via `schemars`
rather than type-name matching), `prelude`, `BedrockModel::default_model()` /
`with_model_id()`, `AgentResult::text()`, `ModelEventStream` alias, provided `Model::aggregate`,
`Error::model_with_source`, clearer missing-model build error, "Adding a model provider" docs,
and the 20-line `weather_agent` example. Kept ours (with reasons in the file): `Clone` handle
`Agent` + `&self` (his slice has no hooks/plugins/`ToolContext.agent`, which force it),
`Tool::invoke -> Result<ToolResultBlock>` (TS parity; his `Result<Value>` cannot express
error-with-content, multi-block or media results), fallible async `build()`, `Error` naming, the
full `ContentBlock`/`StopReason`/error sets. Found in his slice: `Message.tracking_id` lacks
`#[serde(default)]`, so TS messages without `trackingId` fail to deserialize.

## Phase 2 — harness-ready SDK + `strands-harness`

Goal: `strands_harness::create_harness(Default::default())` reproduces `harness-ts` `createHarness()`
defaults. Plan: `PLAN_PHASE2.md`. Final state: 997 tests (844 SDK, 181 harness, 1 macro, doctests),
all gates green, live suites: Bedrock SDK 23/23, harness defaults 6/6 (`docs/reviews/*-run.log`).

**New SDK modules** (`strands-agents`): `storage/` (Storage trait, InMemoryStorage, LocalFileStorage
with TS key rules + atomic writes, keyword search), `context_manager/` ('auto'/'agentic' presets,
truncate/summarize/emergency strategies with verbatim markers and prompt, stash over Storage,
retrieval tool, overflow retry), `background_tasks/` (policy, `_background_execution` schema
injection, manage tool, synthetic tool-pair delivery via continuation input), `memory/`
(MemoryStore trait, MemoryManager + tools, ModelExtractor, FileMemoryStore, injection via the
`BeforeModelCallEvent.messages` seam), `plugins::vended::{AgentSkills, ContextInjector}`,
`tools::vended::ShellTool` (process-group kill on timeout/cancel), plus `Agent::{storage,
session_id, memory_manager}` and `AgentBuilder::{storage, context_manager, memory_manager,
background_tasks}`. `BeforeModelCallEvent.messages` replaces TS's `InvokeModelStage.Input` middleware.

**`strands-harness`**: `create_harness`/`HarnessAgentOptions` (TS algorithm step by step),
`defaults`, `prompt` (`HARNESS_CONTRACT` byte-identical — `tests/prompt_parity.rs` diffs it against
`harness-ts/src/prompt.ts`), `models::resolve_model` (bedrock effort/thinking/caching matrix; other
providers → clear unsupported error), `builtin_tools` resolution, tools `read/write/edit`,
`web_fetch`, `web_search` (Exa), `subagent` (option modes, schema incl. `context`/`last_messages`,
depth limit), plugins `todos`, `environment`, `resolve_memory`, interventions skeleton.

Default-behaviour parity (validator table in `docs/reviews/validation-phase2.md`): model id, effort,
caching, system prompt, context manager auto, session on (`./.agent/sessions`, 8-hex minted id,
`LocalFileStorage` as agent storage), skills on-if-dir, memory on (`./.agent/memory`, extractor),
builtin tools order, builtin plugins, background policy with forced `subagent`, interventions none —
all MATCH. Tool descriptions the model sees are byte-identical to TS for shell/read/write/edit/
web_fetch/todo_write/subagent. **Known DIFFs:** `programmatic_tool_caller` not ported (Monty Python
runtime; omitted with a warning), telemetry (`setupTelemetry`) not ported, MCP servers error out,
`shell.timeout` schema says `number` where zod says `integer`, web_fetch runs host-process `reqwest`
(http/https-only redirects) instead of sandboxed curl, shell runs `sh -c` on the host with the
inherited env (TS `LocalSandbox` equivalent).

Review disposition (phase 2): correctness 🔴 fixed — hook `cancel` now beats background routing,
context compaction reaches the triggering call, shell kills the whole process group with a bounded
drain; API 🔴 fixed — plain `CachingOption`/`BackgroundTasksOption`/`InterventionsOption` enums,
`Instance(..)` naming, `ResolveModelOptions`, `Agent::memory_manager()`, trimmed re-exports, prelude.
Open 🟡: background-task completion may overwrite `Cancelled`; background runs don't fire
`AfterToolCallEvent`; no matrices for harness-agent/models/plugins and context-manager (counts in
validation report; ~86 `agent.test.ts` rows unported); memory pass C items (key-aware extractor,
per-store triggers); `shell.timeout` integer type.

## Phase 3 — feature completion

All gates green at the final merge; **1210 tests passed, 0 failed, 39 ignored** (ignored = live
suites), across `strands-agents` (lib 880+), `strands-agents-macros`, `strands-harness` (lib 209).

```
$ cargo fmt --check
(clean)
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized] target(s) in 19.13s
$ cargo build -p strands-agents  # default features
    Finished `dev` profile [unoptimized] target(s) in 12.13s
$ cargo build --no-default-features -p strands-agents
    Finished `dev` profile [unoptimized] target(s) in 8.96s
$ cargo doc --no-deps --all-features
warnings: 0
```

Live runs (`docs/reviews/*-run.log`, genuine cargo output): Bedrock SDK e2e **22/22**
(`integ_bedrock`, the sliding-window test went with conversation managers), harness defaults e2e
**7/7** incl. the new `interventions_deny_the_shell_tool` (HITL with a scripted `ask`), MCP e2e
**2/2** against a real stdio server, sandbox tests run real `sh` processes, telemetry e2e via an
in-memory OTel exporter. Anthropic/OpenAI live tests skip without API keys.

**Landed (all with e2e where feasible):** conversation managers removed → `ContextManagerOption::Auto`
default; `middleware/` (TypeId-keyed stages, owned `Next<S>`, `wrap_fn(|ctx, sink, next| …)`,
input/wrap/output, loop + executor integration); interrupts + `interventions/` + `HumanInTheLoop`
(`Error::Interrupt`, `ToolContext::interrupt`, `InvokeInput::InterruptResponses`, pending-batch
resume; `FunctionTool` passes interrupts through); snapshots + checkpoints (`take_snapshot` /
`load_snapshot`, `checkpointing`, `StopReason::Checkpoint`, resume); `sandbox/` (`Sandbox` trait,
`LocalSandbox` process group + bounded drain, `ShellTool` routes through it, stdin raced with
timeout); `telemetry/` (`gen_ai.*` spans/events/metrics, `AgentResult.{metrics,traces}`, spans
ended on interrupt paths); `mcp/` over rmcp 3.5 (`McpClient`, `McpTool`, `load_servers`, stdio +
streamable-http; owned `McpServerInfo`, no rmcp types leaked); background-tasks engine
(`max_concurrency` semaphore, per-task `timeout`, pass-id dedup, `Cancelled` guard,
`AfterToolCallEvent` for background runs, `JoinHandle` abort); memory pass C (key-aware extractor,
`resolve_extraction_config`, read-only views, `TestMemoryStore`); `Continuations` API (mark
appended after the real append, abandon on early exit, same-role merge); `Agent::as_tool`
(interrupt namespacing/resume), `AgentDelegation`; Anthropic + OpenAI (Responses/Chat/bedrock-mantle)
providers over a byte-based `SseParser` (UTF-8 split + CRLF-across-chunks fixed) with injectable
transports. Harness: `interventions` → `HumanInTheLoop` (`ask` seam, collision text), `mcp_servers`
(`McpServersOption::{Path, Servers}` → `load_servers` with prefixing), `setup_telemetry` (env-gated
`OTEL_TRACES_EXPORTER` → `TelemetryConfig`), `sandbox: SandboxOption{Default,Off,Instance}` +
`drop_sandbox_tools`, `resolve_model` arms for `anthropic`/`openai`/`bedrock-mantle`.

**Review disposition (phase 3):**
- Correctness (`review-phase3-loop.md`): 🔴 UTF-8 chunk split corrupting SSE text (proven) → byte-based
  parser; 🟡 CRLF across chunks, un-ended tool span on interrupt, continuation `mark_appended`
  ordering, same-role merge, sandbox stdin deadlock — all fixed with regression tests.
- API bar-raiser (`review-phase3-api.md`): 🔴 1–5 fixed (owned `Next`, opaque `InterruptStateHandle`,
  infallible `intervention(impl …)`, no `rmcp`/`parking_lot`/`IndexMap` leaks, `opentelemetry`
  re-exported). **Rejected/deferred with reasons:** `#[non_exhaustive]` on `AnthropicModelConfig`/
  `OpenAIModelConfig` — forbids `..Default::default()` struct literals from other crates (would break
  the live tests and user code) until a builder exists; 🟡 6–10 (async input/output handlers,
  `AgentStreamStage`, per-stage event types, `pub(crate)`-ing provider internals, config builders)
  are tracked as follow-ups.
- Validator (`validation-phase3.md`): 🔴 plain `cargo build -p strands-agents` was broken (auto-
  discovered test bin without `required-features`) → `[[bin]]` declared; 2 rustdoc warnings fixed.
  PASS-WITH-FINDINGS otherwise; its unported-test counts (MCP tasks ~194, telemetry ~191,
  middleware ~108, sandbox ~72) are the honest coverage gap — matrices for those modules are the
  implementers' reports in `docs/matrices/`, not row-by-row inventories.

**Not ported (by decision):** MCP tasks/annotations/progress, docker/ssh sandboxes, `AgentStreamStage`,
HITL classifier/Cedar policies (hard errors naming the gap), OTLP exporter installation (the host
installs it), printer, A2A, multiagent, Gemini/LiteLLM/OpenRouter.

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

## API review follow-ups (2026-10-14)

Landed from the Rust-idiom/DevX review (branches `agent-tasks/strands-rs-fix-{builder,macro,hooks,stream,devx}`):
`AgentBuilder::build()`/`Agent::new()` are sync (init is lazy; `create_harness` initializes eagerly); no
`*_arc`/`*_ref` builder twins; `#[tool]` propagates `Interrupt`/`Cancelled`, accepts `#[tool(name, description)]`
and per-parameter `///` docs; hook callbacks take plain async closures (no `Box::pin`), registration returns `()`,
`AgentBuilder::hook`/`sync_hook`; `AgentStreamEvent::{text_delta,tool_use,tool_result}` + `Agent::stream_text`;
`invoke_structured` → `structured_output`, `AgentResult::structured_output_as`; `ModelStopData.usage`;
`testing` module (alias `test_support`), crate-level guide, actionable no-credentials error, default features
`bedrock + macros`. Gates: fmt, clippy `-D warnings`, 1224 tests, `--no-default-features`, examples — all green.

## Phase 4

- **Vended tool `mcp_router`** (`tools::vended`, branch `agent-tasks/strands-rs-vended-tools`):
  `McpRouterTool`/`McpRouterConfig` port the runtime MCP router behind the existing `mcp` feature —
  `connect`/`list_connections`/`list_tools`/`call_tool`/`disconnect` over an allowlist of
  `McpServerConfig`s, with TS's name, description and schema verbatim and its connection cap and
  duplicate-id rules. Connections are held per tool instance (no `WeakMap`/finalizer in Rust).
  13 unit tests cover validation and dispatch; the live-server cases still need a
  `tests/mcp_router_e2e.rs` against `mcp_echo_server`.
- **Vended tools `fileEditor` + `http_request`** (`tools::vended`, branch
  `agent-tasks/strands-rs-vended-tools`): `FileEditorTool` ports `view`/`create`/`str_replace`/
  `insert` with TS's exact error strings and snippet format, routing every file access through a
  `Sandbox` (upstream has no `undo_edit`, so neither does the port); `HttpRequestTool` ports the
  `http_request` tool behind a new `http` cargo feature, with an injectable `reqwest::Client`, a
  `tokio::select!` timeout and TS's name/description/schema verbatim. 70 unit tests
  (53 + 17), the HTTP ones against a loopback `TcpListener` server.
- **Vended tools `sleep` + `handoff_to_user`** (`tools::vended`, branch
  `agent-tasks/strands-rs-vended-tools`): `SleepTool`/`SleepConfig` pause through an injected
  `retry::Sleeper` (no real sleeps in tests) and race `ToolContext::cancel`;
  `HandoffToUserTool`/`HandoffToUserConfig` raise a `strands:handoff-to-user` interrupt via
  `ToolContext::interrupt` and return the human's reply as the tool result. Name, description and
  input-schema text match TS verbatim. 25 unit + 4 agent-loop tests; see
  `docs/matrices/vended-tools.md`. `file_editor`, `http_request` and `mcp_router` landed on the
  same branch (see the matrix); nothing is left in this module.
