# strands-rs design decisions

`strands-agents` (Rust) is a port of the core of `strands-ts` (`@strands-agents/sdk`). The TS
source and its tests are the behavioural spec; this document records where Rust idiom deliberately
diverges from the TS structure. Everything not listed here is a literal translation.

## Scope (v1)

In: `types/` (messages, content blocks, media, tool specs, JSON), `models/` (`Model` trait,
stream events, aggregation, Bedrock Converse provider), `tools/` (`Tool` trait, `FunctionTool`,
registry, sequential + concurrent executors), `hooks/` (registry + all events), `plugins/`,
`agent/` (event loop, `invoke`/`stream`, state store, continuation/resume), `conversation_manager/`
(null + sliding window), `session/` (file storage + session manager), `retry/`, `error.rs`.

Since then (phases 2–3) the scope grew to: storage, context manager (replaces conversation
managers — `ContextManagerOption::Auto` is the Agent default), background tasks (engine with
max_concurrency/timeout/dedup), memory manager, skills/context-injector plugins, sandbox
(`Sandbox` trait + `LocalSandbox`; shell runs through it), middleware (TypeId-keyed stages,
owned `Next`), interrupts/HITL (`Error::Interrupt`, `ToolContext::interrupt`,
`InvokeInput::InterruptResponses`, `HumanInTheLoop`), snapshots/checkpoints, telemetry (OpenTelemetry
`gen_ai.*`), MCP (rmcp 3.x; stdio + streamable-http — rmcp has no SSE client), agent-as-tool,
delegation, continuations, Anthropic + OpenAI (Responses/Chat/bedrock-mantle) providers.

Still out: printer, A2A, multiagent (swarm/graph), S3 session storage, docker/ssh sandboxes,
Gemini/LiteLLM/OpenRouter providers, OTLP exporter installation (the SDK emits spans; the host
installs the exporter).

## Core shapes

| TS | Rust | Why |
|---|---|---|
| `Agent` class, mutable fields | `Agent` is a cheaply-`Clone` **handle** (`Arc<AgentInner>`); state behind `RwLock`/`Mutex`; `invoke`/`stream` take `&self` | Hook callbacks and tool contexts receive the agent while the loop is running. A handle avoids `&mut` borrow conflicts and lets callbacks be `'static`. Same pattern as `reqwest::Client`/tokio handles. |
| `JSONValue` | `serde_json::Value` (`preserve_order`) | Insertion order is observable in tool specs. |
| `Message` / `ContentBlock` class-per-variant with `toJSON` | `struct Message`, `enum ContentBlock` with serde externally-tagged repr (`{"text": "..."}`, `{"toolUse": {...}}`), camelCase field names | Wire-compatible with TS/py session files. |
| `StopReason` open string union | `enum StopReason { EndTurn, ToolUse, …, Other(String) }` with custom serde | Unknown values must round-trip. |
| `Model` abstract class, generic over config | `trait Model: Send + Sync` (object-safe, no associated config type); provider config lives on the concrete struct (`BedrockModel::config()/update_config()` with interior mutability) | Agent holds `Arc<dyn Model>` (`AgentBuilder::model` takes `impl Model + 'static` and wraps it); config generics would break object safety. |
| `AsyncIterable<ModelStreamEvent>` | `BoxStream<'_, Result<ModelStreamEvent, Error>>` | |
| `Tool.stream()` async generator (yields progress, returns `ToolResultBlock`) | `#[async_trait] trait Tool { async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock, Error> }`; progress via `ctx.progress(value).await` (mpsc sender in the context) | Generators don't exist; a result-returning future + side channel is the idiomatic equivalent and keeps per-tool event ordering. `Err` is reserved for exceptional control flow (cancellation, future interrupts); tool failures are `Ok(result with status: error)` exactly as in TS. |
| `FunctionTool` / `tool()` with zod | `FunctionTool::builder(name).handler(|input: JsonValue, ctx| ..)` for raw JSON, or `.typed_handler(|input: T, ctx| ..)` for `T: schemars::JsonSchema + DeserializeOwned` (schema derived, input validated with `jsonschema`) | schemars is the Rust analogue of zod. |
| Hook events = classes; registry keyed by constructor | one `struct` per event implementing `trait HookEvent: Any + Send` (`fn reverse_callbacks() -> bool`); `HookRegistry` keyed by `TypeId`; callbacks `for<'a> Fn(&'a mut E) -> BoxFuture<'a, Result<(), Error>> + Send + Sync + 'static`, invoked **sequentially** in `order` (ties: registration order), reversed for `After*` events | Mutable control fields (`cancel`, `retry`, `selected_tool`, `result`, …) are written by callbacks and read by the loop. |
| `agent.stream()` async generator returning `AgentResult` | `agent.stream(input) -> AgentStream` / `agent.stream_with(input, InvokeOptions)` (`impl Stream<Item = Result<AgentStreamEvent, Error>>`), loop runs in a spawned task feeding an mpsc channel; the final `AgentStreamEvent::AgentResult` is the last item; `invoke()` drains and returns the result | Mirrors TS where `AgentResultEvent` is the last yielded event. Cleanup events (`AfterTools`, `AfterInvocation`) are emitted explicitly, never from `Drop`. |
| `AbortSignal` | `tokio_util::sync::CancellationToken` | |
| `Error` subclasses | one `thiserror` `enum Error` with a variant per TS error class (`ContextWindowOverflow`, `ModelThrottled`, `MaxTokens`, `ToolNotFound`, `ToolValidation`, `Cancelled`, …) plus `Other(Box<dyn std::error::Error + Send + Sync>)` | The loop matches on error kinds. |
| `logger` | `tracing` macros; `warn_once` keeps a process-wide `HashSet` | |
| `Plugin` interface | `#[async_trait] trait Plugin: Send + Sync { fn name(&self) -> &str; async fn init_agent(&self, agent: &Agent) -> Result<()>; fn tools(&self) -> Vec<Arc<dyn Tool>> }` | |
| `SessionStorage` / `ConversationManager` | traits `SnapshotStorage` / `ConversationManager`; `FileStorage`, `SlidingWindowConversationManager`, `NullConversationManager` | |

