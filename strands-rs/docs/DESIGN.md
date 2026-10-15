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

## Multi-agent

`strands_agents::multiagent` ports `strands-ts/src/multiagent/`. Increment 1 landed the shared
state types, the `MultiAgent` trait with its invocation types, and the hook/stream events; Graph
and Swarm code against the signatures below.

### `multiagent::state`

```rust
pub enum Status { Pending, Executing, Completed, Failed, Cancelled, Interrupted }   // serde SCREAMING
pub enum ResultStatus { Completed, Failed, Cancelled, Interrupted }
impl From<ResultStatus> for Status;
impl Status { pub fn as_result(self) -> Option<ResultStatus>; }

pub struct NodeResult {                      // serde: {"type":"nodeResult", camelCase}
    pub node_id: String, pub status: ResultStatus, pub duration: u64,
    pub content: Vec<ContentBlock>, pub error: Option<String>,
    pub structured_output: Option<JsonValue>, pub usage: Option<Usage>,
    pub interrupts: Vec<Interrupt>,
}
impl NodeResult {
    pub fn new(node_id: impl Into<String>, status: ResultStatus, duration: u64) -> Self;
    pub fn with_content(self, Vec<ContentBlock>) -> Self;
    pub fn with_error(self, impl Into<String>) -> Self;
    pub fn with_structured_output(self, JsonValue) -> Self;
    pub fn with_usage(self, Usage) -> Self;
    pub fn with_interrupts(self, Vec<Interrupt>) -> Self;
    pub fn merge(self, NodeResultUpdate) -> Self;
}
#[non_exhaustive]
pub struct NodeResultUpdate {                // TS NodeResultUpdate; all fields Option
    pub status: Option<ResultStatus>, pub content: Option<Vec<ContentBlock>>,
    pub error: Option<String>, pub structured_output: Option<JsonValue>,
    pub usage: Option<Usage>, pub interrupts: Option<Vec<Interrupt>>,
}
impl NodeResultUpdate {                      // #[non_exhaustive]: build it with the setters
    pub fn with_status(self, ResultStatus) -> Self;
    pub fn with_content(self, Vec<ContentBlock>) -> Self;
    pub fn with_error(self, impl Into<String>) -> Self;
    pub fn with_structured_output(self, JsonValue) -> Self;
    pub fn with_usage(self, Usage) -> Self;
    pub fn with_interrupts(self, Vec<Interrupt>) -> Self;
}

pub struct NodeState {                       // serde: camelCase, no `type` tag (TS omits it)
    pub status: Status, pub terminus: bool, pub start_time: u64,
    pub results: Vec<NodeResult>, pub interrupts: Vec<Interrupt>,
    pub interrupted_snapshot: Option<Snapshot>,
}
impl NodeState { pub fn new() -> Self; pub fn content(&self) -> &[ContentBlock]; }

pub struct MultiAgentResult {                // serde: {"type":"multiAgentResult", camelCase}
    pub status: ResultStatus, pub results: Vec<NodeResult>, pub content: Vec<ContentBlock>,
    pub duration: u64, pub usage: Usage, pub error: Option<String>, pub interrupts: Vec<Interrupt>,
}
impl MultiAgentResult {
    pub fn new(results: Vec<NodeResult>, duration: u64) -> Self;   // derives status + usage + interrupts
    pub fn with_status(self, ResultStatus) -> Self;
    pub fn with_content(self, Vec<ContentBlock>) -> Self;
    pub fn with_error(self, impl Into<String>) -> Self;
    pub fn with_interrupts(self, Vec<Interrupt>) -> Self;
}
impl Display for MultiAgentResult;           // joined text blocks of `content`

pub struct MultiAgentState {                 // serde: camelCase, `app` flattens StateStore JSON
    pub start_time: u64, pub steps: u64, pub results: Vec<NodeResult>,
    pub app: StateStore, pub pending_input: Option<MultiAgentInput>,   // serialized as `_pendingInput`
}
impl Clone for MultiAgentState;              // manual: detaches `app` via StateStore::from_map(get_all())
impl MultiAgentState {
    pub fn new(node_ids: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn node(&self, id: &str) -> Option<&NodeState>;
    pub fn node_mut(&mut self, id: &str) -> Option<&mut NodeState>;
    pub fn node_or_insert(&mut self, id: impl Into<String>) -> &mut NodeState;
    pub fn nodes(&self) -> &IndexMap<String, NodeState>;   // insertion order, like the TS Map
}

pub struct MultiAgentStateHandle(Arc<Mutex<MultiAgentState>>);   // Clone; events carry this
impl MultiAgentStateHandle {
    pub fn new(MultiAgentState) -> Self;
    pub fn with<R>(&self, impl FnOnce(&MultiAgentState) -> R) -> R;       // NOT reentrant
    pub fn with_mut<R>(&self, impl FnOnce(&mut MultiAgentState) -> R) -> R;   // NOT reentrant
    pub fn snapshot(&self) -> MultiAgentState;   // copy-out; lock released before you read it
    pub fn node(&self, id: &str) -> Option<NodeState>;
    pub fn results(&self) -> Vec<NodeResult>;
    pub fn steps(&self) -> u64;
}

pub fn resolve_status(&[NodeResult]) -> ResultStatus;   // FAILED > INTERRUPTED > CANCELLED > COMPLETED
pub fn aggregate_node_usage(&[NodeResult]) -> Usage;
```

