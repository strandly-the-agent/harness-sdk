# A2A (`strands-ts/src/a2a/` → `crates/strands-agents/src/a2a/`)

Behind the `a2a` cargo feature. Protocol types are in-tree (TS depends on `@a2a-js/sdk`).

## API map

| TS | Rust |
| --- | --- |
| `a2a/types` (from `@a2a-js/sdk`) | `a2a::types::{Message, Part, Task, TaskStatus, TaskState, Artifact, TaskStatusUpdateEvent, TaskArtifactUpdateEvent, A2AEventData, AgentCard, AgentSkill, AgentCapabilities, AgentProvider, MessageSendParams, TaskQueryParams, TaskIdParams, JsonRpcRequest, JsonRpcResponse, JsonRpcError, error_codes}` |
| `partsToContentBlocks` / `contentBlocksToParts` | `a2a::parts_to_content_blocks` / `a2a::content_blocks_to_parts` |
| `A2AStreamUpdateEvent`, `A2AResultEvent` | `a2a::{A2AStreamUpdateEvent, A2AResultEvent, A2AStreamEvent}` |
| `InvokableAgent` (`types/agent.ts`) | `agent::InvokableAgent` |
| `AsyncLock` (`a2a/async-lock.ts`) | `tokio::sync::Mutex` (no port; see divergences) |
| `AgentFactory` | `a2a::AgentFactory = Arc<dyn Fn(&str) -> Agent + Send + Sync>` |
| `DEFAULT_MAX_CONTEXTS` | `a2a::DEFAULT_MAX_CONTEXTS` |
| `RequestContext` (SDK) | `a2a::RequestContext::{new, for_message}` |
| `ExecutionEventBus` (SDK) | `a2a::EventBus::{channel, publish}` |
| `new A2AExecutor({ agentFactory })` | `A2AExecutor::with_factory(f)` |
| `new A2AExecutor({ agent })` (deprecated) | `A2AExecutor::new(agent) -> Result<Self>` |
| `{ maxContexts }` | `.with_max_contexts(n)` |
| `executor.execute(ctx, bus)` | `A2AExecutor::execute(&self, &RequestContext, &EventBus) -> Result<()>` |
| `executor.cancelTask(id, bus)` | `A2AExecutor::cancel_task(&self, &str, &EventBus) -> Result<()>` |
| `invocationState.a2aRequestContext` | `a2a::A2A_REQUEST_CONTEXT_KEY` |
| `InMemoryTaskStore` + `DefaultRequestHandler` aggregation (SDK) | `a2a::TaskStore::{new, get, save, apply}` |
| `new A2AServer({ ... })` / `new A2AExpressServer({ ... })` | `A2AServer::builder(name)` → `.agent(a)` / `.executor(e)` / `.description` / `.http_url` / `.version` / `.skills` / `.agent_card` / `.serve_at_root` / `.host` / `.port` / `.listener` / `.build() -> Result<A2AServer>` |
| `server.agentCard` | `A2AServer::agent_card() -> &AgentCard`, `ServerHandle::agent_card()` |
| `{ taskStore }` config option | `A2AServer::task_store() -> &Arc<TaskStore>` (no pluggable store; see divergences) |
| `server.createMiddleware()` (express `Router`) | `A2AServer::router() -> axum::Router` |
| `server.serve({ signal })` | `A2AServer::serve() -> Result<ServerHandle>`; `ServerHandle::{local_addr, agent_card, shutdown}` |
| `agentCardHandler` at `/.well-known/agent-card.json` | `GET types::AGENT_CARD_PATH` and legacy `types::PREV_AGENT_CARD_PATH` |
| `jsonRpcHandler` at `/` | `POST /` — `message/send`, `message/stream` (SSE), `tasks/get`, `tasks/cancel` |
| `UserBuilder.noAuthentication` | **not ported** — wrap `router()` in your own `axum` middleware |