## Conventions

- Module layout mirrors `strands-ts/src` (snake_case): `types/`, `models/`, `tools/`, `hooks/`,
  `plugins/`, `agent/`, `conversation_manager/`, `session/`, `retry/`, `error.rs`, `state_store.rs`.
- Public items: short (1–4 line) `///` docs. `#![forbid(unsafe_code)]`, `missing_docs = warn`,
  clippy `all` warn-free with `-D warnings`.
- Tests: unit tests in `#[cfg(test)] mod tests` next to the code (mirroring `__tests__/`),
  integration tests in `crates/strands-agents/tests/`. Every TS `it(...)` has a Rust test with the
  same intent; the name is the TS name in snake_case. Fixtures (`MockMessageModel`, hook
  recorder) live in `src/test_support.rs` (`#[cfg(test)]`/`pub` behind `test-support` feature).
- Gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-features`.
- Public API shape: builders take `impl Trait + 'static` and wrap in `Arc` internally
  (`AgentBuilder::{model, aux_model, tool, plugin, storage, sandbox, tool_executor}`). There are no
  `*_arc`/`*_ref` twins: every one of those traits has a blanket `impl Trait for Arc<T>`, so an
  `Arc<dyn Trait>` is itself `impl Trait + 'static` and goes through the same method (one extra
  pointer hop when a shared handle is re-wrapped). Typed plugin slots take
  their own trait: `conversation_manager(impl ConversationManager)`,
  `retry_strategy(impl ModelRetryStrategy)`, `session_manager(impl Into<Arc<SessionManager>>)`.
- S3 backends are built by an async builder (`S3Storage::builder(bucket).build().await`), since
  resolving the AWS config is async; the snapshot one is `session::S3SessionStorage` (TS calls it
  `S3Storage`) so it does not clash with the unified `storage::S3Storage`.
- `AgentBuilder::build()` and `Agent::new()` are **synchronous**: they allocate the agent and
  nothing else. `Agent::initialize()` (plugins, session restore, sandbox tools) runs lazily on the
  first `invoke`/`stream`; call it explicitly to observe that state earlier.
- Option/result structs that will grow are `#[non_exhaustive]` and built from `Default` plus
  `with_*` setters (`InvokeOptions::default().with_limits(Limits::default().with_turns(2))`);
  public enums that mirror a provider's wire format are `#[non_exhaustive]` too.
- `#[tool]` re-propagates control-flow errors (`Error::Interrupt`, `Error::Cancelled`) instead of
  turning them into error tool results, via `tools::tool_error_or_propagate` (the macro only sees
  `E: Display`, so it downcasts `Box<dyn Any>` to `Error`); this requires `E: 'static`.
- `AgentStreamEvent` carries accessor shortcuts (`text_delta`, `tool_use`, `tool_result`,
  `result`) so the common streaming paths don't need a nested destructure; `Agent::stream_text`
  is the text-only view. `ModelStopData.usage` exposes the call's token usage to after-model hooks.
- Hook callbacks are plain closures returning a future (`hooks.add_callback(|e: &mut Event| async move
  { Ok(()) })`, plus `AgentBuilder::hook`/`sync_hook` for build-time registration); registration returns
  `()` and the `*_scoped` variants return the `#[must_use]` `HookCleanup`. `add_callback_boxed*` keeps the
  `|e| Box::pin(async move { .. })` form, needed when the future borrows the event (a closure's return
  type cannot capture the event's lifetime).