### `multiagent::multiagent`

```rust
pub enum MultiAgentInput { None, Text(String), Blocks(Vec<ContentBlock>), InterruptResponses(Vec<InterruptResponseContent>) }
// From<&str>, From<String>, From<Vec<ContentBlock>>, From<Vec<InterruptResponseContent>>,
// TryFrom<InvokeInput> (rejects Messages / CheckpointResume with UNSUPPORTED_MULTIAGENT_INPUT_MESSAGE)
impl MultiAgentInput { pub fn resume_responses(&self) -> Option<&[InterruptResponseContent]>; }

#[non_exhaustive]
pub struct MultiAgentInvokeOptions { pub invocation_state: Option<InvocationState>, pub cancel: Option<CancellationToken> }
// with_invocation_state, with_cancel — mirrors agent::InvokeOptions

#[async_trait]
pub trait MultiAgent: Send + Sync {          // object-safe; used as Arc<dyn MultiAgent>
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> Option<&str> { None }
    fn hooks(&self) -> &HookRegistry;
    async fn invoke_with(&self, MultiAgentInput, MultiAgentInvokeOptions) -> Result<MultiAgentResult>;
    fn stream_with(&self, MultiAgentInput, MultiAgentInvokeOptions) -> MultiAgentStream;
    fn as_any(&self) -> &dyn Any;            // implement as `{ self }`; TS `instanceof` stand-in
}
pub trait MultiAgentExt: MultiAgent {        // blanket impl, incl. dyn MultiAgent
    fn invoke(&self, impl Into<MultiAgentInput>) -> BoxFuture<'_, Result<MultiAgentResult>>;
    fn stream(&self, impl Into<MultiAgentInput>) -> MultiAgentStream;
}
// BINDING for Graph/Swarm: every concrete orchestrator MUST also expose inherent
// `invoke`/`invoke_with`/`stream`/`stream_with` (like `Agent`, `event_loop.rs:361-430`) so users
// never need `use MultiAgentExt`; the ext trait exists for `dyn MultiAgent` (nested nodes).

#[must_use]
pub struct MultiAgentStream;                 // newtype over BoxStream<'static, Result<MultiAgentStreamEvent>>
impl MultiAgentStream { pub fn new(BoxStream<..>) -> Self; pub fn into_inner(self) -> BoxStream<..>; }
impl Stream for MultiAgentStream { type Item = Result<MultiAgentStreamEvent>; }

pub fn group_interrupt_responses_by_node(&[InterruptResponseContent], &MultiAgentState)
    -> Result<IndexMap<String, Vec<InterruptResponseContent>>>;   // response order, as in TS
pub fn drop_stale_interrupted_result(node_id: &str, &mut MultiAgentState);
pub fn record_hook_interrupt(node_id: &str, &mut NodeState) -> NodeResult;
pub fn interrupted_result(&MultiAgentState, content: Vec<ContentBlock>) -> MultiAgentResult;
pub fn apply_orchestrator_hook_responses(&mut NodeState, Vec<InterruptResponseContent>)
    -> Vec<InterruptResponseContent>;         // returns the responses bound for the child agent
```

### `multiagent::events`

