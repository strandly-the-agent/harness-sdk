# API bar-raise: strands-agents (Rust) — 2026-10-14T00:21:22Z

_Incremental; findings appended as found._

## 🔴 Blocking

1. **`AgentBuilder` typed-plugin slots accept any `Plugin`** — `src/agent/builder.rs:89,101,107`: `conversation_manager(Arc<dyn Plugin>)`, `session_manager(Arc<dyn Plugin>)`, `retry_strategy(Arc<dyn Plugin>)`. Nothing stops `.conversation_manager(my_session_manager)`; the type system no longer documents the slot. Use the specific trait: `fn conversation_manager(self, m: impl ConversationManager + 'static) -> Self` (same for `impl SessionManager`, `impl ModelRetryStrategy`), coercing to `Arc<dyn Plugin>` internally.
2. **`Arc<dyn …>` forced on every caller** — `builder.rs:53,65,71,77,83,119`, `agent/mod.rs:143`, `examples/tools.rs:22,30` show the cost: `Arc::new(FunctionTool::builder(..).typed_handler(..))`, `.model(Arc::new(model))`. Take `impl Model + 'static` / `impl Tool + 'static` / `impl Plugin + 'static` and `impl IntoIterator<Item = ToolRef>` for the plural forms; keep an `*_arc` or `From<Arc<dyn Tool>>`-based path for shared instances (`impl Into<ToolRef>` covers both if you add `impl<T: Tool + 'static> From<T> for ToolRef`-style helper type).
3. **SemVer: all-public-field structs without `#[non_exhaustive]`** — `agent/result.rs:12` `AgentResult`, `event_loop.rs:48` `Limits`, `event_loop.rs:91` `InvokeOptions`, `tool_caller.rs:19` `CallOptions`, `tools/tool.rs:24` `ToolContext`, `tools/executor.rs:23` `ExecuteToolsInput`, `tools/tool.rs:16` `ToolStreamEvent`. `AgentStreamEvent` (`event_loop.rs:104`) *is* `#[non_exhaustive]`, so the convention exists but isn't applied. Every one of these will grow (TS `InvokeOptions` already has more fields: interrupts, structured output type). Add `#[non_exhaustive]` + `Default`/constructor; `ToolExecutorEvent` (`executor.rs:36`) and `InvokeInput` (`agent/mod.rs:35`) likewise.
4. **`stream` has no options-less form; `invoke` has no streaming parity** — `event_loop.rs:265` `stream(&self, input, options: InvokeOptions)` vs `event_loop.rs:221/226` `invoke(input)` + `invoke_with(input, options)`. TS: `stream(input, options?)`. Mirror: `stream(input)` + `stream_with(input, options)`.

## 🟡 Should fix before 0.1