| `new A2AAgent({ url, agentCardPath, id, name, description, clientFactory })` | `A2AAgent::builder(url)` → `.agent_card_path` / `.agent_card` / `.http_client` / `.timeout` / `.id` / `.name` / `.description` / `.build() -> A2AAgent` |
| `agent.id` / `agent.name` / `agent.description` | `A2AAgent::{id, name, description, url}` |
| `agent.getAgentCard()` | `A2AAgent::agent_card() -> Result<&AgentCard>` (cached) |
| `agent.invoke(args, options)` | `A2AAgent::{invoke, invoke_with}` |
| `agent.stream(args, options)` | `A2AAgent::{stream, stream_with} -> impl Stream<Item = Result<A2AStreamEvent>>` |
| `DEFAULT_AGENT_CARD_PATH` (`'/.well-known/agent-card.json'`) | `a2a::DEFAULT_AGENT_CARD_PATH` |
| *(no TS equivalent)* | `A2AAgent::context_id() -> Option<String>`, `A2AAgent::as_tool(AgentAsToolOptions) -> ToolRef` |
| `makeA2AClient({ name, description, allowedEndpoints })` | `A2AClientTool::new(endpoints) -> Result<Self>` / `A2AClientTool::builder(endpoints)` → `.name` / `.description` / `.endpoint_with_client` / `.build() -> Result<A2AClientTool>` |
| `DEFAULT_A2A_CLIENT_DESCRIPTION` | `tools::vended::DEFAULT_A2A_CLIENT_DESCRIPTION` (verbatim), `A2A_CLIENT_TOOL_NAME` |
| `[url, ClientFactory]` endpoint tuple | `.endpoint_with_client(url, reqwest::Client)` (auth lives on the HTTP client) |
| `a2aClientInputSchema` (zod) | the tool's `input_schema` JSON (same names, enum and descriptions) |

## Test map