Ten hook events, each `impl HookEvent`, each carrying `state: MultiAgentStateHandle` and
`invocation_state: InvocationState`; orchestrator-scoped ones also carry
`orchestrator: OrchestratorRef` (`= Arc<dyn MultiAgent>`):
`MultiAgentInitializedEvent`, `BeforeMultiAgentInvocationEvent`, `AfterMultiAgentInvocationEvent`
(reverse), `BeforeNodeCallEvent` (`cancel: Cancel`,
`interrupt<T: DeserializeOwned>(InterruptParams) -> Result<T>`, id from
`interrupt::before_node_call_interrupt_id`), `AfterNodeCallEvent` (reverse, `error: Option<String>`),
`NodeStreamUpdateEvent` (`inner: NodeInnerEvent`, plus
`wrapping(&dyn Node, &MultiAgentStateHandle, &NodeInputOptions, NodeInnerEvent) -> Self`),
`NodeResultEvent`, `MultiAgentHandoffEvent`, `NodeCancelEvent`, `MultiAgentResultEvent`.

```rust
pub enum NodeInnerEvent { Agent(Box<AgentStreamEvent>), MultiAgent(Box<MultiAgentStreamEvent>), Custom(JsonValue) }
impl NodeInnerEvent { pub fn source(&self) -> &'static str; }   // "agent" | "multiAgent" | "custom"

#[non_exhaustive]
pub enum MultiAgentStreamEvent { BeforeMultiAgentInvocation(..), AfterMultiAgentInvocation(..),
    BeforeNodeCall(..), AfterNodeCall(..), NodeStreamUpdate(..), NodeResult(..), NodeCancel(..),
    MultiAgentHandoff(..), MultiAgentResult(..) }
impl MultiAgentStreamEvent {
    pub fn type_name(&self) -> &'static str;   // TS discriminators, e.g. "nodeResultEvent"
    pub fn node_id(&self) -> Option<&str>;
    pub fn result(&self) -> Option<&MultiAgentResult>;
}

pub enum NodeType { Agent, MultiAgent, Custom(String) }   // serde "agentNode"/"multiAgentNode"/<name>
```

### `multiagent::nodes`

```rust
pub type NodeSink = tokio::sync::mpsc::Sender<Result<MultiAgentStreamEvent>>;

#[non_exhaustive]
pub struct NodeConfig { pub description: Option<String> }
#[non_exhaustive]
pub struct NodeInputOptions {                     // TS NodeInputOptions
    pub structured_output_schema: Option<JsonValue>,
    pub invocation_state: InvocationState,        // shared by reference across nodes
    pub cancel: Option<CancellationToken>,
}
impl NodeInputOptions { with_structured_output_schema, with_invocation_state, with_cancel }

#[async_trait]
pub trait Node: Send + Sync {
    fn id(&self) -> &str;
    fn node_type(&self) -> NodeType;
    fn config(&self) -> &NodeConfig;
    fn timeout(&self) -> Option<Duration> { None }   // the orchestrator honors it per node
    async fn handle(&self, input: MultiAgentInput, state: &MultiAgentStateHandle,
                    options: &NodeInputOptions, events: &NodeSink) -> Result<NodeResultUpdate>;
}

/// The shared TS `Node.stream` wrapper.
pub async fn run_node(node: &dyn Node, input: MultiAgentInput, state: &MultiAgentStateHandle,
                      options: &NodeInputOptions, events: &NodeSink) -> NodeResult;

#[non_exhaustive]
pub struct AgentNodeOptions { pub agent: Agent, pub description: Option<String>,
                              pub timeout: Option<Duration>, pub preserve_context: bool }
impl AgentNodeOptions { pub fn new(Agent) -> Self; with_description, with_timeout, with_preserve_context }
pub struct AgentNode;                        // Debug (manual); reloads a paused agent in `handle`
impl AgentNode {
    pub fn new(options: impl Into<AgentNodeOptions>) -> Result<Self>;   // rejects a zero timeout
    pub fn agent(&self) -> &Agent;
    pub fn preserve_context(&self) -> bool;
}
// `timeout()` is the `Node` trait method, not an inherent one — import `Node` to call it.

#[non_exhaustive]
pub struct MultiAgentNodeOptions { pub orchestrator: Arc<dyn MultiAgent>, pub description: Option<String> }
impl MultiAgentNodeOptions { pub fn new(Arc<dyn MultiAgent>) -> Self; with_description }
pub struct MultiAgentNode;
impl MultiAgentNode {
    pub fn new(options: impl Into<MultiAgentNodeOptions>) -> Self;
    pub fn orchestrator(&self) -> &Arc<dyn MultiAgent>;
}

pub enum NodeDefinition { Agent(AgentNodeOptions), MultiAgent(MultiAgentNodeOptions), Node(Arc<dyn Node>) }
impl NodeDefinition { pub fn into_node(self) -> Result<Arc<dyn Node>>; }
// From<Agent>, From<AgentNodeOptions>, From<Arc<dyn MultiAgent>>, From<MultiAgentNodeOptions>, From<Arc<dyn Node>>
```