5. **DESIGN.md drift** — DESIGN names `FileSessionStorage`/`SessionStorage` → code exports `FileStorage`/`SnapshotStorage` (`lib.rs:33`); DESIGN `Error::AgentCancelled` → code `Error::Cancelled` (`error.rs:79`); DESIGN `FunctionTool::builder(name).input::<T>()` → code `typed_handler::<I,…>()` (`function_tool.rs:113`) and no `.input::<T>()`. Pick one and update the other.
6. **`AgentResult::to_json()`** (`result.rs:25`) — hand-rolled JSON on a type that should just `#[derive(Serialize)]` (C-CONV says `to_*` is for cheap borrowed conversions; the rest of `types/` uses serde). Derive `Serialize` and drop `to_json`.
7. **`ToolExecutor::execute` out-param** (`executor.rs:55-59`): `results: &mut Vec<ToolResultBlock>` + `events: &mpsc::Sender<..>` leaks tokio channel type into a public trait and is un-Rusty. Prefer `async fn execute(&self, input: ExecuteToolsInput, events: ToolEventSink) -> Result<Vec<ToolResultBlock>>` with partial results carried in the error (`Error::Cancelled { partial: Vec<..> }`) or a small `ExecutionOutcome` struct.
8. **`FunctionToolBuilder::handler/typed_handler` are terminal** (`function_tool.rs:96,113`) — builders elsewhere (`AgentBuilder::build`, `BedrockModel::builder?`) end in `build()`. Either rename to `build_with(handler)` or accept the handler earlier and keep `build()` for symmetry; also `tool(name)` (`function_tool.rs:61`) duplicates `FunctionTool::builder(name)` — export one.
9. **Zero `#[must_use]` in the crate** (`grep -rc must_use src` = 0). Builders (`AgentBuilder` `builder.rs:30`, `FunctionToolBuilder` `function_tool.rs:54`, `BedrockModelBuilder` `bedrock/mod.rs:282`), `AgentStream` (`event_loop.rs:178`), `ToolHandle` (`tool_caller.rs:37`) and especially `HookCleanup` (`hooks/registry.rs:62`: dropping it silently leaks the hook, no `Drop` impl) need `#[must_use]`.
10. **`ContentBlock`, `ModelStreamEvent`, `ContentBlockStart/Delta`, `ToolExecutorEvent`, `SystemContentBlock`, `ToolChoice`, `ReduceTrigger` are exhaustive public enums** (`types/messages.rs:169`, `models/streaming.rs:12,94,111`, `tools/executor.rs:36`, `types/system_prompt.rs:10`, `types/tool.rs:66`, `conversation_manager/mod.rs:56`). Only 4 items in the crate carry `#[non_exhaustive]` (`AgentStreamEvent`, `Error`, `BackoffContext`, `StopReason`). Bedrock/TS already add block kinds (video, citations, interrupts are listed as "out of scope, keep the seams" in DESIGN.md) — add `#[non_exhaustive]` now or every addition is a major bump.
11. **Test-assertion strings in the public API** — `CONCURRENT_INVOCATION_MESSAGE`, `STRUCTURED_OUTPUT_FORCED_FAILURE_MESSAGE` (`event_loop.rs:31,41`, re-exported `agent/mod.rs:26`), `DIRECT_CALL_DURING_INVOCATION_MESSAGE` (`tool_caller.rs:13`), `HOOK_CANCELLED_MESSAGE`/`EXECUTION_CANCELLED_MESSAGE` (`executor.rs:18,20`), `default_*_warning_message`, `get_context_window_limit` (`models/mod.rs:11-13`; `get_` prefix violates C-GETTER), `session::{validate_identifier, validate_scope, validate_uuid_v7}` (`session/mod.rs:17`), `register_model_retry_hook` (`retry/mod.rs:22`). Users should match `Error` variants, not compare strings. Make these `pub(crate)` (tests are in-crate) or move behind `test-support`.
12. **Two public `Redaction` types** — `models/model.rs:53` and `hooks/events.rs:68` are structurally identical (`{ user_message: String }`) but distinct; `hooks::*` is glob-re-exported (`hooks/mod.rs:6`) so `use strands_agents::{hooks::*, models::*}` collides. Keep one in `types/` and re-use it.
13. **`&str` vs `impl Into<String>` inconsistency** — `SessionManager::new(session_id: &str, …)` (`session_manager.rs:81`), `Agent::tool(&self, name: &str)` fine, but `AgentBuilder::id/name` take `impl Into<String>`. Also `SessionManager::new` takes `Arc<dyn SnapshotStorage>` (same Arc-at-call-site problem as #2) and `save_snapshot(agent, is_latest: bool)` is a bool flag → `save_snapshot(agent)` + `save_latest(agent)` or a small enum.
14. **`BedrockModelConfig.max_tokens: Option<i32>`** (`bedrock/mod.rs:214`) vs `Limits.output_tokens: Option<u64>` (`event_loop.rs:52`) and `Usage` u64 fields. Use `u32`/`u64`; convert at the SDK boundary.
15. **`types::JsonSchema = serde_json::Value`** (`types/mod.rs:29`) collides with `schemars::JsonSchema` (the trait every typed-tool user imports, see `examples/tools.rs:7`). `ToolSpec::with_input_schema(schema: JsonSchema)` (`types/tool.rs:40`) reads as taking the trait. Rename to `InputSchema` or just use `JsonValue`.
16. **`BedrockModelBuilder` not re-exported** — `models/mod.rs:10` exports only `BedrockModel, BedrockModelConfig`; builder is reachable only via `models::bedrock::BedrockModelBuilder`. Export it alongside (and `CacheConfig`, `BedrockGuardrailConfig` which are fields of the exported config).

## 🟢 Nits

17. `HookRegistry` has 4 `add_*` permutations (`registry.rs:90-142`); `add_callback(cb)` + `add_callback_with_order(order, cb)` and let sync closures be wrapped by the caller — or one `add(order, cb)` with `hook_order::DEFAULT`.
18. `Message::text()`/`SystemPrompt::text()`/`ToolResultBlock::text()` allocate a `String` (`messages.rs:131,316`, `system_prompt.rs:31`) — C-GETTER-ish name for a joining conversion; `to_text()`/`joined_text()`.
19. `InvokeInput::None` (`agent/mod.rs:37`) shadows `Option::None` under glob import; `InvokeInput::Empty`. `From<&str>`/`From<String>`/`From<Message>` exist (`agent/mod.rs:59-79`) — good, but no `From<ContentBlock>`.
20. `AgentStreamEvent::BeforeToolCall(crate::hooks::events::BeforeToolCallEvent)` (`event_loop.rs:123,127`) uses a full path while siblings use imported names — cosmetic but shows `hooks` re-export list (`event_loop.rs:15-17`) was hand-maintained; the glob re-export in `hooks/mod.rs:6` already covers it.
21. `Agent::new(model: Arc<dyn Model>) -> Result<Agent>` is `async` (`builder.rs:213`) — a constructor named `new` that must be awaited and can fail surprises; keep only `builder().build().await` or name it `Agent::with_model`.
22. `ToolContext::cancel: CancellationToken` (required) vs `InvokeOptions.cancel: Option<CancellationToken>` / `StreamOptions.cancel: Option<..>` — consistent *by role* (input optional, context always present), OK; but document the child-token relationship (`event_loop.rs:295`) on `InvokeOptions.cancel`.

## Verified against
- `git log -1` = `24fd4a68e` on `agent-tasks/strands-rs`; read-only (no files modified).
- DESIGN.md decisions (Agent handle, single `Error`, `Result`-returning tools, TypeId hooks) not re-litigated; #5 lists only where code and DESIGN.md disagree.

## Fixes applied / skipped — 2026-10-14 (worktree `agent-tasks/strands-rs-fix-api`)

### Applied
- **#1 typed builder slots** — `conversation_manager(impl ConversationManager + 'static)`,
  `retry_strategy(impl ModelRetryStrategy + 'static)`, `session_manager(impl Into<Arc<SessionManager>>)`
  (the builder now stores `Option<Arc<SessionManager>>`, not `Arc<dyn Plugin>`).
- **#2 no `Arc` at call sites** — `model(impl Model + 'static)`, `tool(impl Tool + 'static)`,
  `plugin(impl Plugin + 'static)`, `tool_executor(impl ToolExecutor + 'static)`,
  `tools(impl IntoIterator<Item = ToolRef>)`, `plugins(impl IntoIterator<Item = Arc<dyn Plugin>>)`,
  `messages(impl IntoIterator<Item = Message>)`, `Agent::new(impl Model + 'static)`,
  `Agent::set_model(impl Model + 'static)` (+ `set_model_arc`). Shared-instance
  escape hatches kept: `model_arc`, `tool_ref`, `plugin_arc`, `tool_executor_arc`.
  `Arc::new` is gone from all four `examples/*.rs`.
- **#3 `#[non_exhaustive]`** on `AgentResult`, `Limits`, `InvokeOptions`, `CallOptions`, `ToolContext`,
  `ExecuteToolsInput`, `ToolStreamEvent` (+ `ToolExecutorEvent` from #10). Constructibility preserved:
  `Limits::with_{turns,output_tokens,total_tokens}`, `InvokeOptions::with_{invocation_state,cancel,limits,
  structured_output_schema}`, `CallOptions::with_{record_direct_tool_call,invocation_state}`,
  `AgentResult::new(..).with_structured_output(..)`, `ExecuteToolsInput::new(..)`,
  `ToolStreamEvent::{new, default}`. All struct-literal sites in `tests/` rewritten to `with_*` chains.
- **#4 `stream`/`stream_with`** — `stream(input)` added, two-arg form renamed `stream_with(input, options)`;
  `invoke_with` and all test/example call sites updated.
- **#9 `#[must_use]`** on `AgentBuilder`, `FunctionToolBuilder`, `BedrockModelBuilder`, `AgentStream`,
  `ToolHandle`, `HookCleanup` (+ doc note: dropping a `HookCleanup` leaves the hook registered;
  there is no `Drop` impl). 47 in-crate/test call sites now bind `let _ =` explicitly.
- **#10 `#[non_exhaustive]`** on `ContentBlock`, `ModelStreamEvent`, `ContentBlockStart`,
  `ContentBlockDelta`, `ToolExecutorEvent`, `SystemContentBlock`, `ToolChoice`, `ReduceTrigger`.
  No `match` site needed a `_ =>` arm (in-crate matches stay exhaustive); verified with
  `cargo build --all-targets --all-features`.
- **#12 one `Redaction`** — `hooks::events::Redaction` is now `pub use crate::models::Redaction;`
  (`models::Redaction` gained `Serialize` + camelCase to keep the wire form). New test
  `hooks::tests::hooks_and_models_globs_do_not_collide` imports both globs and constructs it.
- **#15 `JsonSchema` out of signatures** — `ToolSpec::{input_schema, output_schema}` and
  `with_input_schema`/`default_input_schema` now use `JsonValue`. `types::JsonSchema` kept as a
  documentation-only alias, still absent from the `lib.rs` re-exports.
- **#16 bedrock re-exports** — `models::{BedrockModelBuilder, CacheConfig, CacheSection, CacheStrategy,
  BedrockGuardrailConfig, BedrockGuardrailRedactionConfig, GuardrailTraceMode,
  GuardrailStreamProcessingMode, IncludeToolResultStatus}` under `feature = "bedrock"`.
- **#6 `AgentResult: Serialize`** — derived with `#[serde(tag = "type", rename = "agentResult",
  rename_all = "camelCase")]` and `#[serde(skip)]` on `invocation_state`; `to_json` kept as a
  one-line delegate so the wire shape and the existing tests are unchanged.