| TS `it` | Rust test |
| --- | --- |
| adapters: `converts text parts to TextBlocks` | `a2a::adapters::converts_text_parts_to_text_blocks` |
| adapters: `converts data parts to TextBlock with JSON` | `converts_data_parts_to_a_text_block_of_pretty_json` |
| adapters: `handles mixed part types` | `handles_mixed_part_types` |
| adapters: `returns empty array for empty input` | `returns_no_blocks_for_no_parts` |
| adapters: `converts text blocks to text parts` | `converts_text_blocks_to_text_parts` |
| adapters: `handles mixed text and media blocks` | `handles_mixed_text_and_media_blocks` |
| adapters: `skips unsupported block types` | `skips_unsupported_block_types`, `skips_a_file_part_whose_bytes_are_not_base64` |
| events: `creates instance with correct properties` (×2) | `a_stream_update_event_carries_the_protocol_event`, `a_result_event_carries_the_agent_result` |
| events: `serializes` / `only excludes expected fields` (×2) | `a_stream_update_event_serializes_with_its_type_and_event`, `a_result_event_serializes_with_its_type_and_result` |
| executor: `streams text deltas as artifact chunks and publishes completed status` | `a2a::executor::streams_text_deltas_as_artifact_chunks_and_publishes_completed_status` |
| executor: `sets append to true for subsequent chunks after the first` | `sets_append_to_true_for_subsequent_chunks_after_the_first` |
| executor: `converts A2A parts to content blocks and passes to stream` | `converts_a2a_parts_to_content_blocks_and_passes_them_to_the_agent` |
| executor: `forwards the A2A request context to the agent via invocationState` | `forwards_the_a2a_request_context_to_the_agent_via_invocation_state` |
| executor: `re-throws when agent throws, publishing only the initial task event` | `returns_the_agent_error_after_publishing_only_the_initial_task_event` |
| executor: `publishes image content blocks as separate file artifacts` | **not ported** — a mock `Model` cannot emit a non-text content block through the model stream, so the branch is unreachable without stubbing `Agent::stream`. The conversion itself is covered by `adapters::converts_media_blocks_to_file_parts`. |
| executor: `throws A2AError.invalidRequest when parts produce no content blocks` | `rejects_a_message_whose_parts_produce_no_content_blocks` |
| executor: `throws A2AError.unsupportedOperation` (cancelTask) | `cancelling_a_task_is_unsupported` |
| executor: `throws when neither agent nor agentFactory is provided` | **N/A** — separate constructors make it a compile error |
| executor: `throws when both agent and agentFactory are provided` | **N/A** — same |
| executor: `throws when maxContexts is less than 1` | `a_zero_max_contexts_is_clamped_to_one` (clamps instead of erroring) |
| executor: `builds one agent per context and reuses it within a context` | `builds_one_agent_per_context_and_reuses_it_within_a_context` |
| executor: `isolates history across contexts and continues it within one` | `isolates_history_across_contexts_and_continues_it_within_one` |
| executor: `evicts the least-recently-used context beyond maxContexts` | `evicts_the_least_recently_used_context_beyond_max_contexts` |
| executor: `isolates history across contexts via snapshot swapping` | `a_shared_agent_isolates_history_across_contexts_via_snapshot_swapping` |
| executor: `throws when a single agent has a sessionManager` | covered by `A2AExecutor::new` returning `Err`; no dedicated test yet |
| `async-lock.test.ts` (4 `it`s) | **N/A** — `tokio::sync::Mutex` replaces `AsyncLock`; not re-tested |
| server: `builds agent card with provided values` | `a2a::server::builds_agent_card_with_provided_values` |
| server: `uses default values when optional config is omitted` | `uses_default_values_when_optional_config_is_omitted` |
| server: `builds an agent card from a factory-built representative agent` | `describes_itself_with_the_agents_own_description` (agent mode; the factory is never called just for the card) |
| server: `accepts custom taskStore` | **N/A** — the store is not pluggable; `an_explicit_agent_card_overrides_the_card_settings` covers the remaining override knob |
| server: `throws when neither agent nor agentFactory is provided` | `requires_an_agent_or_an_executor` |
| server: `throws when both agent and agentFactory are provided` | **N/A** — `.agent()`/`.executor()` share one slot, last call wins |
| server (node): `builds agent card with default values` / `uses custom config values` / `uses httpUrl override when provided` | `uses_default_values_when_optional_config_is_omitted`, `builds_agent_card_with_provided_values`, `serves_the_agent_card_at_both_well_known_paths` |
| server (node): `is an instance of A2AServer` | **N/A** — one type, no subclassing |
| server (node): `returns an express router with SDK middleware` | `serves_the_agent_card_at_both_well_known_paths`, `serves_under_the_path_of_the_http_url`, `serve_at_root_ignores_the_http_url_path` (behavioural, not mock-shaped) |
| *(no TS equivalent — the SDK's `DefaultRequestHandler` is not unit-tested here)* | `message_send_returns_the_completed_task`, `message_stream_emits_the_task_artifacts_and_final_status_as_sse`, `tasks_get_returns_a_previously_served_task`, `tasks_get_truncates_the_history_to_the_requested_length`, `tasks_get_reports_an_unknown_task`, `tasks_cancel_reports_an_unknown_task`, `tasks_cancel_reports_a_finished_task_as_not_cancelable`, `reports_an_unknown_method`, `reports_invalid_params`, `reports_a_malformed_body_as_a_parse_error`, `reports_a_wrong_protocol_version_as_an_invalid_request`, `serves_on_a_provided_listener_and_stops_on_shutdown`, `derives_the_mount_path_from_the_http_url` |
| *(no TS equivalent — store aggregation)* | `a2a::task_store::*` (8 tests) |
| a2a-agent: `defaults id to the URL when not provided` | `a2a::a2a_agent::defaults_id_to_the_url_when_not_provided` |
| a2a-agent: `uses provided id from config` | `uses_provided_id_from_config` |
| a2a-agent: `uses provided name and description from config` | `uses_provided_name_and_description_from_config` |
| a2a-agent: `has undefined name and description when not provided in config` | `falls_back_to_the_url_as_the_name_before_connecting` (name is never absent in Rust) |
| a2a-agent: `populates name and description from agent card on first connection` | `populates_name_and_description_from_agent_card_on_first_connection` |
| a2a-agent: `does not overwrite config-provided name and description with agent card values` | `does_not_overwrite_config_provided_name_and_description_with_agent_card_values` |
| a2a-agent: `returns AgentResult with response text` | `invoke_returns_an_agent_result_with_the_response_text`, `a2a_e2e::a2a_agent_invokes_the_served_agent` |
| a2a-agent: `auto-connects on first invoke` | `auto_connects_on_first_invoke` |
| a2a-agent: `uses custom clientFactory when provided` | `uses_a_custom_http_client_when_provided` |
| a2a-agent: `yields A2AStreamUpdateEvent for each A2A event and A2AResultEvent at the end` | `stream_yields_update_events_for_each_a2a_event_and_a_result_event_last`, `a2a_e2e::a2a_agent_streams_protocol_events_then_a_result` |
| a2a-agent: `yields multiple A2AStreamUpdateEvents for streamed artifact chunks` | `falls_back_to_accumulated_artifact_chunks_when_the_last_event_has_no_text` (chunk accumulation) + the e2e stream test |
| a2a-agent: `yields A2AStreamUpdateEvent for Message response` | **N/A** — the in-tree server always creates a task; `A2AEventData::Message` text extraction is covered in `a2a::types` |
| a2a-agent: `builds result from status-update with completed state` | `builds_the_result_from_a_completed_status_update` |
| a2a-agent: `returns empty text when no events are received` | `builds_an_empty_result_when_no_events_are_received` |
| a2a-agent: `extracts text from Task response` / `joins multiple text parts from Task artifacts` / `joins text from multiple Task artifacts` | `joins_multiple_text_parts_from_task_artifacts` |
| a2a-agent: `falls back to Task status message when no artifacts` | `falls_back_to_the_task_status_message_when_there_are_no_artifacts` |
| a2a-agent: `returns empty text for Task with no text content` / `extracts text from Message response` | covered by `a2a::types::A2AEventData::text` tests |
| a2a-agent: `extracts text from string / Message[] / ContentBlock[] args`, `throws on non-text content` | `extracts_the_text_to_send_from_every_kind_of_input`, `rejects_non_text_content` |
| *(no TS equivalent — JSON-RPC error mapping)* | `a_json_rpc_error_event_becomes_a_model_error_keeping_the_code`, `an_event_with_neither_result_nor_error_is_an_error`, `surfaces_a_transport_failure_as_a_model_error` |
| *(no TS equivalent — context continuity, trait object, tool)* | `continues_the_same_remote_context_across_turns`, `is_invocable_behind_the_invokable_agent_trait`, `as_tool_names_and_describes_itself_from_the_remote_card`, `as_tool_overrides_the_name_and_description` |
| a2a-client: `throws when allowedEndpoints is empty` | `tools::vended::a2a_client::fails_when_allowed_endpoints_is_empty` |
| a2a-client: `uses custom name` | `uses_a_custom_name` |
| a2a-client: `default description includes endpoints` | `the_default_description_includes_the_endpoints` |
| a2a-client: `custom description overrides the default` | `a_custom_description_overrides_the_default` |
| a2a-client: `rejects an endpoint not in the allowlist` / `includes permitted endpoints in the rejection message` | `rejects_an_endpoint_not_in_the_allowlist` |
| a2a-client: `returns the agent card as a plain object` | `discover_returns_the_agent_card` |
| a2a-client: `wraps discovery errors with cause` | `wraps_a_discovery_failure` |
| a2a-client: `returns the response message` | `send_message_returns_the_response_message` |
| a2a-client: `requires 'message' parameter` / `rejects empty string as 'message'` / `rejects null as 'message'` | `send_message_requires_a_message` |
| a2a-client: `wraps send errors with cause` | `wraps_a_send_failure` |
| a2a-client: `throws for a failed task with no status message` | **N/A** — the in-tree server marks a failed task and the client reports it; no stub emits a non-completed terminal state yet |
| a2a-client: `uses the provided ClientFactory for the endpoint` | `uses_the_client_configured_for_the_endpoint` |
| *(no TS equivalent — schema and bad input)* | `the_input_schema_matches_the_ts_zod_schema`, `an_unknown_operation_is_an_input_error` |
| *(no TS equivalent — end to end over real HTTP)* | `tests/a2a_e2e.rs`: `a2a_agent_discovers_the_served_agent_card`, `a2a_agent_keeps_one_remote_context_across_turns`, `a2a_agent_reports_an_unreachable_server_as_a_model_error`, `a_local_agent_discovers_then_messages_the_served_agent_with_the_a2a_client_tool`, `tasks_get_returns_the_task_a2a_agent_created_and_cancel_rejects_it`, `serves_concurrent_requests_in_isolated_contexts` |
| *(no TS equivalent — live)* | `tests/integ_a2a.rs::a2a_agent_asks_a_bedrock_backed_served_agent_a_question` (`#[ignore]`, Bedrock Haiku 4.5) |

## Divergences

- Protocol types are defined in-tree rather than imported from `@a2a-js/sdk`; serde matches the
  wire format (camelCase, `kind` discriminators, kebab-case update kinds).
- `AsyncLock` is not ported: `tokio::sync::Mutex` is the same FIFO-ish serialization primitive, and
  in factory mode the per-context mutex *holds* the agent, so locking and access are one step.
- The executor works with a concrete `Agent` (and an `AgentFactory` returning one) rather than
  `InvokableAgent`: streaming and snapshots are local-agent capabilities, so TS's runtime
  `asSnapshotAgent` check becomes a compile-time guarantee.
- Single-agent vs factory mode are two constructors (`new` / `with_factory`) instead of one
  options bag, so "exactly one of agent/agentFactory" is a compile error, not a runtime throw.
- `with_max_contexts(0)` clamps to 1 instead of throwing.
- `A2AError` has no Rust equivalent; protocol errors travel as `Error::Other(JsonRpcError)` so a
  server can recover the JSON-RPC code, while agent failures pass through as the crate's own
  errors.
- `EventBus` is an unbounded `tokio::sync::mpsc` sender rather than the SDK's event bus; the
  server (next increment) drains the receiver to aggregate a task or emit SSE.
- The request handling `@a2a-js/sdk`'s `DefaultRequestHandler` performs is in-tree over
  `a2a::TaskStore`, which is a concrete in-memory store rather than TS's `TaskStore` interface:
  there is no second implementation to abstract over yet, so the `taskStore` config option has no
  Rust twin (`A2AServer::task_store()` exposes the store for inspection).
- `A2AServer::builder(name)` mirrors `AgentBuilder`; `.agent()` and `.executor()` write the same
  slot, so "exactly one of agent/agentFactory" cannot be violated and `build()` fails only when
  neither was set.
- In executor mode no representative agent exists, so TS's `__agent_card__` placeholder factory
  call is not ported — the card's description comes from `.description()`, falling back to the
  agent's own `description()` in `.agent()` mode.
- The card advertises protocol version `0.3.0` (what `a2a::types` implements), not TS's
  hard-coded `0.2.0`.
- The agent card is served at both `/.well-known/agent-card.json` and the legacy
  `/.well-known/agent.json`; TS serves only the former.
- Routes mount under the path component of `http_url` (Python's `serve_at_root` / mount-path
  semantics), which TS's express server does not do; `serve_at_root(true)` forces the root.
- `serve()` returns a `ServerHandle` (`local_addr`, `agent_card`, `shutdown`) instead of TS's
  promise that resolves on listen plus an `AbortSignal`; shutdown is graceful.
- `message/send` always runs the agent to completion: `configuration.blocking: false` is treated
  as blocking, since there are no push notifications to deliver the result out of band.
- An executor failure marks the task `failed` in the store (mirroring `DefaultRequestHandler`)
  and is returned as a JSON-RPC error, recovering the executor's own code when it has one.
- `tasks/cancel` answers `-32001` for an unknown task and `-32002` for a terminal one before
  reaching the executor, which still reports `-32004` (unsupported).
- No auth hook (`UserBuilder`) is ported: hosts wrap `router()` in their own `axum` middleware.
- `A2AAgent::name()` returns `&str`, so an agent with no configured name reports its URL until the
  card arrives (TS leaves `name` `undefined`); `description()` stays `Option<&str>` and is `None`
  until then. Card values land in a `OnceLock`, so a configured name/description always wins.
- TS's per-endpoint `ClientFactory` becomes a `reqwest::Client`: that is where auth headers,
  proxies and timeouts live in Rust, so `A2AAgent::builder().http_client(..)` and
  `A2AClientTool::builder(..).endpoint_with_client(..)` replace the factory abstraction.
- `A2AAgent` threads the remote's `contextId` into the next `message/stream` request, so successive
  turns continue one remote conversation; TS sends every turn without a context and gets a fresh
  one. `context_id()` exposes it.
- `A2AAgent::invoke` consumes `stream`, as in TS, but errors are stream items (`Result`) rather
  than exceptions; a transport, SSE-decode or JSON-RPC failure arrives as
  `Error::Model { source: JsonRpcError | reqwest::Error }` so the code is recoverable by
  downcasting the source (TS wraps with `{ cause }`).
- `A2AAgent::stream` takes `InvokeInput` (`impl Into`), and `InvokeInput::{InterruptResponses,
  CheckpointResume}` are rejected: a remote agent cannot resume local interrupts or checkpoints.
- `A2AAgent::as_tool` exists (TS has no equivalent) and mirrors `Agent::as_tool`'s name/description
  defaults and `prompt` schema, minus interrupt propagation — a remote agent cannot interrupt.
  `InvokableAgent` is implemented, so an `A2AAgent` also drops into `Arc<dyn InvokableAgent>`.
- The vended tool is a `Tool` struct (`A2AClientTool`), not a `tool()` factory call, and its
  endpoints are a sorted `BTreeMap`, so the description's endpoint list is deterministic without
  TS's explicit `sort()`. Tool errors come back as tool-error results (the crate's convention)
  rather than thrown exceptions, with TS's message text preserved verbatim.