### `multiagent::edge`

```rust
pub type EdgeHandler = Arc<dyn Fn(&MultiAgentStateHandle) -> BoxFuture<'static, bool> + Send + Sync>;
pub fn edge_handler(predicate: impl Fn(&MultiAgentState) -> bool + Send + Sync + 'static) -> EdgeHandler;

pub struct Edge { pub source: Arc<dyn Node>, pub target: Arc<dyn Node>, pub handler: Option<EdgeHandler> }
impl Edge {                                  // Debug (manual): node ids + whether it is conditional
    pub fn new(Arc<dyn Node>, Arc<dyn Node>) -> Self;
    pub fn with_handler(self, EdgeHandler) -> Self;
    pub async fn should_traverse(&self, state: &MultiAgentStateHandle) -> bool;   // true when no handler
}

#[non_exhaustive]
pub struct EdgeDefinition { pub source: String, pub target: String, pub handler: Option<EdgeHandler> }
impl EdgeDefinition { pub fn new(impl Into<String>, impl Into<String>) -> Self; with_handler }
impl<S: Into<String>, T: Into<String>> From<(S, T)> for EdgeDefinition;   // the TS `[source, target]` tuple
```

### `multiagent::plugins`

```rust
#[async_trait]
pub trait MultiAgentPlugin: Send + Sync {
    fn name(&self) -> &str;
    async fn init_multi_agent(&self, orchestrator: &OrchestratorRef) -> Result<()>;   // retainable handle
}
impl<P: MultiAgentPlugin + ?Sized> MultiAgentPlugin for Arc<P>;

pub struct MultiAgentPluginRegistry;    // mirrors plugins::PluginRegistry exactly
impl MultiAgentPluginRegistry {
    pub fn new(plugins: Vec<Arc<dyn MultiAgentPlugin>>) -> Self;
    pub fn add(&self, plugin: Arc<dyn MultiAgentPlugin>);
    pub fn get(&self, name: &str) -> Option<Arc<dyn MultiAgentPlugin>>;
    pub async fn initialize(&self, orchestrator: &OrchestratorRef) -> Result<()>;   // fail-closed, once
}
```

### `multiagent::snapshot`

```rust
#[non_exhaustive]
pub struct TakeMultiAgentSnapshotOptions { pub app_data: Map<String, JsonValue> }
impl TakeMultiAgentSnapshotOptions { pub fn with_app_data(self, Map<String, JsonValue>) -> Self }

pub fn take_snapshot(orchestrator: &dyn MultiAgent, state: Option<&MultiAgentStateHandle>,
                     options: TakeMultiAgentSnapshotOptions) -> Result<Snapshot>;   // session::types::Snapshot
pub fn load_snapshot(orchestrator: &dyn MultiAgent, snapshot: &Snapshot,
                     state: Option<&MultiAgentStateHandle>) -> Result<()>;
```

### `multiagent::spec`