- **#5 doc drift** — `docs/DESIGN.md`: `FileStorage`/`SnapshotStorage`, `Error::Cancelled`,
  `typed_handler`/`handler` instead of `.input::<T>()`, `stream`/`stream_with`, plus two new
  convention bullets (impl-trait builders + `#[non_exhaustive]` option structs).
  `docs/PORTING.md`: new rows for the builder, `stream`/`stream_with`, `ToolHandle`, `HookCleanup`,
  and the `JsonSchema` alias caveat.
- **#11 (partial)** — `CONCURRENT_INVOCATION_MESSAGE`, `STRUCTURED_OUTPUT_FORCED_FAILURE_MESSAGE`,
  `DIRECT_CALL_DURING_INVOCATION_MESSAGE`, `HOOK_CANCELLED_MESSAGE`, `EXECUTION_CANCELLED_MESSAGE`
  marked `#[doc(hidden)]` (kept `pub`: narrowing to `pub(crate)` would mean editing the
  `agent/mod.rs` re-export list, which a parallel worker owns). `register_model_retry_hook` left
  `pub` — the builder path needs it.

### Skipped
- **#7** `ToolExecutor::execute` out-params — a trait-signature redesign touching
  `tools/executor.rs` + `event_loop.rs`, both owned by the parallel correctness worker. Not attempted.
- **#8** `FunctionToolBuilder::handler/typed_handler` terminal vs `build()`, and `tool()` vs
  `FunctionTool::builder()` duplication.
- **#13** `SessionManager::new(&str, Arc<dyn SnapshotStorage>)` and the `save_snapshot(_, bool)` flag.
- **#14** `BedrockModelConfig.max_tokens: Option<i32>` → `u32`.
- **#17–#22** (nits): `HookRegistry` `add_*` permutations, `text()` → `to_text()`,
  `InvokeInput::None` → `Empty`, the `crate::hooks::events::` full paths in `AgentStreamEvent`,
  removing `Agent::new`, and the `InvokeOptions.cancel` child-token doc note.
- Pre-existing `cargo doc` warnings (private intra-doc link in `sliding_window.rs:31`, two redundant
  link targets in `lib.rs`) left alone — unrelated to this pass.
