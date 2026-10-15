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

## A2A

The `a2a` cargo feature serves a Strands agent over the Agent-to-Agent protocol and calls remote
ones. Protocol wire types live in-tree (`a2a::types`) rather than coming
from an SDK as in TS (`@a2a-js/sdk`); serde matches the wire format byte-for-byte. Decisions, each
a deliberate divergence from `strands-ts/src/a2a/`:

- `A2AExecutor` takes a concrete `Agent` (and an `AgentFactory = Arc<dyn Fn(&str) -> Agent>`), not
  an `InvokableAgent`: streaming and snapshots are local-agent capabilities, so TS's runtime
  `asSnapshotAgent` check is a compile-time guarantee here.
- The two context-isolation modes are separate constructors — `A2AExecutor::with_factory` (a
  dedicated agent per A2A `contextId`, the recommended mode) and `A2AExecutor::new` (one shared
  agent whose session snapshot is swapped per context under a lock) — so TS's "provide exactly one
  of `agent`/`agentFactory`" runtime throw becomes impossible to express.
- `with_max_contexts(0)` clamps to 1 instead of throwing, matching the crate's "setters don't
  fail" convention.
- `AsyncLock` is not ported: in factory mode a `tokio::sync::Mutex<Agent>` *is* the per-context
  lock, so acquiring it and getting the agent are one step; shared mode uses a plain
  `tokio::sync::Mutex<()>`.
- `A2AError` has no Rust twin. Protocol errors travel as `Error::Other(JsonRpcError)` so a server
  can recover the JSON-RPC code; agent failures propagate as the crate's own errors unchanged.
- `ExecutionEventBus` becomes `a2a::EventBus`, an unbounded `tokio::sync::mpsc` sender; the
  consumer (JSON-RPC handler) drains the receiver to aggregate a task or to emit SSE.

The server (`a2a::A2AServer`) is `axum` rather than express, and the request handling TS gets from
`@a2a-js/sdk`'s `DefaultRequestHandler` + `InMemoryTaskStore` is in-tree over `a2a::TaskStore`,
which folds the executor's events into one `Task`. `A2AServer::builder(name)` mirrors
`AgentBuilder`; `router() -> axum::Router` mounts into a host application and
`serve() -> ServerHandle` runs it (`local_addr`, `agent_card`, graceful `shutdown`). Further
deliberate divergences:

- `.agent()` and `.executor()` write the same builder slot, so TS's "provide exactly one of
  `agent`/`agentFactory`" throw is unexpressible; `build()` fails only when neither was set.
- The task store is concrete and in-memory, not a trait: TS's `taskStore` option has no twin until
  there is a second implementation to abstract over.
- The card is served at both `/.well-known/agent-card.json` and the legacy
  `/.well-known/agent.json`, advertises protocol version `0.3.0` (not TS's `0.2.0`), and takes its
  description from the served agent in `.agent()` mode — TS's `__agent_card__` placeholder factory
  call is not ported.
- Routes mount under the path of `http_url` with `serve_at_root(true)` forcing the root, following
  Python rather than TS (whose express server always mounts at `/`).
- `message/send` always runs to completion: `configuration.blocking: false` is treated as blocking
  since nothing delivers a result out of band yet. An executor failure marks the task `failed` and
  returns its JSON-RPC code.
- No auth hook (TS's `UserBuilder`): hosts wrap `router()` in their own `axum` middleware.

The client half is `a2a::A2AAgent` — a remote agent built with `A2AAgent::builder(url)`, invocable
(`invoke`/`invoke_with`) and streamable (`stream`/`stream_with`, yielding `A2AStreamEvent`s) like a
local one, and usable as an `Arc<dyn InvokableAgent>` or, via `as_tool`, as a tool of a local
agent. `tools::vended::A2AClientTool` is the vended `a2a_client` tool: `discover` plus
`send_message` over an allow-list of endpoints. Deliberate divergences from
`strands-ts/src/a2a/a2a-agent.ts` and `strands-ts/src/vended-tools/a2a-client/`:

- TS's per-endpoint `ClientFactory` has no twin: auth, proxies and timeouts belong on a
  `reqwest::Client`, so `.http_client(..)` (and `.endpoint_with_client(..)` on the tool) is the
  extension point.
- `A2AAgent::name()` returns `&str`, so an unnamed agent reports its URL until the card arrives
  (TS's `name` is `undefined` until then); the card's values land in a `OnceLock` and never
  overwrite a configured name or description.
- `A2AAgent` remembers the remote's `contextId` and sends it on later turns, so successive
  invocations continue one remote conversation (TS starts a fresh context every turn);
  `context_id()` exposes it.
- A protocol or transport failure is `Error::Model` with the `JsonRpcError`/`reqwest::Error` kept
  as its source, so the JSON-RPC code survives (TS throws with `{ cause }`).
- `InvokeInput::{InterruptResponses, CheckpointResume}` are rejected: a remote agent cannot resume
  a local interrupt or checkpoint. Non-text content is rejected with TS's message.
- `as_tool` is additive (TS has none) and mirrors `Agent::as_tool`'s defaults and `prompt` schema
  without interrupt propagation, which a remote agent cannot do.
- The vended tool is a `Tool` struct rather than a `tool()` factory call; its endpoints live in a
  sorted `BTreeMap`, so the description's endpoint list is deterministic, and failures come back as
  tool-error results with TS's wording preserved.