```rust
pub type AgentBuilder = Arc<dyn Fn(AgentSpec) -> Result<Agent> + Send + Sync>;

pub enum SpecModel { Id(String), Model(Arc<dyn Model>) }   // PartialEq by id / pointer identity

pub struct ChoiceOption<T = String> { pub name: String, pub value: Option<T>, pub description: String }
impl<T> ChoiceOption<T> {
    pub fn new(name: impl Into<String>) -> Self;            // value = None, i.e. TS UNSET
    pub fn with_value(self, value: impl Into<T>) -> Self;
    pub fn with_description(self, description: impl Into<String>) -> Self;
}
impl<T> From<&str> for ChoiceOption<T>;
impl<T> From<String> for ChoiceOption<T>;

pub struct Choice<T = String> { pub options: Vec<ChoiceOption<T>>, pub multiple: bool }
impl<T> Choice<T> {
    pub fn new<O: Into<ChoiceOption<T>>>(options: impl IntoIterator<Item = O>) -> Self;
    pub fn multiple(self) -> Self;
}
impl<T: Clone + From<String>> Choice<T> {
    pub fn normalized(&self) -> Vec<ChoiceOption<T>>;       // fills None values from the name
    pub fn to_schema_property(&self, description: &str) -> Map<String, JsonValue>;
    pub fn value_for(&self, name: &str) -> T;               // passthrough when unknown
}

pub enum ScalarAxis<T = String> { Fixed(Option<T>), Inherit, Open, Choice(Choice<T>) }   // Default = Inherit
impl<T> ScalarAxis<T> { pub fn fixed(value: impl Into<T>) -> Self }

pub enum ListAxis<T = String> { Fixed(Option<Vec<T>>), Inherit, Choice(Choice<T>) }      // Default = Inherit
impl<T> ListAxis<T> { pub fn fixed(values: impl IntoIterator<Item = impl Into<T>>) -> Self }

pub struct Preset { pub instructions: Option<String>, pub tools: Option<Vec<String>>,
                    pub model: Option<SpecModel>, pub description: String }              // Default + with_*

pub struct AgentSpec { pub name: Option<String>, pub agent_type: Option<String>,
                       pub instructions: Option<String>, pub tools: Option<Vec<String>>,
                       pub mcp_servers: Option<Vec<String>>, pub model: Option<SpecModel> }  // None = inherit

pub struct ResolveSpecAxes { pub presets: IndexMap<String, Preset>, pub default_preset: Option<String>,
                             pub instructions: ScalarAxis<String>, pub tools: ListAxis<String>,
                             pub mcp_servers: ListAxis<String>, pub model: ScalarAxis<SpecModel> }

pub fn resolve_spec(model_input: &Map<String, JsonValue>, axes: &ResolveSpecAxes) -> Result<AgentSpec>;
```

Precedence per axis: a model-supplied argument, then the preset's value, then the axis default.
An omitted `agent_type` falls back to `default_preset`; ad-hoc `instructions` suppress that
fallback. A `Choice` list axis drops values that are not options; `Inherit` resolves to `None`.

### Decision log

- **`ResultStatus` is its own enum**, not a runtime-checked subset of `Status`, so a `NodeResult`
  cannot hold `PENDING`/`EXECUTING`. `From<ResultStatus> for Status` and `Status::as_result` bridge
  the two; serde strings are identical to TS.
- **`error: Option<String>`**, not a boxed error: TS `serializeError` serialises an `Error` to its
  message, so the message is the whole wire shape and the only thing that survives a round-trip.
- **`interrupts: Vec<Interrupt>` with `skip_serializing_if = "Vec::is_empty"`** instead of
  `Option<Vec<_>>`, matching `AgentResult.interrupts`. The JSON is identical to TS (omitted when
  empty); callers get one shape instead of two.
- **`structured_output` round-trips an explicit `null`** via a `deserialize_some` helper — plain
  `Option<JsonValue>` would collapse a present `null` to absent, which TS distinguishes.
- **`NodeState` / `MultiAgentState` serialise without a `type` tag.** The TS classes declare
  `type` but their `stateToJSON` omits it; only `NodeResult` and `MultiAgentResult` (which have a
  real `toJSON`) carry the tag.
- **`MultiAgentState.nodes` is an `IndexMap`** (`indexmap` is already a workspace dependency, with
  serde): insertion order, matching the TS `Map`. The order *is* observable — `interrupted_result`
  and `group_interrupt_responses_by_node` iterate it — so a `BTreeMap`'s id ordering would diverge
  from TS. `group_interrupt_responses_by_node` returns an `IndexMap` for the same reason.
- **`MultiAgentStateHandle` is the handle events carry**, mirroring `InterruptStateHandle`: hook
  callbacks get `&mut Event`, so sharing the state has to go through an `Arc<Mutex<..>>` rather
  than a borrow. `MultiAgentState` itself stays a plain struct.
