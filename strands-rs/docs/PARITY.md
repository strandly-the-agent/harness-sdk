# Parity with strands-ts

Every TS spec file under `strands-ts/src/` at the tracked spec commit (`7296caba7`, see [README.md](../README.md)) has a row. `ported/total` counts the TS `it(...)` cases that have a Rust test with the same intent; a gap needs a reason (out of scope, TS-only mechanism, deprecated). Rust tests are named after the TS `it(...)` title in snake_case, and each test module's doc comment names its TS spec file.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/__tests__/default-slot.test.ts` | | 0/4 |  |
| `strands-ts/src/__tests__/errors.test.ts` | `error::tests` | 20/29 | `isFetchNetworkError` (3) and `normalizeError` (6): TS-only mechanisms (JS `TypeError` messages from `fetch`, and wrapping thrown non-`Error` values); Rust errors are typed. Transport classification is ported with each provider |
| `strands-ts/src/__tests__/index.test.ts` | | 0/9 |  |
| `strands-ts/src/__tests__/interrupt.test.ts` | `interrupt::tests` | 24/26 | `InterruptError` (2): interrupts are not errors in Rust; an unanswered interrupt is returned as `Err(Interrupt)` and becomes a tool outcome |
| `strands-ts/src/__tests__/mime.test.ts` | `mime::tests` | 6/6 |  |
| `strands-ts/src/__tests__/queue.test.ts` | | 0/10 |  |
| `strands-ts/src/__tests__/state-store.test.ts` | `state_store::tests` | 32/49 | Functions, symbols and `undefined` in values (11) and a `null` key (1): not representable in Rust's typed `Serialize` values and `&str` keys; `isStateSerializable` (5): TS symbol protocol, Rust snapshots call the store directly |
| `strands-ts/src/a2a/__tests__/a2a-agent.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/a2a/__tests__/adapters.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/a2a/__tests__/async-lock.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/a2a/__tests__/events.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/a2a/__tests__/executor.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/a2a/__tests__/server.test.node.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/a2a/__tests__/server.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts` | | 0/15 |  |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-context.test.ts` | `agent::agent_as_tool::tests` | 2/3 | `trips a limit that only the parent-plus-child total exceeds` (1): covered by the shared-limit case |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-state.test.ts` | `agent::agent_as_tool::tests` | 1/1 |  |
| `strands-ts/src/agent/__tests__/agent-as-tool.test.ts` | `agent::agent_as_tool::tests` | 18/31 | Session-manager conflicts (3): Phase 6. Cancellation variants (4): Phase 4 cancel semantics. Explicit invalid name and getter-instance cases (3) are covered by the registry tests; `unwraps toolStreamUpdateEvent` (1) and nested auto-wrap (1) share code with the covered cases; `passes options through` (1) is the options tests |
| `strands-ts/src/agent/__tests__/agent-delegation.test.ts` | | 0/17 |  |
| `strands-ts/src/agent/__tests__/agent.cancel.test.ts` | | 0/28 |  |
| `strands-ts/src/agent/__tests__/agent.checkpoint.test.ts` | | - | Out of scope: checkpointing is out of v0.1 |
| `strands-ts/src/agent/__tests__/agent.concurrent.test.ts` | | 0/22 |  |
| `strands-ts/src/agent/__tests__/agent.context-manager.test.ts` | | 0/16 |  |
| `strands-ts/src/agent/__tests__/agent.continuation.test.ts` | | 0/6 |  |
| `strands-ts/src/agent/__tests__/agent.hook.test.ts` | | 0/70 |  |
| `strands-ts/src/agent/__tests__/agent.interrupt.test.ts` | | 0/32 |  |
| `strands-ts/src/agent/__tests__/agent.invocation-context.test.ts` | `agent::tests::invocation::agent_context` | 10/16 | Unknown-context and frozen-bag cases (2): not constructible in Rust. Hand-written tool forwarding, nested structured output and auxiliary-call cases (4): need agents as tools and auxiliary model calls, covered when those land |
| `strands-ts/src/agent/__tests__/agent.invocation-state.test.ts` | `agent::tests::invocation::state` | 7/11 | Plain-object identity and recursive-cycle variants (3) collapse into the `ptr_eq` cases; AfterToolCallEvent retry identity (1) is covered by the shared `ToolRun` state |
| `strands-ts/src/agent/__tests__/agent.model-retry.test.ts` | `agent::tests::retry` | 10/12 | `disables retries when retryStrategy is null` (1): Rust has one off form, the empty list (P-53). `accepts an array of distinct retry strategy types` (1): only one strategy type exists in v0.1 |
| `strands-ts/src/agent/__tests__/agent.model-routing.test.ts` | | 0/10 |  |
| `strands-ts/src/agent/__tests__/agent.stateful-model.test.ts` | | 0/12 |  |
| `strands-ts/src/agent/__tests__/agent.storage.test.ts` | | 0/7 |  |
| `strands-ts/src/agent/__tests__/agent.test.ts` | `agent::tests::agent`, `agent::tests::fixtures` | 58/147 | Hook errors (8): Rust hooks cannot fail (P-45). Concurrency guards (3): `&mut self` makes concurrent invocation a compile error. Stream abandonment, `shutdown` and async disposal (6): Phase 4. MCP tools-changed, session id and agent metadata (8): Phases 6 and 9. Checkpoints and history offloading in the structured-output attempt cases (5): out of scope or Phase 6. Bedrock model construction (8): Phase 5. The rest of the structured-output attempt-limit and redaction/estimation variants are not yet ported |
| `strands-ts/src/agent/__tests__/agent.tracer.test.node.ts` | | 0/47 |  |
| `strands-ts/src/agent/__tests__/invocation-context.test.ts` | `agent::tests::invocation::context` | 2/2 |  |
| `strands-ts/src/agent/__tests__/printer.test.ts` | `agent::printer::tests`, `agent::tests::agent::configuration` | 8/9 | `prints comprehensive scenario with all output types` (1): the per-output cases cover each line kind |
| `strands-ts/src/agent/__tests__/snapshot.test.ts` | | 0/32 |  |
| `strands-ts/src/agent/__tests__/tool-caller.test.ts` | `agent::tool_caller::tests` | 14/26 | Concurrent-invocation and isInvoking cases (3): `&mut self` makes them compile errors. Proxy mechanics (3): TS-only. `stream` handle and multi-yield variants (4): the stream handle is not ported in Phase 3. Error propagation and parameter pass-through variants (2) are covered by the registry and tool tests |
| `strands-ts/src/background-tasks/__tests__/background-tasks.test.ts` | | 0/9 |  |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-execution.test.ts` | | 0/5 |  |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-transitions.test.ts` | | 0/7 |  |
| `strands-ts/src/background-tasks/in-process/__tests__/manager.test.ts` | | 0/9 |  |
| `strands-ts/src/context-manager/__tests__/context-manager.test.ts` | | 0/20 |  |
| `strands-ts/src/context-manager/__tests__/presets.test.ts` | | 0/17 |  |
| `strands-ts/src/context-manager/__tests__/retrieval-tool.test.ts` | | 0/9 |  |
| `strands-ts/src/context-manager/__tests__/stash-integration.test.ts` | | 0/17 |  |
| `strands-ts/src/context-manager/__tests__/stash.test.ts` | | 0/16 |  |
| `strands-ts/src/context-manager/strategies/__tests__/offload-strategy.test.ts` | | 0/54 |  |
| `strands-ts/src/context-manager/strategies/__tests__/summarize-strategy.test.ts` | | 0/54 |  |
| `strands-ts/src/conversation-manager/__tests__/agentic-context.test.ts` | | 0/30 |  |
| `strands-ts/src/conversation-manager/__tests__/context-compression.test.ts` | | 0/32 |  |
| `strands-ts/src/conversation-manager/__tests__/conversation-manager.test.ts` | | 0/19 |  |
| `strands-ts/src/conversation-manager/__tests__/null-conversation-manager.test.ts` | | 0/4 |  |
| `strands-ts/src/conversation-manager/__tests__/pin.test.ts` | | 0/16 |  |
| `strands-ts/src/conversation-manager/__tests__/sliding-window-conversation-manager.test.ts` | | 0/65 |  |
| `strands-ts/src/conversation-manager/__tests__/summarizing-conversation-manager.test.ts` | | 0/20 |  |
| `strands-ts/src/conversation-manager/__tests__/token-usage-middleware.test.ts` | | 0/15 |  |
| `strands-ts/src/decisions/__tests__/decision-model.test.ts` | | - | Out of scope: decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/llm-decision-model.test.ts` | | - | Out of scope: decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/llm-decision-model.tracing.test.node.ts` | | - | Out of scope: decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/system-one-decision-models.test.node.ts` | | - | Out of scope: decision models are out of v0.1 |
| `strands-ts/src/experimental/__tests__/checkpoint.test.ts` | | - | Out of scope: checkpointing is out of v0.1 |
| `strands-ts/src/hooks/__tests__/events.test.ts` | `hooks::events::tests` | 69/71 | The two serialization-completeness cases hold by construction: records are separate types without the agent |
| `strands-ts/src/hooks/__tests__/registry.test.ts` | `hooks::registry::tests` | 22/25 | Callback errors (3): Rust hook callbacks return `()` and cannot fail (P-45) |
| `strands-ts/src/injection/__tests__/message-injection.test.ts` | | 0/31 |  |
| `strands-ts/src/interventions/__tests__/handler.test.ts` | | 0/2 |  |
| `strands-ts/src/interventions/__tests__/registry.test.ts` | | 0/44 |  |
| `strands-ts/src/logging/__tests__/logger.test.ts` | | 0/6 |  |
| `strands-ts/src/logging/__tests__/warn-once.test.ts` | `logging::warn_once::tests` | 3/3 |  |
| `strands-ts/src/mcp/__tests__/client-annotations.test.ts` | | 0/4 |  |
| `strands-ts/src/mcp/__tests__/client-progress.test.ts` | | 0/2 |  |
| `strands-ts/src/mcp/__tests__/client.test.ts` | | 0/126 |  |
| `strands-ts/src/mcp/__tests__/config.test.browser.ts` | | - | Not applicable: browser-only spec |
| `strands-ts/src/mcp/__tests__/config.test.node.ts` | | 0/48 |  |
| `strands-ts/src/mcp/__tests__/task-client-legacy.test.node.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client-stdio.test.node.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client.test.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-transport.test.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/src/memory/__tests__/memory-manager.test.ts` | | 0/81 |  |
| `strands-ts/src/memory/__tests__/memory-tracing.test.ts` | | 0/17 |  |
| `strands-ts/src/memory/extraction/__tests__/extraction.test.ts` | | 0/42 |  |
| `strands-ts/src/memory/extraction/__tests__/model-extractor.test.ts` | | 0/14 |  |
| `strands-ts/src/memory/extraction/__tests__/resolve-extraction-config.test.ts` | | 0/13 |  |
| `strands-ts/src/middleware/__tests__/agent-middleware.test.ts` | | 0/62 |  |
| `strands-ts/src/middleware/__tests__/copy-on-input.test.ts` | | 0/21 |  |
| `strands-ts/src/middleware/__tests__/custom-stages.test.ts` | | 0/5 |  |
| `strands-ts/src/middleware/__tests__/middleware-interrupts.test.ts` | | 0/15 |  |
| `strands-ts/src/middleware/__tests__/registry.test.ts` | | 0/24 |  |
| `strands-ts/src/models/__tests__/anthropic.test.ts` | | 0/186 |  |
| `strands-ts/src/models/__tests__/bedrock.test.ts` | | 0/274 |  |
| `strands-ts/src/models/__tests__/bedrock.transport.test.browser.ts` | | - | Not applicable: browser-only spec |
| `strands-ts/src/models/__tests__/cache-portability.test.ts` | | 0/1 | Every case drives a provider's request builder (Bedrock, Anthropic, Vercel); arrives with the providers in Phase 5. The generic part, `resolve_cache_section`, is covered in `models::model::tests` |
| `strands-ts/src/models/__tests__/defaults.test.ts` | `models::defaults::tests` | 7/7 |  |
| `strands-ts/src/models/__tests__/google.test.ts` | | - | Out of scope: Google provider is out of v0.1 |
| `strands-ts/src/models/__tests__/model-info-lookup.test.ts` | `models::model_info::tests` | 9/9 |  |
| `strands-ts/src/models/__tests__/model-info.test.node.ts` | `models::model_info::tests` | 1/1 |  |
| `strands-ts/src/models/__tests__/model-info.test.ts` | `models::model_info::tests` | 4/5 | `returns a frozen record` (1): Rust values are immutable through shared references |
| `strands-ts/src/models/__tests__/model-proxy.test.ts` | `models::model_proxy::tests` | 16/17 | `sends text to a custom provider serving a model that takes temporary messages` (1): needs a provider that maps temporary messages (Phase 5) |
| `strands-ts/src/models/__tests__/model.test.ts` | `models::model::tests`, `models::model::aggregate::tests`, `models::summarize::tests` | 68/68 |  |
| `strands-ts/src/models/__tests__/native-output.test.ts` | `models::native_output::tests` | 14/15 | `matches the Anthropic SDK transform` (1): compares with `transformJSONSchema` from the Anthropic TS SDK, which Rust doesn't have; the conversion rules it exercises are covered by the hand-written schema cases. Zod schemas are written as the JSON `z.toJSONSchema` produces |
| `strands-ts/src/models/__tests__/server-side-summary-capability.test.ts` | | 0/1 | Needs an agent with the `Offload` context strategy (Phases 3 and 6) and the providers (Phase 5); the trait part (client-side `summarize` default, `model_info` capability reads) is in `models::summarize::tests` and `models::model::tests` |
| `strands-ts/src/models/__tests__/streaming.test.ts` | `models::streaming::tests` | 14/14 | `isModelStreamEvent` is ported as deserializing into `ModelStreamEvent` |
| `strands-ts/src/models/__tests__/strict-schema.test.ts` | `models::strict_schema::tests` | 24/24 |  |
| `strands-ts/src/models/__tests__/vercel.test.ts` | | - | Out of scope: Vercel provider is out of v0.1 |
| `strands-ts/src/models/openai/__tests__/chat.test.ts` | | 0/96 |  |
| `strands-ts/src/models/openai/__tests__/errors.test.ts` | | 0/7 |  |
| `strands-ts/src/models/openai/__tests__/mantle.test.ts` | | 0/30 |  |
| `strands-ts/src/models/openai/__tests__/responses-cache.test.ts` | | 0/17 |  |
| `strands-ts/src/models/openai/__tests__/responses.test.ts` | | 0/72 |  |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.decision-model.test.ts` | | - | Out of scope: decision-model classifier is out of v0.1 |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.test.ts` | | 0/25 |  |
| `strands-ts/src/models/routing/__tests__/fallback-strategy.test.ts` | | 0/5 |  |
| `strands-ts/src/models/routing/__tests__/router.test.ts` | | 0/22 |  |
| `strands-ts/src/multiagent/__tests__/agent-tool-streamer.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/events.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.invocation-context.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.invocation-state.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.tracer.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/interrupts.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/nodes.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/plugins.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/snapshot.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/spec.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/state.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-context.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-state.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.tracer.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/src/plugins/__tests__/plugin.test.ts` | | 0/7 |  |
| `strands-ts/src/plugins/__tests__/registry.test.ts` | | 0/10 |  |
| `strands-ts/src/registry/__tests__/tool-registry.test.ts` | `tools::registry::tests` | 30/33 | Non-string name, non-string description, null description (3): not expressible with `Tool::name() -> &str` and a required description |
| `strands-ts/src/retry/__tests__/backoff-strategy.test.ts` | `retry::backoff_strategy::tests` | 15/19 | `rejects attempts below 1` (3) and `rejects non-integer attempts` (1): `BackoffContext.attempt` is a `NonZeroU32`, so zero and fractional attempts can't be built |
| `strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts` | `retry::default_model_retry_strategy::tests` | 12/15 | `registers an AfterModelCallEvent hook` (1): arrives with the hook registry in Phase 3; `rejects maxAttempts below 1` (1): `max_attempts` is a `NonZeroU32`; `lets subclasses expand the retryable set` (1): Rust has no subclassing, a custom set is a `ModelRetryStrategy` impl (the composition case is `short_circuits_without_retry_..`) |
| `strands-ts/src/sandbox/__tests__/default.test.browser.ts` | | - | Not applicable: browser-only spec |
| `strands-ts/src/sandbox/__tests__/default.test.node.ts` | | 0/4 |  |
| `strands-ts/src/sandbox/__tests__/docker.test.node.ts` | | - | Out of scope: Docker sandbox is out of v0.1 |
| `strands-ts/src/sandbox/__tests__/errors.test.node.ts` | `sandbox::errors::tests` | 9/9 |  |
| `strands-ts/src/sandbox/__tests__/not-a-sandbox-local-environment.test.node.ts` | | 0/27 |  |
| `strands-ts/src/sandbox/__tests__/posix-shell.test.node.ts` | | 0/55 |  |
| `strands-ts/src/sandbox/__tests__/read-file-errors.test.node.ts` | | 0/6 |  |
| `strands-ts/src/sandbox/__tests__/ssh.test.node.ts` | | - | Out of scope: SSH sandbox is out of v0.1 |
| `strands-ts/src/sandbox/__tests__/stream-process.test.node.ts` | | 0/5 |  |
| `strands-ts/src/session/__tests__/file-storage.test.node.ts` | | - | Out of scope: deprecated session FileStorage |
| `strands-ts/src/session/__tests__/s3-storage.test.ts` | | - | Out of scope: S3 storage is out of v0.1 |
| `strands-ts/src/session/__tests__/session-manager.test.ts` | | 0/71 |  |
| `strands-ts/src/session/__tests__/snapshot-storage-adapter.test.ts` | | 0/24 |  |
| `strands-ts/src/session/__tests__/validation.test.ts` | | 0/10 |  |
| `strands-ts/src/storage/__tests__/in-memory-storage.test.ts` | | 0/25 |  |
| `strands-ts/src/storage/__tests__/local-file-storage.test.node.ts` | | 0/31 |  |
| `strands-ts/src/storage/__tests__/namespaced-storage.test.ts` | | 0/10 |  |
| `strands-ts/src/storage/__tests__/s3-storage.test.ts` | | - | Out of scope: S3 storage is out of v0.1 |
| `strands-ts/src/storage/embeddings/__tests__/bedrock.test.ts` | | - | Out of scope: embeddings are out of v0.1 |
| `strands-ts/src/storage/search/__tests__/keyword.test.ts` | | 0/13 |  |
| `strands-ts/src/storage/search/__tests__/qmd.test.node.ts` | | - | Out of scope: QMD search needs the external QMD SQLite index; only keyword search is in v0.1 |
| `strands-ts/src/storage/search/__tests__/s3-vector.test.ts` | | - | Out of scope: vector search is out of v0.1 |
| `strands-ts/src/telemetry/__tests__/config.test.node.ts` | | 0/18 |  |
| `strands-ts/src/telemetry/__tests__/config.test.ts` | | 0/6 |  |
| `strands-ts/src/telemetry/__tests__/json.test.ts` | | 0/15 |  |
| `strands-ts/src/telemetry/__tests__/local-trace.test.ts` | | 0/16 |  |
| `strands-ts/src/telemetry/__tests__/meter.test.ts` | | 0/62 |  |
| `strands-ts/src/telemetry/__tests__/tracer.test.node.ts` | | 0/100 |  |
| `strands-ts/src/tools/__tests__/structured-output-tool.test.ts` | `tools::structured_output::tests` | 11/12 | `returns error result for non-ZodError exceptions` (1): validation cannot throw anything but its own failures |
| `strands-ts/src/tools/__tests__/tool-factory.test.ts` | | 0/8 |  |
| `strands-ts/src/tools/__tests__/tool.test.ts` | `tools::tool::tests`, `tools::function_tool::tests` | 31/46 | TS-only (15): undefined returns, deep copies of returned objects, circular references, functions in values, non-Error throws, Error subclasses and stack traces, instanceof and interface-signature checks |
| `strands-ts/src/tools/__tests__/zod-tool.test.ts` | `tools::function_tool::tests::typed`, `tools::schema::tests` | 7/32 | Zod-specific (25): Zod error text and Zod schema features map to `typed_callback` with schemars plus `jsonschema`; the covered cases are schema generation, typed input, invalid input and uncompilable schemas. The macro cases arrive with `#[tool]` |
| `strands-ts/src/tools/executors/__tests__/executor.cancel-signal.test.ts` | `tools::executors::tests` | 2/3 | `keeps middleware and tools on the supplied execution signal` (1): needs middleware (Phase 4) |
| `strands-ts/src/types/__tests__/agent.test.ts` | | 0/30 | `checkpoint` (4): out of scope, checkpointing is out of v0.1; the other 26 arrive with `AgentResult` and the agent loop |
| `strands-ts/src/types/__tests__/citations.test.ts` | `types::tests::citations` | 6/6 |  |
| `strands-ts/src/types/__tests__/json.test.ts` | | 0/44 | TS-only mechanism: `deepCopy` and `deepCopyWithValidation` reject JS values JSON can't hold (functions, symbols, `undefined`); a Rust `JsonValue` is always serializable and `Clone` is the deep copy. JSON reading and writing rules are tested in `types::json::tests` |
| `strands-ts/src/types/__tests__/media.test.ts` | `types::tests::media` | 51/51 |  |
| `strands-ts/src/types/__tests__/messages.test.ts` | `types::tests::messages` | 86/86 |  |
| `strands-ts/src/types/__tests__/validation.test.ts` | `types::validation::tests` | 4/5 | `when value is null`: Rust `Option` has no null/undefined split; `None` reports `undefined` |
| `strands-ts/src/vended-interventions/cedar/__tests__/cedar.test.node.ts` | | 0/39 |  |
| `strands-ts/src/vended-interventions/hitl/__tests__/hitl.test.ts` | | 0/36 |  |
| `strands-ts/src/vended-interventions/steering/__tests__/handler.test.ts` | | - | Out of scope: steering is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/llm.test.ts` | | - | Out of scope: steering is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/tool-ledger.test.ts` | | - | Out of scope: steering is out of v0.1 |
| `strands-ts/src/vended-memory-stores/bedrock-knowledge-base/__tests__/bedrock-knowledge-base-store.test.node.ts` | | - | Out of scope: Bedrock Knowledge Base store is out of v0.1 |
| `strands-ts/src/vended-memory-stores/file-memory-store/__tests__/file-memory-store.test.node.ts` | | 0/52 |  |
| `strands-ts/src/vended-memory-stores/test-memory-store/__tests__/test-memory-store.test.node.ts` | | - | Out of scope: TS test helper store; Rust tests use `strands::testing` |
| `strands-ts/src/vended-plugins/context-injector/__tests__/plugin.test.ts` | | 0/19 |  |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/file-storage-sandbox.test.node.ts` | | - | Out of scope: the context-offloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.node.ts` | | - | Out of scope: the context-offloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.ts` | | - | Out of scope: the context-offloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts` | | - | Out of scope: the context-offloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.node.ts` | | - | Out of scope: the context-offloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.ts` | | - | Out of scope: the context-offloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/goal/__tests__/plugin.test.ts` | | - | Out of scope: GoalLoop is out of v0.1 |
| `strands-ts/src/vended-plugins/skills/__tests__/agent-skills.test.node.ts` | | 0/39 |  |
| `strands-ts/src/vended-plugins/skills/__tests__/skill.test.node.ts` | | 0/46 |  |
| `strands-ts/src/vended-tools/__tests__/paths.test.ts` | | 0/7 |  |
| `strands-ts/src/vended-tools/a2a-client/__tests__/a2a-client.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/src/vended-tools/bash/__tests__/bash.test.node.ts` | | 0/46 |  |
| `strands-ts/src/vended-tools/edit/__tests__/edit.test.node.ts` | | 0/13 |  |
| `strands-ts/src/vended-tools/file-editor/__tests__/file-editor.test.node.ts` | | - | Out of scope: deprecated `fileEditor` |
| `strands-ts/src/vended-tools/graph/__tests__/graph.test.ts` | | - | Out of scope: Graph is out of v0.1 |
| `strands-ts/src/vended-tools/handoff-to-user/__tests__/handoff-to-user.test.ts` | | 0/9 |  |
| `strands-ts/src/vended-tools/http-request/__tests__/http-request.test.ts` | | 0/12 |  |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.node.ts` | | 0/1 |  |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.ts` | | 0/28 |  |
| `strands-ts/src/vended-tools/notebook/__tests__/notebook.test.ts` | | 0/62 |  |
| `strands-ts/src/vended-tools/python-repl/__tests__/python-repl.test.node.ts` | | 0/12 |  |
| `strands-ts/src/vended-tools/read/__tests__/read.test.node.ts` | | 0/34 |  |
| `strands-ts/src/vended-tools/shell/__tests__/shell.test.node.ts` | | 0/18 |  |
| `strands-ts/src/vended-tools/sleep/__tests__/sleep.test.ts` | | 0/19 |  |
| `strands-ts/src/vended-tools/stop/__tests__/stop.test.ts` | | 0/18 |  |
| `strands-ts/src/vended-tools/subagent/__tests__/subagent.test.ts` | | 0/33 |  |
| `strands-ts/src/vended-tools/swarm/__tests__/swarm.test.ts` | | - | Out of scope: Swarm is out of v0.1 |
| `strands-ts/src/vended-tools/web-fetch/__tests__/extract.test.ts` | | 0/15 |  |
| `strands-ts/src/vended-tools/web-fetch/__tests__/web-fetch.test.ts` | | 0/30 |  |
| `strands-ts/src/vended-tools/write/__tests__/write.test.node.ts` | | 0/11 |  |

## Live checklist

The integration specs under `strands-ts/test/integ/` run against real models and services. They map to `#[ignore]`d `live_*` tests (see [TESTING.md](TESTING.md)); the Rust tests column says `live` for them.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/test/integ/a2a/a2a-agent.test.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/test/integ/a2a/express-server.test.node.ts` | | - | Out of scope: A2A is out of v0.1 |
| `strands-ts/test/integ/agent-as-tool.test.ts` | live | 0/1 |  |
| `strands-ts/test/integ/agent.cancel.test.ts` | live | 0/5 |  |
| `strands-ts/test/integ/agent.checkpoint.test.node.ts` | | - | Out of scope: checkpointing is out of v0.1 |
| `strands-ts/test/integ/agent.test.ts` | live | 0/18 |  |
| `strands-ts/test/integ/cedar.test.node.ts` | live | 0/3 |  |
| `strands-ts/test/integ/conversation-manager/agentic-context.test.ts` | live | 0/11 |  |
| `strands-ts/test/integ/conversation-manager/context-manager-message-reduction.test.ts` | live | 0/1 |  |
| `strands-ts/test/integ/conversation-manager/context-manager-summarize-tool-calls.test.ts` | live | 0/1 |  |
| `strands-ts/test/integ/conversation-manager/summarizing-conversation-manager.test.ts` | live | 0/2 |  |
| `strands-ts/test/integ/decisions/decision-model.test.ts` | | - | Out of scope: decision models are out of v0.1 |
| `strands-ts/test/integ/decisions/system-one-decision-models.test.node.ts` | | - | Out of scope: decision models are out of v0.1 |
| `strands-ts/test/integ/environment.test.browser.ts` | | - | Not applicable: browser-only spec |
| `strands-ts/test/integ/environment.test.node.ts` | | - | Not applicable: checks the TS runtime and toolchain |
| `strands-ts/test/integ/environment.test.ts` | | - | Not applicable: checks the TS runtime and toolchain |
| `strands-ts/test/integ/goal/goal.test.node.ts` | | - | Out of scope: GoalLoop is out of v0.1 |
| `strands-ts/test/integ/interrupt.test.ts` | live | 0/3 |  |
| `strands-ts/test/integ/interventions.test.ts` | live | 0/52 |  |
| `strands-ts/test/integ/mcp/mcp-connect-abort.test.node.ts` | live | 0/1 |  |
| `strands-ts/test/integ/mcp/mcp-legacy-fallback.test.node.ts` | live | 0/4 |  |
| `strands-ts/test/integ/mcp/mcp-reconnect.test.node.ts` | live | 0/3 |  |
| `strands-ts/test/integ/mcp/mcp-sep2663.test.node.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/test/integ/mcp/mcp-tasks-stdio.test.node.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/test/integ/mcp/mcp-tasks.test.node.ts` | | - | Out of scope: MCP tasks are out of v0.1 |
| `strands-ts/test/integ/mcp/mcp-v2.test.node.ts` | live | 0/7 |  |
| `strands-ts/test/integ/mcp/mcp.test.node.ts` | live | 0/7 |  |
| `strands-ts/test/integ/memory/bedrock-knowledge-base-memory-manager.test.node.ts` | | - | Out of scope: Bedrock Knowledge Base store is out of v0.1 |
| `strands-ts/test/integ/memory/bedrock-knowledge-base-store.test.node.ts` | | - | Out of scope: Bedrock Knowledge Base store is out of v0.1 |
| `strands-ts/test/integ/models/anthropic.test.ts` | live | 0/13 |  |
| `strands-ts/test/integ/models/bedrock.test.node.ts` | live | 0/3 |  |
| `strands-ts/test/integ/models/bedrock.test.ts` | live | 0/29 |  |
| `strands-ts/test/integ/models/classifier-strategy.test.ts` | live | 0/1 |  |
| `strands-ts/test/integ/models/context-windows.test.ts` | live | 0/1 |  |
| `strands-ts/test/integ/models/google.test.ts` | | - | Out of scope: Google provider is out of v0.1 |
| `strands-ts/test/integ/models/model-smoke.test.ts` | live | 0/2 |  |
| `strands-ts/test/integ/models/openai/chat.test.ts` | live | 0/7 |  |
| `strands-ts/test/integ/models/openai/mantle-routing.test.node.ts` | live | 0/1 |  |
| `strands-ts/test/integ/models/openai/mantle.test.node.ts` | live | 0/4 |  |
| `strands-ts/test/integ/models/openai/responses.test.ts` | live | 0/12 |  |
| `strands-ts/test/integ/models/openai/session-cache-key.test.ts` | live | 0/6 |  |
| `strands-ts/test/integ/multiagent/graph.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/test/integ/multiagent/interrupt-hook.test.node.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/test/integ/multiagent/interrupt-node.test.node.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/test/integ/multiagent/interrupt-session.test.node.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/test/integ/multiagent/session-manager.test.node.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/test/integ/multiagent/swarm.test.ts` | | - | Out of scope: Graph and Swarm are out of v0.1 |
| `strands-ts/test/integ/qmd-search-strategy.test.node.ts` | | - | Out of scope: QMD search needs the external QMD SQLite index; only keyword search is in v0.1 |
| `strands-ts/test/integ/s3-vector-search-strategy.test.node.ts` | | - | Out of scope: vector search is out of v0.1 |
| `strands-ts/test/integ/sandbox/docker.test.node.ts` | | - | Out of scope: Docker sandbox is out of v0.1 |
| `strands-ts/test/integ/sandbox/ssh.test.node.ts` | | - | Out of scope: SSH sandbox is out of v0.1 |
| `strands-ts/test/integ/session-manager.test.node.ts` | live | 0/9 |  |
| `strands-ts/test/integ/skills/agent-skills.test.node.ts` | live | 0/4 |  |
| `strands-ts/test/integ/telemetry.test.node.ts` | live | 0/36 |  |
| `strands-ts/test/integ/tools/bash.test.node.ts` | live | 0/4 |  |
| `strands-ts/test/integ/tools/file-editor.test.node.ts` | | - | Out of scope: deprecated `fileEditor` |
| `strands-ts/test/integ/tools/http-request.test.ts` | live | 0/1 |  |
| `strands-ts/test/integ/tools/notebook.test.ts` | live | 0/3 |  |
| `strands-ts/test/integ/vended-interventions/steering/steering.test.node.ts` | | - | Out of scope: steering is out of v0.1 |

