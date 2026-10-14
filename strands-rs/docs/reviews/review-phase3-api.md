# Phase 3 API bar-raise — strands-rs (`agent-tasks/strands-rs` @ 5276b6593)

Read-only review, no cargo. Severity: 🔴 painful/unsound/misleading/parity-breaking · 🟡 fix before 0.1 · 🟢 nit.
Paths relative to `strands-rs/crates/strands-agents/src/` unless noted.

## 1. `middleware/` — vs TS `middleware/{types,stages,registry}.ts`

Surface: `Stage` (mod.rs:23), `Middleware<S>`/`Terminal<S>`/`Next<'a,S>`/`WrapFn`/`wrap_fn`/`PhaseKind`/`MiddlewareHandle`/`MiddlewareRegistry` (registry.rs:17-222), `EventSink<E>`/`EventTarget<E>` (sink.rs:12-70), `InvokeModelStage`/`InvokeModelContext`, `ExecuteToolStage`/`ExecuteToolContext` (stages.rs:17-100), `Agent::{middleware,add_middleware,add_wrap_middleware,add_input_middleware,add_output_middleware,remove_middleware}` (agent/mod.rs:222-253).

Verdict on the sink shape: **sound and filter/inject-capable** — a wrap handler gets `&EventSink<S::Event>` plus `Next`, so it can drop events (`sink.filter(..)`), rewrite them (`sink.map(..)`), inject (`sink.send(..)`), short-circuit (skip `next.run`) and retry (`Next: Clone`, `run(&self)`). That is the right Rust translation of the TS async-generator handler. Problems are at the edges:

- 🔴 **`wrap_fn` is unusable without a `BoxFuture` cast and three type annotations** — registry.rs:123-131 `F: for<'a> Fn(S::Context, EventSink<S::Event>, Next<'a, S>) -> BoxFuture<'a, Result<S::Result>>`. The crate's own test has to write `|ctx: String, sink: EventSink<String>, next: Next<'_, TestStage>| { .. Box::pin(async move {..}) as BoxFuture<'_, Result<String>> }` (middleware/tests.rs:62-71). The borrowed `Next<'a>` is what forces the HRTB + cast. Fix: make `Next<S>` owned/`'static` (hold `Arc<[HandlerRef<S>]>` + `Arc<dyn Terminal<S>>`), then `pub fn wrap_fn<S: Stage, F, Fut>(f: F) -> WrapFn<F> where F: Fn(S::Context, EventSink<S::Event>, Next<S>) -> Fut + Send + Sync + 'static, Fut: Future<Output = Result<S::Result>> + Send + 'static` so `agent.add_middleware::<InvokeModelStage>(wrap_fn(|ctx, sink, next| async move { next.run(ctx, &sink).await }))` compiles. `futures::future::BoxFuture` also leaks into a pub bound without re-export.
- 🟡 **Input/output handlers are sync-only** — registry.rs:174-186 `impl Fn(S::Context) -> Result<S::Context>`; TS `MiddlewareInputHandler = (context) => TContext | Promise<TContext>` (types.ts:75,78). Any input middleware that needs I/O (retrieval, token counting via `Model::count_tokens`) must be written as a wrap handler. Replacement: `add_input<S, F, Fut>(&self, f: F) where F: Fn(S::Context) -> Fut, Fut: Future<Output = Result<S::Context>> + Send`.
- 🟡 **Parity: `AgentStreamStage` missing** — TS stages.ts:184 exports a third built-in stage; nothing in `middleware/` or docs mentions it (`rg AgentStreamStage` = 0). Either port it or record the omission in DESIGN.md.
- 🟡 **Parity: per-stage event types vs TS `AgentStreamEvent`** — stages.rs:19-21/56-58 use `ModelStreamEvent` / `ToolStreamEvent`; TS uses `AgentStreamEvent` for all stages so a middleware can inject agent-level/custom events into `agent.stream()`. Rust middleware can only inject model/tool-shaped events. Deliberate (stronger typing) is fine, but say so in DESIGN.md and consider `type Event = AgentStreamEvent` for parity.
- 🟡 **`EventSink::collector() -> (Self, Arc<Mutex<Vec<E>>>)`** sink.rs:46 leaks `parking_lot::Mutex` (sink.rs:3) in a pub signature with no re-export. Return a small `EventBuffer<E>` newtype with `fn drain(&self) -> Vec<E>` / `fn snapshot(&self) -> Vec<E>`.
- 🟡 **Three names for one thing** — `Agent::add_middleware` and `Agent::add_wrap_middleware` (agent/mod.rs:227,232) are identical; `MiddlewareRegistry::add` is the third. Keep `add_middleware` (TS bare-stage overload) + `add_input_middleware`/`add_output_middleware`; drop `add_wrap_middleware`.
- 🟡 **`ExecuteToolContext` has no `interrupt()`** — TS `ExecuteToolContext extends MiddlewareInterruptible` (stages.ts:116); stages.rs:105 comment admits the seam is missing while `interrupt.rs` already exists in this phase. A middleware-level HITL gate (the headline TS use case) cannot be written yet. Add `pub fn interrupt(&self, params: InterruptParams) -> Result<InterruptResponseContent>` (or the `Error::Interrupt` constructor helper) now that the module exists.
- 🟢 `Terminal<S>` has a blanket impl for `Fn(S::Context, &EventSink<S::Event>) -> Fut` (registry.rs:57) but `Middleware<S>` has none — needs `wrap_fn`; consider the symmetric blanket impl so `wrap_fn` becomes optional.
- 🟢 `MiddlewareRegistry::invoke` takes `terminal: &dyn Terminal<S>` while everything else is `impl Trait`. `#[must_use]` missing on `MiddlewareHandle`-returning `add_*` (a dropped handle = unremovable middleware) — phase-1 convention (#9) was `#[must_use]` on handle/builder returns.
- 🟢 `Stage::name()` carries `where Self: Sized` on a trait that is already `'static` and never used as `dyn` — drop the bound.

## 2. `interrupt.rs` + `types/interrupt.rs` + `interventions/`

Surface: `InterruptSource`, `Interrupt`, `PendingToolExecution`, `InterruptState`, `InterruptStateHandle`, `InterruptFn`, `tool_interrupt_id/before_tool_call_interrupt_id/before_tools_interrupt_id` (interrupt.rs:22-277); `InterruptParams`, `InterruptResponse`, `InterruptResponseContent` (types/interrupt.rs:11-88); `Error::Interrupt(Vec<Interrupt>)` (error.rs:101); `ToolContext::{interrupt,with_interrupt}` (tools/tool.rs:68-77); `InvokeInput::InterruptResponses` (agent/mod.rs:60); `Agent::{interrupt_state,unanswered_interrupts}` (agent/mod.rs:317-322); `InterventionHandler`, `Action<E>`, `ToolCallAction`, `Confirm`, `InterventionRegistry`, `EvaluateFn`, `Transform<E>`, `DENIED_PREFIX`… (interventions/mod.rs:24-156); `HumanInTheLoop`, `AskFn`, `TRUSTED_TOOLS_KEY`, `TRUST_RESPONSES` (vended/hitl.rs:18-83); `AgentBuilder::interventions` (builder.rs:148).

Good: `ToolContext::interrupt::<T>(InterruptParams) -> Result<T>` with the "propagate with `?`" doc is the right shape; `Interrupt`/`InterruptParams`/`InterruptResponse` are `#[non_exhaustive]` + `new`/`with_*` per convention; `Error::Interrupt(Vec<Interrupt>)` mirrors TS `InterruptError.interrupts`.

- 🔴 **`parking_lot::Mutex` leaks through `pub type InterruptStateHandle = Arc<Mutex<InterruptState>>`** (interrupt.rs:13,272) and is returned by the public `Agent::interrupt_state()` (agent/mod.rs:317). Users must add `parking_lot` to call `.lock()`, and the alias freezes the locking strategy into the API. Replace with an opaque `pub struct InterruptStateHandle(Arc<Mutex<InterruptState>>)` exposing `fn with<R>(&self, f: impl FnOnce(&mut InterruptState) -> R) -> R` / `fn snapshot(&self) -> InterruptState`, or keep `Agent::unanswered_interrupts()` + `Agent::interrupts()` as the only public read path and make `interrupt_state()` `pub(crate)`.
- 🔴 **`AgentBuilder::interventions(..) -> Result<Self>`** (builder.rs:148-152) is the only fallible builder method; it breaks `AgentBuilder::new().model(m).interventions(..)?.tool(t)` chains and forces `Arc<dyn InterventionHandler>` at every call site, against DESIGN.md "builders take `impl Trait + 'static` and wrap in `Arc` internally". Replacement: `pub fn intervention(self, handler: impl InterventionHandler + 'static) -> Self` (repeatable, like `tool`/`plugin`) and move the duplicate-name check into `build()` (which already returns `Result`).
- 🟡 **Parity: `InterventionHandler` has 3 of TS's 5 gates** — interventions/mod.rs:131-144 `before_invocation/before_tool_call/after_tool_call`; TS handler.ts:60-64 also has `beforeModelCall`/`afterModelCall`, and TS has an `onError: 'throw' | 'proceed' | 'deny'` policy (handler.ts:19; `rg on_error interventions/` = 0). A handler that gates model calls (prompt-injection scan, token budget) cannot be ported. Add the two gates with default `Proceed` + `fn on_error(&self) -> OnError`.
- 🟡 **`InvokeInput::InterruptResponses(Vec<InterruptResponseContent>)`** (agent/mod.rs:60) carries the *wire wrapper* (`{ interruptResponse: {..} }`) in a programmatic API; `InterruptResponseContent` exists only to serialise as a content block. The variant should hold `Vec<InterruptResponse>` (the `From<InterruptResponse>`/`From<Vec<_>>` impls at mod.rs:93-99 already hide the wrapper, which shows it isn't wanted). Keep `InterruptResponseContent` as a `ContentBlock` detail.
- 🟡 **`ToolContext::with_interrupt(InterruptFn)` where `InterruptFn = Arc<dyn Fn(InterruptParams) -> Result<JsonValue> + Send + Sync>`** (tool.rs:68, interrupt.rs:277) — same `Arc`-at-call-site smell; take `impl Fn(InterruptParams) -> Result<JsonValue> + Send + Sync + 'static`. Same for `HumanInTheLoop::ask(AskFn)` (hitl.rs:23,69) which additionally leaks `futures::BoxFuture` — accept `F: Fn(String) -> Fut + Send + Sync + 'static, Fut: Future<Output = Result<JsonValue>> + Send + 'static`.
- 🟡 **`InterruptState` is fully `pub` with mutators (`activate`, `register`, `get_or_create`, `set_pending_tool_execution`, `resume`)** (interrupt.rs:119-230) and reachable via `Agent::interrupt_state()`; users can desync the loop (e.g. `deactivate()` while a pending tool batch is replayed). TS keeps `InterruptState` internal and exposes only `agent.interrupts`/`InterruptError`. Make the mutators `pub(crate)`, keep `list()`/`unanswered()`/`pending_tool_execution()`.
- 🟡 **Prefix/response-string constants in the public API** — `DENIED_PREFIX`, `GUIDANCE_PREFIX`, `CONFIRMATION_FAILED_PREFIX` (interventions/mod.rs:24-28), `TRUST_RESPONSES`, `TRUSTED_TOOLS_KEY` (hitl.rs:18-20). Phase-1 #11 flagged exactly this pattern (`CONCURRENT_INVOCATION_MESSAGE`); make them `pub(crate)` or `#[doc(hidden)]`.
- 🟢 `tool_interrupt_id`/`before_tool_call_interrupt_id`/`before_tools_interrupt_id` as free `pub fn`s at the module root (interrupt.rs:81-91) — group under `impl Interrupt { pub fn tool_id(..) }` or a `interrupt::ids` module; `HumanInTheLoop::enable_trust(bool)` (hitl.rs:62) — `bool` setter; `trust()`/`no_trust()` or `with_trust(true)` — minor. (`HumanInTheLoop` setters already carry `#[must_use]` — good.)

## 3. `agent/{snapshot,checkpoint}.rs`

Surface: `SnapshotField`, `ALL_SNAPSHOT_FIELDS`, `SnapshotPreset`, `TakeSnapshotOptions::{preset,include,exclude,app_data,fields}`, `Agent::{take_snapshot,load_snapshot}` (snapshot.rs:12-149); `CHECKPOINT_SCHEMA_VERSION`, `CheckpointPosition`, `CheckpointData`, `Checkpoint::{new,schema_version,to_json,from_json}`, `CheckpointResumeContent` (checkpoint.rs:14-88); `InvokeInput::CheckpointResume(CheckpointResumeContent)` (agent/mod.rs:62); `AgentResult.checkpoint` (result.rs:37); `AgentBuilder::checkpointing(bool)` (builder.rs:263).

- 🟡 **Wire wrapper in the programmatic input, again** — `InvokeInput::CheckpointResume(CheckpointResumeContent)` (agent/mod.rs:62) mirrors the `{ checkpoint: {..} }` content block; no `From<Checkpoint> for InvokeInput` exists (`rg 'From<Checkpoint' agent/mod.rs` = 0), so resuming is `InvokeInput::CheckpointResume(CheckpointResumeContent { checkpoint: result.checkpoint.unwrap() })`. Make the variant `CheckpointResume(Checkpoint)` and add `From<Checkpoint>`; keep `CheckpointResumeContent` for the message wire form.
- 🟡 **`TakeSnapshotOptions` setters break the phase-1 `with_*` convention** — snapshot.rs:64-85 `preset()/include()/exclude()/app_data()` vs DESIGN.md "built from `Default` plus `with_*` setters" (`InvokeOptions::with_limits`, `ExecuteOptions::with_timeout`). Rename to `with_preset/with_field/without_field/with_app_data` (they already have `#[must_use]`); and give `Agent::take_snapshot()` an options-less form (`take_snapshot()` + `take_snapshot_with(opts)`) to match `stream`/`stream_with` (phase-1 #4).
- 🟡 **`CheckpointData` is a public wire struct with `extra: Map<String, Value>`** (checkpoint.rs:31-42) exported from `agent` (mod.rs:32); it exists only to implement `Serialize/Deserialize for Checkpoint`. Make it private; `Checkpoint::to_json/from_json` (checkpoint.rs:77-82) duplicate `serde_json::to_value/from_value` on a type that already implements serde (phase-1 #6 precedent) — drop or `#[doc(hidden)]`.
- 🟢 `Checkpoint` and `CheckpointPosition` (wire enum) lack `#[non_exhaustive]` while `Interrupt`/`InterruptParams` have it; `ALL_SNAPSHOT_FIELDS: [SnapshotField; 5]` fixes the array length into the API — prefer `SnapshotField::ALL: &[SnapshotField]` or `fn all()`.

## 4. `sandbox/`

Surface: `Sandbox` trait (mod.rs:178-241), `LocalSandbox::{new,with_base_dir,with_env}` (local.rs:27-47), `ExecuteOptions` (mod.rs:90-105), `ExecutionResult`, `StreamChunk`, `StreamType`, `FileInfo`, `OutputFile`, `SandboxEvent`, `SandboxError`, `SandboxResult`, `SandboxRef`, `is_valid_language/is_valid_env_key/shell_quote`, `NO_SANDBOX_MESSAGE`, `set_default_sandbox/clear_default_sandbox/default_sandbox/register_local_sandbox_default` (mod.rs:146-281), `AgentBuilder::{sandbox,sandbox_arc,no_sandbox}` (builder.rs:183-196), `Agent::sandbox() -> Result<SandboxRef>` (mod.rs:376).

Good: `sandbox(impl Sandbox + 'static)` + `sandbox_arc` follows the phase-1 convention exactly; `SandboxError` is `#[non_exhaustive]` and converts into `Error`.

- 🟡 **Process-global mutable default** — `set_default_sandbox(SandboxRef)` / `clear_default_sandbox()` / `register_local_sandbox_default()` (mod.rs:259-281) are hidden global state that `Agent::sandbox()` falls back to; two agents in one process cannot have different implicit defaults and tests race. TS has the same `setDefaultSandbox`, so parity-wise OK, but at minimum: document the fallback order on `Agent::sandbox()`, and make `register_local_sandbox_default()` the only `pub` writer or fold it into `set_default_sandbox(LocalSandbox::new())`.
- 🟡 **`ExecuteOptions` is all-`pub` without `#[non_exhaustive]`** (mod.rs:89-100) while `ExecutionResult` is `Default`+all-pub too (mod.rs:67-76); phase-1 #3 made every option struct `#[non_exhaustive]`. `pub cancel: Option<tokio_util::sync::CancellationToken>` is written with a full path instead of the crate's imported `CancellationToken` (cf. `ToolContext::cancel`). Add `#[non_exhaustive]` + `with_cwd/with_env/with_stdin/with_cancel` next to the existing `with_timeout`.
- 🟢 `Sandbox::tools() -> Vec<ToolRef>` (mod.rs:241) matches `Plugin::tools()` in code (plugins/mod.rs:26) but not DESIGN.md, which still says `Vec<Arc<dyn Tool>>` — doc drift only.
- 🟢 `is_valid_language`, `is_valid_env_key`, `shell_quote` (mod.rs:149-167) are helpers for sandbox *implementors*; `pub` is fine but put them under `sandbox::util` or document "for implementors". `NO_SANDBOX_MESSAGE` is another assertion string (phase-1 #11 pattern).

## 5. `telemetry/`

Surface: `TelemetryConfig::{new,from_env,with_trace_attributes}`, `AttrValue`, `DEFAULT_SERVICE_NAME`, `service_name()`, `CycleScope::{start,span,cycle_id}` (mod.rs:18-180); `Tracer::{new,config,local_traces,start_*_span,end_*_span}`, `SpanHandle::{context,start_time}`, `AgentSpanOptions<'a>`, `ModelSpanOptions<'a>` (tracer.rs:19-267); `Meter`, `CycleHandle` (meter.rs); `AgentMetrics`, `AgentTrace`, `CycleMetrics`, `InvocationMetrics`, `ToolMetrics` (types.rs); `to_telemetry_json` (json.rs); `AgentResult.{metrics,traces}`; `AgentBuilder::{telemetry,trace_attributes}`.

- 🔴 **`opentelemetry::Context` leaks through `SpanHandle::context() -> Option<&opentelemetry::Context>`** (tracer.rs:28) with no re-export; the `opentelemetry` crate is not re-exported from `lib.rs` either, so the user must pin the *same* `opentelemetry` version to use it — the classic OTEL SemVer trap. Either `pub use opentelemetry;` at the crate root (documented as a public dep) or hide the method behind a feature and return an opaque handle.
- 🟡 **`Tracer`'s span lifecycle is a public, manually paired API** — `start_agent_span/end_agent_span`, `start_model_invoke_span/end_model_invoke_span(…)`, `start_tool_call_span/end_tool_call_span` (tracer.rs:115-267) and `Meter::{start_new_invocation,start_cycle,end_cycle,update_cycle,end_tool_call,end_invocation}` (meter.rs:59-156) with `&str` error params and positional `Option<&Message>`; `CycleScope` (mod.rs:146) already shows the RAII shape. Only `CycleScope` and `Tracer::{new,config,local_traces}` should be `pub`; the start/end pairs are loop internals — `pub(crate)` them, or wrap each in a `*Scope` guard.
- 🟡 **`AgentSpanOptions<'a>` / `ModelSpanOptions<'a>` are borrowed, all-pub option structs** (tracer.rs:48-69) without `#[non_exhaustive]` (`AgentSpanOptions` derives `Default`, `ModelSpanOptions` does not); inconsistent with every other `*Options` in the crate. If they stay public, own their data and follow the `Default + with_*` pattern.
- 🟡 **`TelemetryConfig::from_env()` is the only env-reading constructor in the crate** (mod.rs:86) and its doc does not list the variables it reads (`OTEL_SERVICE_NAME`, semconv opt-in, …); document them and state precedence vs `with_trace_attributes`. (`Default` is derived — fine.)
- 🟢 `AttrValue` has `From<&str/String/i64/u64/f64/bool>` but `with_trace_attributes` takes `(String, AttrValue)` pairs — accept `(impl Into<String>, impl Into<AttrValue>)`. `to_telemetry_json` as a free fn at `telemetry::` root — consider `AgentMetrics::to_json`/`Serialize`.

## 6. `mcp/`

- 🔴 **`rmcp` types in the public surface without re-export** — `McpClient::server_info() -> Option<Arc<rmcp::model::ServerPeerInfo>>` (client.rs:220) and `McpTool::new(client, tool: &rmcp::model::Tool, ..)` (tool.rs:30); `rg 'pub use rmcp' lib.rs` = 0. Same for `indexmap`: `McpConfigFile::load() -> Result<IndexMap<String, McpServerConfig>>` (config.rs:263). Return crate-owned `McpServerInfo`/`Vec<(String, McpServerConfig)>` (or `pub use rmcp;` behind the `mcp` feature and say it's a public dep); make `McpTool::new` `pub(crate)`.
- 🟡 **No builder slot for MCP clients** — `rg mcp agent/builder.rs` = 0; TS `ToolList = (Tool | McpClient | Agent | ToolList)[]` lets `tools: [client]` load lazily. Users must `client.list_tools().await?` and push each `ToolRef`. Add `AgentBuilder::mcp_client(impl Into<McpClient>)` (loads on `build()`/first invoke) for parity with `tool`/`plugin`.
- 🟢 `ToolsChangedCallback` alias (client.rs:55) is `pub` but unused by the public `on_tools_changed(impl Fn..)` — make it private. `McpServerConfig::{stdio,streamable_http}` constructors + `LoadOptions`/`ListOptions` are fine; `McpConnectionState` and `McpTransportKind` need `#[non_exhaustive]`.

## 7. `agent/{agent_as_tool,delegation,continuation}.rs`

- 🟡 **Loop internals are public on `Continuations`** — `prepare()`, `mark_appended()`, `abandon()` (continuation.rs:95-123) plus the free `is_complete_message_input` (continuation.rs:143) are reachable via `Agent::continuations()` (agent/mod.rs:~300); a user calling `mark_appended()` corrupts the queue. Keep `add_input`/`has_input` pub, the rest `pub(crate)`.
- 🟢 `Agent::as_tool(&self, options: AgentAsToolOptions)` requires `AgentAsToolOptions::named(..)` — matches TS `asTool({name, description})`; fine. `AgentDelegation::new(tool_names)` + `delegate_tool` is a sensible port. Add `#[non_exhaustive]` to `AgentAsToolOptions` (agent_as_tool.rs:23) per convention.

## 8. `models/{anthropic,openai}/` + `models/sse.rs`

Surface: `AnthropicModel::{new,with_transport,config,max_tokens,format_request}`, `AnthropicModelConfig` (all-pub, `Default`, anthropic/mod.rs:105-132), `AnthropicCacheConfig/Section/Strategy`, `AnthropicTransport`, `ReqwestTransport`, `TransportResponse`, `ByteStream`, `CannedStream` (:477, outside `cfg(test)`), `map_error`, `format_request`, `format_count_tokens_request`, `map_stop_reason`, five `pub const`s (:33-54); `OpenAIModel::{new,with_transport}`, `OpenAIModelConfig` (all-pub, openai/mod.rs:35-61), `OpenAIApi`, `OpenAIRequest`, `OpenAITransport`, `ReqwestTransport`, `BedrockMantleConfig`, `resolve_mantle_base_*`, `classify_openai_error`; `SseEvent`, `SseParser` re-exported at `models::` (mod.rs:39).

- 🟡 **Config shape diverges from `BedrockModel`** — Bedrock got a `BedrockModelBuilder` (phase-1 #16); Anthropic/OpenAI are `AnthropicModel::new(AnthropicModelConfig { model_id: Some("..".into()), api_key: Some(key), ..Default::default() })?` — 12 `Option` fields, struct-literal only, and **not `#[non_exhaustive]`** (`rg non_exhaustive models/{anthropic,openai}` = 1 hit) so adding a field is a breaking change. Replacement: `AnthropicModel::builder().model_id("..").api_key(key).max_tokens(4096).build()?` (same for `OpenAIModel`), `#[non_exhaustive]` on both configs.
- 🟡 **`with_transport(config, transport: Arc<dyn AnthropicTransport>)`** (anthropic/mod.rs:218-221, openai/mod.rs:137) — `Arc` at the call site, against the DESIGN.md builder convention; take `impl AnthropicTransport + 'static`.
- 🟡 **Wire internals exported** — `format_request`, `format_count_tokens_request`, `map_stop_reason`, `map_error`, `CONTEXT_WINDOW_OVERFLOW_ERRORS`, `SERVER_TOOL_BLOCK_TYPES`, `ANTHROPIC_VERSION`, `MAX_PAUSE_TURN_CONTINUATIONS` (anthropic/mod.rs:10-54,309), `CannedStream` (:477), `classify_openai_error`, `OpenAIRequest`, `resolve_mantle_base_path/url` (openai/mod.rs:21,35-37 re-exports), and `SseEvent`/`SseParser` at `models::` root (mod.rs:39). None are needed to *use* a model; they freeze provider wire details into SemVer. `pub(crate)` them (keep `SseParser` under `models::sse` only if a third-party provider is the intended consumer — say so in the module doc).
- 🟢 `AnthropicModelConfig.max_tokens: Option<u64>` is the right width (phase-1 #14) — now `BedrockModelConfig.max_tokens: Option<i32>` is the odd one out. `api_key: Option<String>` prints in `Debug` (`#[derive(Clone, Debug, Default)]` :104) — redact in a manual `Debug` impl.

## 9. `lib.rs` / features

- 🟡 **`mcp-echo-server` is a user-visible feature** (`Cargo.toml:22-23`, used only by dev-deps line 59). It enables `rmcp/server` for every downstream that sees the feature list. Move the echo server to a `dev-dependency`-only crate under `crates/` or an `examples/`-style bin, or mark it `# internal` in `[features]` docs and exclude from `--all-features` docs builds.
- 🟡 **`default` turns on every provider + telemetry** (`Cargo.toml:15`): `anthropic`+`openai` pull `reqwest`, `telemetry` pulls `opentelemetry_sdk`, `mcp` pulls `rmcp+http+regex`, for a Bedrock-only user. Phase-1 kept defaults minimal; propose `default = ["bedrock", "macros"]`, and `full = [...]`.
- 🟢 `lib.rs:42` re-exports `InterruptState` (the mutable loop-internal state, see §2) at the crate root next to `Interrupt`/`InterruptSource`; drop it from the root once its mutators are `pub(crate)`. `InterruptParams`/`InterruptResponse` are correctly at the root (lib.rs:67-68).

## Verified against / not covered

- `git log -1` = `5276b6593` on `agent-tasks/strands-rs`; read-only, no cargo. Every `file:line` above was read with `sed`/`rg`; TS siblings cited from `strands-ts/src/{middleware,interventions,agent}`.
- Not re-raised: phase-1/phase-2 accepted or skipped items (`ToolExecutor` out-params, `BedrockModelConfig.max_tokens: i32`, `HookRegistry` permutations, harness-crate options).
- Not checked (time-box): `models/openai/{chat,responses}.rs` item-level docs, `telemetry/json.rs`, `mcp/config.rs` env-interpolation semantics, and whether `missing_docs` is clean for the new modules (needs `cargo doc`).

## Fix now (ranked)

1. 🔴 §1 `wrap_fn`/`Next<'a,S>`: make `Next` owned so closures infer and no `BoxFuture` cast (registry.rs:69,123).
2. 🔴 §2 `InterruptStateHandle = Arc<parking_lot::Mutex<..>>` returned by `Agent::interrupt_state()` — opaque handle / `pub(crate)` (interrupt.rs:272, agent/mod.rs:317).
3. 🔴 §2 `AgentBuilder::interventions(..) -> Result<Self>` + `Arc<dyn>` → `intervention(impl InterventionHandler)`; validate in `build()` (builder.rs:148).
4. 🔴 §6 `rmcp::model::{ServerPeerInfo,Tool}` + `IndexMap` in pub signatures (client.rs:220, tool.rs:30, config.rs:263).
5. 🔴 §5 `SpanHandle::context() -> Option<&opentelemetry::Context>` without `pub use opentelemetry` (tracer.rs:28).
6. 🟡 §1 async `add_input`/`add_output` handlers; drop `add_wrap_middleware` dup; add `ExecuteToolContext::interrupt()`.
7. 🟡 §2/§3 `InvokeInput::{InterruptResponses, CheckpointResume}` should carry `InterruptResponse`/`Checkpoint`, not the wire wrappers.
8. 🟡 §2 `InterventionHandler` missing `before_model_call`/`after_model_call`/`on_error` (TS handler.ts:19,60-64).
9. 🟡 §5/§7/§8 `pub(crate)` the loop/wire internals: `Tracer::start_*/end_*`, `Meter::*_cycle/*_invocation`, `Continuations::{prepare,mark_appended,abandon}`, Anthropic/OpenAI `format_*`/`map_*`/consts.
10. 🟡 §8/§9 `AnthropicModelConfig`/`OpenAIModelConfig` → `#[non_exhaustive]` + builders like `BedrockModelBuilder`; `mcp-echo-server` out of the user-facing feature list.

## Fixes applied

Branch `agent-tasks/strands-rs-api-fix` (worktree `/home/sandbox/wt/api-fix`). 🔴 items 1–5 of
"Fix now", plus part of item 10. Paths relative to `strands-rs/crates/strands-agents/src/`.

1. **§1 `Next` is owned** — `Next<S>` now holds `Arc<[HandlerRef<S>]>` + index + `Arc<dyn Terminal<S>>`
   (registry.rs:68-97); `Middleware::call`/`WrapFn`/the input+output adapters take `Next<S>`.
   `WrapFn` is now `WrapFn<S>` (boxes the future internally), so `wrap_fn` is
   `wrap_fn<S, F, Fut>(f) -> WrapFn<S>` with no HRTB and **no `BoxFuture` in any public bound** —
   hence no re-export needed. `agent.add_middleware::<InvokeModelStage>(wrap_fn(|ctx, sink, next|
   async move { next.run(ctx, &sink).await }))` compiles with zero type annotations (verified).
   `MiddlewareRegistry::invoke` now takes `terminal: impl Terminal<S> + 'static`.
   Crate tests (middleware/tests.rs, tests/middleware_loop.rs) dropped every `Box::pin(..) as
   BoxFuture<'_, ..>` cast and all `Next<'_, S>` annotations. Caveat: when the closure is wrapped in
   a block (`wrap_fn({ let x = ..; move |ctx, ..| })`), inference still needs the *first* param
   annotated (`ctx: InvokeModelContext`) — rustc can't resolve `S` before checking the closure body
   in that form; the plain `wrap_fn(move |ctx, sink, next| ..)` form needs nothing.
2. **§2 no `parking_lot` in the public API** — `pub type InterruptStateHandle = Arc<Mutex<..>>` is now
   `pub(crate) type SharedInterruptState`, and `InterruptStateHandle` is an opaque newtype
   (interrupt.rs:272-310) with `is_activated()`, `interrupts()`, `unanswered()`, `pending()`,
   `snapshot()`. `Agent::interrupt_state()` returns it; the loop uses the new
   `pub(crate) Agent::interrupt_state_shared()`. `interrupt::raise` is `pub(crate)` (it took the
   `Arc<Mutex<_>>`).
3. **§2 builder** — `interventions(..) -> Result<Self>` is gone. `AgentBuilder::intervention(impl
   InterventionHandler + 'static)` (repeatable) + `interventions(impl IntoIterator<Item = Arc<dyn
   InterventionHandler>>)` (extends), both infallible; handlers accumulate in a builder field and the
   duplicate-name check runs in `build()` (builder.rs:145-164, 303-319). No fallible builder method
   remains. Harness caller `crates/strands-harness/src/agent.rs:294` lost its `?` — the one harness edit.
4. **§6 MCP** — `McpClient::server_info()` now returns an owned
   `McpServerInfo { name, version, protocol_version, instructions }` (`#[non_exhaustive]`,
   mcp/client.rs) instead of `Arc<rmcp::model::ServerPeerInfo>`; `McpTool::new` is `pub(crate)`
   (the `&rmcp::model::Tool` param is no longer reachable); `IndexMap` is re-exported as
   `mcp::IndexMap` with a doc note that it is a public dep (it is carried by both
   `ConfigSource::Inline` and `ConfigSource::load`, so re-export beats changing one of them).
5. **§5 telemetry** — `pub use ::opentelemetry;` under `telemetry::` (feature-gated, documented as a
   public dependency and pointed at from `SpanHandle::context`). Method kept public.
10. (partial) `mcp-echo-server` is now documented in `Cargo.toml` as internal/test-only, not part of
   the supported feature set. **Skipped: `#[non_exhaustive]` on `AnthropicModelConfig`/
   `OpenAIModelConfig`** — both already derive `Default`, but `#[non_exhaustive]` forbids struct-literal
   construction from *other* crates entirely (even with `..Default::default()`), which breaks
   `tests/integ_anthropic.rs:30,118` and `tests/integ_openai.rs:14`. It only becomes cheap together
   with the `AnthropicModel::builder()`/`OpenAIModel::builder()` half of item 10 — do them as one change.

Not touched: every 🟡/🟢 item, including §1 async input/output handlers, `add_wrap_middleware`,
`EventSink::collector`'s `parking_lot::Mutex` leak, `InterruptState`'s public mutators, and
`AskFn`/`InterruptFn`/`ToolsChangedCallback` `Arc`-at-call-site aliases.

Gates (from `strands-rs/`): `cargo fmt` clean; `cargo clippy --all-targets --all-features -p
strands-agents -- -D warnings` → `Finished dev profile [unoptimized] target(s) in 19.70s`;
`cargo test --all-features -p strands-agents` → all suites ok, lib `880 passed; 0 failed`;
`cargo check -p strands-harness` → `Finished dev profile [unoptimized] target(s) in 11.86s`.