- **`MultiAgentInvokeOptions.cancel`**, not `cancellation_token`, to match
  `InvokeOptions::cancel`/`with_cancel` exactly (crate consistency over the plan's sketch name).
- **`invoke`/`stream` live on `MultiAgentExt`**, a blanket extension trait: generic
  `impl Into<..>` methods are not object-safe, and `dyn MultiAgent` must stay usable as a node.
  **Binding on Graph/Swarm:** each concrete orchestrator also exposes *inherent*
  `invoke`/`invoke_with`/`stream`/`stream_with` (as `Agent` does, `event_loop.rs:361-430`), so the
  common path needs no trait import; `MultiAgentExt` is for `dyn MultiAgent` (nested nodes).
- **Multi-agent events do not implement `Serialize`** (TS events have `toJSON`): they hold live
  handles (`Arc<dyn MultiAgent>`, the state handle). `MultiAgentStreamEvent::type_name` provides
  the TS `type` discriminator, which is what the TS tests assert on.
- **`NodeInnerEvent::Custom(JsonValue)`**: TS wraps an arbitrary `StreamEvent` from a non-`Agent`
  runnable; the Rust port has no such common event type, so custom nodes forward opaque JSON.
- **`NodeType` lives in `multiagent::nodes`** (increment 1 defines only the enum; the `Node` trait
  and the concrete node types land there next) because the events reference it.
- **`Node::handle` writes to a `NodeSink` and returns `Result<NodeResultUpdate>`** instead of
  being an async generator that both yields and returns: the crate already streams through an
  `mpsc` sink (`agent::event_loop`), and a channel keeps `Node` object-safe. It keeps the four TS
  parameters (`input`, `state`, `options`, plus the sink) rather than a `NodeContext` bundle — the
  bundle would hide the orchestrator's own channel behind a type it has to construct per node, and
  the three parts have different lifetimes to the orchestrator.
- **`NodeSink` stays a plain `mpsc::Sender` alias, not a newtype with `emit`.** Graph/Swarm create
  and own the channel, and the only thing a newtype would add is field-filling, which
  `NodeStreamUpdateEvent::wrapping(node, state, options, inner)` now does without changing the type.
- **`MultiAgent::as_any` is a required method** (`fn as_any(&self) -> &dyn Any { self }` in every
  implementor): hooks and plugins hold `Arc<dyn MultiAgent>` and TS reaches concrete state through
  `instanceof`, whose Rust equivalent is a downcast. Required rather than provided because a
  default cannot return `self` as `Any` (it needs `Self: Sized + 'static`), and adding it after
  Graph/Swarm ship would be a breaking change.
- **Async trait methods use `#[async_trait]`** (`MultiAgent::invoke_with`, `Node::handle`), the
  crate-wide convention (`Plugin`, `Tool`, `Model`, `MultiAgentPlugin`). `stream_with` stays a
  plain `fn` returning `MultiAgentStream`, and `MultiAgentExt`'s generic conveniences stay
  `BoxFuture` because `#[async_trait]` cannot take `impl Into<..>` arguments.
- **`MultiAgentPlugin::init_multi_agent` takes `&OrchestratorRef`**, not `&dyn MultiAgent`, so a
  plugin can clone the `Arc` into the hook callbacks it registers (they outlive the call), matching
  `Plugin::init_agent`, which gets the clonable `&Agent` handle.
- **`MultiAgentStateHandle` has copy-out readers** (`snapshot`, `node`, `results`, `steps`)
  alongside `with`/`with_mut`: the closures run under a non-reentrant lock, so a hook that reads
  the state again from inside one deadlocks. `with_mut` remains for compound mutation.
  `MultiAgentState: Clone` is manual because `StateStore` is not `Clone`; the clone detaches the
  store (`StateStore::from_map(get_all())`) so a snapshot never aliases the live state.
- **The shared TS `Node.stream` wrapper is the free fn `run_node`**, not a provided trait method:
  it needs `async` over `&dyn Node`, and a free fn keeps the trait object-safe without boxing the
  wrapper per implementation. It does not fire the before/after-node hooks — matching TS, where
  Graph and Swarm own those.
- **`AgentNode` wraps the concrete `Agent`**, since the Rust port has no `InvokableAgent`
  interface. The TS "`preserveContext` requires an `Agent` instance" validation (and its test) is
  therefore unnecessary; snapshot/restore always works.
- **No `NodeInputOptions.buffer_output`**: TS swaps the agent's private printer to buffer
  concurrent stdout. The Rust `Agent` exposes no swappable printer, so the knob (and its two TS
  tests) is dropped rather than guessed at — revisit if/when printers are injectable.
- **`AgentNode` timeout is `Option<Duration>`**, with `Duration::MAX` as the TS `Infinity`
  opt-out and `Duration::ZERO` rejected (TS rejects `timeout < 1` ms, same message). `timeout()`
  is a provided `Node` method (default `None`) so an orchestrator can honor it through
  `Arc<dyn Node>`; `AgentNode` overrides it instead of carrying an inherent accessor.
- **`AgentNode::handle` reloads `NodeState::interrupted_snapshot` before streaming** (TS
  `nodes.ts`), independently of `preserve_context`: the post-run restore puts the agent back to its
  pre-run state, so without the reload a resumed run would re-enter the node with no history.
  Note the agent snapshot itself does not yet carry interrupt state (`agent/snapshot.rs` captures
  `interrupts` as `null`), so resume across a *process* boundary still needs that gap closed.
- **`EdgeHandler` takes the `MultiAgentStateHandle` and returns `BoxFuture<'static, bool>`** so
  conditions can be async as in TS *and* an orchestrator can actually await one: a
  `&MultiAgentState` only exists inside `MultiAgentStateHandle::with`, whose borrow cannot escape
  the (synchronous) closure, so `state.with(|s| edge.should_traverse(s)).await` does not compile.
  `edge_handler` wraps the common synchronous predicate and does the `with` internally.
- **`EdgeDefinition` is a struct, not an enum**: TS's `[source, target] | EdgeOptions` union
  collapses into one struct with an optional handler, reached by `From<(S, T)>` for the tuple case.
- **`take_snapshot`/`load_snapshot` are free fns over `&dyn MultiAgent`** (TS takes
  `Graph | Swarm`) and take the state as `Option<&MultiAgentStateHandle>`; `take_snapshot` returns
  `Result` because serializing the state can fail in Rust.
- **Spec axes are two enums, not four classes**: TS's `Fixed | Inherit | Open | Choice` class
  union becomes `ScalarAxis<T>` (scalar axes: instructions, model) and `ListAxis<T>` (list axes:
  tools, mcp_servers), each with `Fixed`/`Inherit`/`Choice` variants and `Open` only on
  `ScalarAxis`. The enums encode per-axis policy at the type level — `ListAxis` has no `Open`,
  matching the TS `ResolveSpecAxes` field types — and both default to `Inherit`.
- **TS `Option` is `ChoiceOption<T>`** (`Option` is `std::option::Option` in Rust), generic over
  the axis value with `T = String`. `value: Option<T>` replaces the `UNSET` sentinel, so TS's
  "explicit `null` is not overwritten" case has no Rust analogue (Rust values are not nullable);
  the test asserts an explicitly set empty value instead.
- **`Choice::new` takes `impl IntoIterator<Item = impl Into<ChoiceOption<T>>>`**: a Rust array
  cannot mix `&str` and `ChoiceOption`, so bare names convert via `From<&str>`/`From<String>` and
  a heterogeneous list is written as `[ChoiceOption::from("a"), ChoiceOption::new("b").with_value(v)]`.
- **`SpecModel` replaces TS's `Model | ModelRouter | string`**: an `Id(String)` the builder
  resolves, or an `Arc<dyn Model>` handed over as-is. It compares by id or pointer identity,
  mirroring TS reference equality (no `ModelRouter` in the Rust port yet).
- **`resolve_spec` takes a `serde_json::Map` and returns `Result<AgentSpec>`**: the model's
  arguments arrive as JSON from a tool call, and an unknown `agent_type` is an
  `Error::ToolValidation` rather than a thrown `Error`. Non-string values for a string-typed axis
  are ignored instead of passed through (TS `Open` returns whatever the model supplied).
- **`AgentBuilder = Arc<dyn Fn(AgentSpec) -> Result<Agent> + Send + Sync>`**: shared and
  `Send + Sync` because orchestrators build children from concurrent tasks, and fallible because
  Rust agent construction can fail (TS's builder returns an `Agent` directly).
- **`Fixed`'s `Object.freeze` has no port**: axis values are immutable by ownership in Rust, so
  the TS "is frozen" test is dropped rather than translated.
- **`_defaultBuilder` is not ported in this increment**: it needs parent-agent internals the Rust
  `Agent` does not expose yet (MCP client inheritance, sandbox, printer, tracer trace-attributes).
  It belongs with the vended multi-agent tools that call it.