## Conformance fixtures

`types::tests::conformance` parses each TS-written fixture with the persisted-data reader, writes it back with the persisted-data writer and compares bytes.

| Fixtures | Rust test | Status |
|---|---|---|
| `messages/*` (every block, media formats, unknown blocks and formats, JSON numbers, legacy tracking id, non-canonical base64) | `message_fixtures_round_trip_byte_identically`, `non_canonical_base64_is_read_leniently_and_written_canonically` | byte-identical |
| `messages/text-edge-cases.json` | `text_edge_cases_round_trip_with_lone_surrogates_replaced` | byte-identical except lone surrogates, which become U+FFFD (see DIVERGENCES.md) |
| `tools/*`, `interrupt/*`, `agent/messages-after-tool-loop.json` | `tool_fixtures_round_trip`, `interrupt_*`, `messages_after_tool_loop_round_trip` | byte-identical |
| `agent/snapshot-*.json`, `storage/**/snapshot_*.json` | `snapshot_fixtures_round_trip`, `interrupted_snapshot_keeps_its_pending_tool_execution` | byte-identical, including the typed `messages`, `systemPrompt` and `interrupts` inside agent snapshots |
| `stream/model-stream-events.json` | `model_stream_event_fixture_round_trips_byte_identically` | byte-identical, including index-keyed `redactedContent` and present `null`s |
| `stream/agent-stream-events*.json`, `hooks/*` | | not yet ported, arrives with the stream and hook event records |
| `storage/**/manifest.json` | | not yet ported, arrives with `SessionManager` |
| `stash/*` | | not yet ported, arrives with the context manager stash |
| `telemetry/*` | | not yet ported, arrives with telemetry |
