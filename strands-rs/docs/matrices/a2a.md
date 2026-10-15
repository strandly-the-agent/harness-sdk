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
| `server.test.ts`, `server.test.node.ts`, `a2a-agent.test.ts` | **not ported yet** (`server.rs`, `A2AAgent` still to come) |

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
