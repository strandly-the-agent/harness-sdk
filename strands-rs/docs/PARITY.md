# Parity

Every TS spec file at the pinned TS baseline maps to Rust tests here. Each Rust test is named after the snake_case form of the TS `it(...)` title. A gap needs a reason (out of scope, TS-only mechanism, deprecated).

`total` counts `it(...)`/`test(...)` call sites in the TS file (a parameterized `it.each` counts once).

## In scope

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/__tests__/default-slot.test.ts` | | 0/4 | |
| `strands-ts/src/__tests__/errors.test.ts` | `strands::error::tests` | 20/29 | `isFetchNetworkError` (3) and `normalizeError` (6) are TS-only (JS `TypeError` messages and non-`Error` throwables); providers map transport failures in Phase 5 |
| `strands-ts/src/__tests__/index.test.ts` | | 0/9 | |
| `strands-ts/src/__tests__/interrupt.test.ts` | `strands::interrupt::tests` | 6/26 | `InterruptError` (2), `InterruptState` behaviour, restore validation and `interruptFromAgent` (18) land with interrupt resume in Phase 4 |
| `strands-ts/src/__tests__/mime.test.ts` | `strands::mime::tests` | 6/6 | |
| `strands-ts/src/__tests__/queue.test.ts` | | 0/10 | |
| `strands-ts/src/__tests__/state-store.test.ts` | | 0/49 | |
| `strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts` | | 0/15 | |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-context.test.ts` | | 0/3 | |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-state.test.ts` | | 0/1 | |
| `strands-ts/src/agent/__tests__/agent-as-tool.test.ts` | | 0/31 | |
| `strands-ts/src/agent/__tests__/agent-delegation.test.ts` | | 0/17 | |
| `strands-ts/src/agent/__tests__/agent.cancel.test.ts` | | 0/28 | |
| `strands-ts/src/agent/__tests__/agent.concurrent.test.ts` | | 0/22 | |
| `strands-ts/src/agent/__tests__/agent.context-manager.test.ts` | | 0/16 | |
| `strands-ts/src/agent/__tests__/agent.continuation.test.ts` | | 0/6 | |
| `strands-ts/src/agent/__tests__/agent.hook.test.ts` | | 0/70 | |
| `strands-ts/src/agent/__tests__/agent.interrupt.test.ts` | | 0/32 | |
| `strands-ts/src/agent/__tests__/agent.invocation-context.test.ts` | | 0/16 | |
| `strands-ts/src/agent/__tests__/agent.invocation-state.test.ts` | | 0/11 | |
| `strands-ts/src/agent/__tests__/agent.model-retry.test.ts` | | 0/12 | |
| `strands-ts/src/agent/__tests__/agent.model-routing.test.ts` | | 0/10 | |
| `strands-ts/src/agent/__tests__/agent.stateful-model.test.ts` | | 0/12 | |
| `strands-ts/src/agent/__tests__/agent.storage.test.ts` | | 0/7 | |
| `strands-ts/src/agent/__tests__/agent.test.ts` | | 0/142 | |
| `strands-ts/src/agent/__tests__/agent.tracer.test.node.ts` | | 0/46 | |
| `strands-ts/src/agent/__tests__/invocation-context.test.ts` | | 0/2 | |
| `strands-ts/src/agent/__tests__/printer.test.ts` | | 0/9 | |
| `strands-ts/src/agent/__tests__/snapshot.test.ts` | | 0/32 | |
| `strands-ts/src/agent/__tests__/tool-caller.test.ts` | | 0/26 | |
| `strands-ts/src/background-tasks/__tests__/background-tasks.test.ts` | | 0/9 | |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-execution.test.ts` | | 0/5 | |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-transitions.test.ts` | | 0/7 | |
| `strands-ts/src/background-tasks/in-process/__tests__/manager.test.ts` | | 0/8 | |
| `strands-ts/src/context-manager/__tests__/context-manager.test.ts` | | 0/20 | |
| `strands-ts/src/context-manager/__tests__/presets.test.ts` | | 0/17 | |
| `strands-ts/src/context-manager/__tests__/retrieval-tool.test.ts` | | 0/9 | |
| `strands-ts/src/context-manager/__tests__/stash-integration.test.ts` | | 0/17 | |
| `strands-ts/src/context-manager/__tests__/stash.test.ts` | | 0/16 | |
| `strands-ts/src/context-manager/strategies/__tests__/offload-strategy.test.ts` | | 0/54 | |
| `strands-ts/src/context-manager/strategies/__tests__/summarize-strategy.test.ts` | | 0/52 | |
| `strands-ts/src/conversation-manager/__tests__/agentic-context.test.ts` | | 0/30 | |
| `strands-ts/src/conversation-manager/__tests__/context-compression.test.ts` | | 0/32 | |
| `strands-ts/src/conversation-manager/__tests__/conversation-manager.test.ts` | | 0/19 | |
| `strands-ts/src/conversation-manager/__tests__/null-conversation-manager.test.ts` | | 0/4 | |
| `strands-ts/src/conversation-manager/__tests__/pin.test.ts` | | 0/16 | |
| `strands-ts/src/conversation-manager/__tests__/sliding-window-conversation-manager.test.ts` | | 0/65 | |
| `strands-ts/src/conversation-manager/__tests__/summarizing-conversation-manager.test.ts` | | 0/20 | |
| `strands-ts/src/conversation-manager/__tests__/token-usage-middleware.test.ts` | | 0/15 | |
| `strands-ts/src/experimental/__tests__/checkpoint.test.ts` | | 0/6 | |
| `strands-ts/src/hooks/__tests__/events.test.ts` | | 0/71 | |
| `strands-ts/src/hooks/__tests__/registry.test.ts` | | 0/25 | |
| `strands-ts/src/injection/__tests__/message-injection.test.ts` | | 0/31 | |
| `strands-ts/src/interventions/__tests__/handler.test.ts` | | 0/2 | |
| `strands-ts/src/interventions/__tests__/registry.test.ts` | | 0/44 | |
| `strands-ts/src/logging/__tests__/logger.test.ts` | | 0/6 | |
| `strands-ts/src/logging/__tests__/warn-once.test.ts` | | 0/3 | |
| `strands-ts/src/mcp/__tests__/client-annotations.test.ts` | | 0/4 | |
| `strands-ts/src/mcp/__tests__/client-progress.test.ts` | | 0/2 | |
| `strands-ts/src/mcp/__tests__/client.test.ts` | | 0/126 | |
| `strands-ts/src/mcp/__tests__/config.test.node.ts` | | 0/48 | |
| `strands-ts/src/memory/__tests__/memory-manager.test.ts` | | 0/81 | |
| `strands-ts/src/memory/__tests__/memory-tracing.test.ts` | | 0/17 | |
| `strands-ts/src/memory/extraction/__tests__/extraction.test.ts` | | 0/42 | |
| `strands-ts/src/memory/extraction/__tests__/model-extractor.test.ts` | | 0/14 | |
| `strands-ts/src/memory/extraction/__tests__/resolve-extraction-config.test.ts` | | 0/13 | |
| `strands-ts/src/middleware/__tests__/agent-middleware.test.ts` | | 0/62 | |
| `strands-ts/src/middleware/__tests__/copy-on-input.test.ts` | | 0/21 | |
| `strands-ts/src/middleware/__tests__/custom-stages.test.ts` | | 0/5 | |
| `strands-ts/src/middleware/__tests__/middleware-interrupts.test.ts` | | 0/15 | |
| `strands-ts/src/middleware/__tests__/registry.test.ts` | | 0/24 | |
| `strands-ts/src/models/__tests__/anthropic.test.ts` | | 0/177 | |
| `strands-ts/src/models/__tests__/bedrock.test.ts` | | 0/272 | |
| `strands-ts/src/models/__tests__/cache-portability.test.ts` | | 0/1 | |
| `strands-ts/src/models/__tests__/defaults.test.ts` | | 0/7 | |
| `strands-ts/src/models/__tests__/google.test.ts` | | 0/85 | |
| `strands-ts/src/models/__tests__/model-info-lookup.test.ts` | | 0/9 | |
| `strands-ts/src/models/__tests__/model-info.test.node.ts` | | 0/1 | |
| `strands-ts/src/models/__tests__/model-info.test.ts` | | 0/5 | |
| `strands-ts/src/models/__tests__/model-proxy.test.ts` | | 0/18 | |
| `strands-ts/src/models/__tests__/model.test.ts` | | 0/68 | |
| `strands-ts/src/models/__tests__/native-output.test.ts` | | 0/11 | |
| `strands-ts/src/models/__tests__/server-side-summary-capability.test.ts` | | 0/1 | |
| `strands-ts/src/models/__tests__/streaming.test.ts` | `strands::models::streaming::tests` | 5/14 | `isModelStreamEvent` (9) lands with `ModelStreamEvent` in Phase 2 |
| `strands-ts/src/models/__tests__/strict-schema.test.ts` | | 0/24 | |
| `strands-ts/src/models/__tests__/vercel.test.ts` | | 0/100 | |
| `strands-ts/src/models/openai/__tests__/chat.test.ts` | | 0/96 | |
| `strands-ts/src/models/openai/__tests__/errors.test.ts` | | 0/6 | |
| `strands-ts/src/models/openai/__tests__/mantle.test.ts` | | 0/30 | |
| `strands-ts/src/models/openai/__tests__/responses-cache.test.ts` | | 0/17 | |
| `strands-ts/src/models/openai/__tests__/responses.test.ts` | | 0/72 | |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.test.ts` | | 0/22 | |
| `strands-ts/src/models/routing/__tests__/fallback-strategy.test.ts` | | 0/5 | |
| `strands-ts/src/models/routing/__tests__/router.test.ts` | | 0/21 | |
| `strands-ts/src/plugins/__tests__/plugin.test.ts` | | 0/7 | |
| `strands-ts/src/plugins/__tests__/registry.test.ts` | | 0/10 | |
| `strands-ts/src/registry/__tests__/tool-registry.test.ts` | | 0/33 | |
| `strands-ts/src/retry/__tests__/backoff-strategy.test.ts` | | 0/19 | |
| `strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts` | | 0/15 | |
| `strands-ts/src/sandbox/__tests__/default.test.node.ts` | | 0/4 | |
| `strands-ts/src/sandbox/__tests__/errors.test.node.ts` | `strands::error::tests` | 9/9 | |
| `strands-ts/src/sandbox/__tests__/not-a-sandbox-local-environment.test.node.ts` | | 0/26 | |
| `strands-ts/src/sandbox/__tests__/posix-shell.test.node.ts` | | 0/52 | |
| `strands-ts/src/sandbox/__tests__/read-file-errors.test.node.ts` | | 0/3 | |
| `strands-ts/src/sandbox/__tests__/stream-process.test.node.ts` | | 0/5 | |
| `strands-ts/src/session/__tests__/session-manager.test.ts` | | 0/71 | |
| `strands-ts/src/session/__tests__/snapshot-storage-adapter.test.ts` | | 0/24 | |
| `strands-ts/src/session/__tests__/validation.test.ts` | | 0/10 | |
| `strands-ts/src/storage/__tests__/in-memory-storage.test.ts` | | 0/25 | |
| `strands-ts/src/storage/__tests__/local-file-storage.test.node.ts` | | 0/30 | |
| `strands-ts/src/storage/__tests__/namespaced-storage.test.ts` | | 0/10 | |
| `strands-ts/src/storage/search/__tests__/keyword.test.ts` | | 0/13 | |
| `strands-ts/src/telemetry/__tests__/config.test.node.ts` | | 0/17 | |
| `strands-ts/src/telemetry/__tests__/config.test.ts` | | 0/6 | |
| `strands-ts/src/telemetry/__tests__/json.test.ts` | | 0/15 | |
| `strands-ts/src/telemetry/__tests__/local-trace.test.ts` | | 0/16 | |
| `strands-ts/src/telemetry/__tests__/meter.test.ts` | | 0/62 | |
| `strands-ts/src/telemetry/__tests__/tracer.test.node.ts` | | 0/99 | |
| `strands-ts/src/tools/__tests__/structured-output-tool.test.ts` | | 0/12 | |
| `strands-ts/src/tools/__tests__/tool-factory.test.ts` | | 0/8 | |
| `strands-ts/src/tools/__tests__/tool.test.ts` | | 0/44 | |
| `strands-ts/src/tools/__tests__/zod-tool.test.ts` | | 0/32 | |
| `strands-ts/src/tools/executors/__tests__/executor.cancel-signal.test.ts` | | 0/3 | |
| `strands-ts/src/types/__tests__/agent.test.ts` | | 0/30 | every case tests `AgentResult`, which lands in Phase 3 |
| `strands-ts/src/types/__tests__/citations.test.ts` | `strands::types::citations::tests` | 6/6 | |
| `strands-ts/src/types/__tests__/json.test.ts` | | 0/44 | TS-only: `deepCopy`/`deepCopyWithValidation` clone JS values and reject functions, symbols, `undefined` and cycles, none of which a Rust `JsonValue` can hold (`clone()` is the deep copy) |
| `strands-ts/src/types/__tests__/media.test.ts` | `strands::types::media::tests` | 51/51 | |
| `strands-ts/src/types/__tests__/messages.test.ts` | `strands::types::messages::tests` | 84/86 | `returns them unchanged` (systemPromptFromData with class instances) and `keeps an InjectedBlock passed inside message data an InjectedBlock` are TS-only (class instances inside plain data) |
| `strands-ts/src/types/__tests__/validation.test.ts` | | 0/5 | TS-only: `ensureDefined` guards `null`/`undefined`, which Rust expresses with `Option` |
| `strands-ts/src/vended-interventions/cedar/__tests__/cedar.test.node.ts` | | 0/39 | |
| `strands-ts/src/vended-interventions/hitl/__tests__/hitl.test.ts` | | 0/36 | |
| `strands-ts/src/vended-memory-stores/file-memory-store/__tests__/file-memory-store.test.node.ts` | | 0/52 | |
| `strands-ts/src/vended-plugins/context-injector/__tests__/plugin.test.ts` | | 0/18 | |
| `strands-ts/src/vended-plugins/skills/__tests__/agent-skills.test.node.ts` | | 0/39 | |
| `strands-ts/src/vended-plugins/skills/__tests__/skill.test.node.ts` | | 0/46 | |
| `strands-ts/src/vended-tools/__tests__/paths.test.ts` | | 0/7 | |
| `strands-ts/src/vended-tools/bash/__tests__/bash.test.node.ts` | | 0/46 | |
| `strands-ts/src/vended-tools/edit/__tests__/edit.test.node.ts` | | 0/13 | |
| `strands-ts/src/vended-tools/handoff-to-user/__tests__/handoff-to-user.test.ts` | | 0/9 | |
| `strands-ts/src/vended-tools/http-request/__tests__/http-request.test.ts` | | 0/12 | |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.node.ts` | | 0/1 | |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.ts` | | 0/28 | |
| `strands-ts/src/vended-tools/notebook/__tests__/notebook.test.ts` | | 0/62 | |
| `strands-ts/src/vended-tools/python-repl/__tests__/python-repl.test.node.ts` | | 0/10 | |
| `strands-ts/src/vended-tools/read/__tests__/read.test.node.ts` | | 0/31 | |
| `strands-ts/src/vended-tools/shell/__tests__/shell.test.node.ts` | | 0/18 | |
| `strands-ts/src/vended-tools/sleep/__tests__/sleep.test.ts` | | 0/19 | |
| `strands-ts/src/vended-tools/stop/__tests__/stop.test.ts` | | 0/18 | |
| `strands-ts/src/vended-tools/subagent/__tests__/subagent.test.ts` | | 0/27 | |
| `strands-ts/src/vended-tools/web-fetch/__tests__/extract.test.ts` | | 0/14 | |
| `strands-ts/src/vended-tools/web-fetch/__tests__/web-fetch.test.ts` | | 0/30 | |
| `strands-ts/src/vended-tools/write/__tests__/write.test.node.ts` | | 0/11 | |

## Out of scope

| TS spec | reason |
|---|---|
| `strands-ts/src/a2a/__tests__/a2a-agent.test.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/a2a/__tests__/adapters.test.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/a2a/__tests__/async-lock.test.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/a2a/__tests__/events.test.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/a2a/__tests__/executor.test.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/a2a/__tests__/server.test.node.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/a2a/__tests__/server.test.ts` | multi-agent and A2A are out of v0.1 (#20) |
| `strands-ts/src/agent/__tests__/agent.checkpoint.test.ts` | checkpointing is out of v0.1 |
| `strands-ts/src/decisions/__tests__/decision-model.test.ts` | decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/llm-decision-model.test.ts` | decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/llm-decision-model.tracing.test.node.ts` | decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/system-one-decision-models.test.node.ts` | decision models are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client-legacy.test.node.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client-stdio.test.node.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client.test.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-transport.test.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.decision-model.test.ts` | decision-model HITL classifier is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/agent-tool-streamer.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/events.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/graph.invocation-context.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/graph.invocation-state.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/graph.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/graph.tracer.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/interrupts.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/nodes.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/plugins.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/snapshot.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/spec.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/state.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-context.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-state.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/swarm.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/multiagent/__tests__/swarm.tracer.test.ts` | multi-agent is out of v0.1 (#20) |
| `strands-ts/src/sandbox/__tests__/docker.test.node.ts` | Docker sandbox is out of v0.1 |
| `strands-ts/src/sandbox/__tests__/ssh.test.node.ts` | SSH sandbox is out of v0.1 |
| `strands-ts/src/session/__tests__/file-storage.test.node.ts` | deprecated session FileStorage (#24) |
| `strands-ts/src/session/__tests__/s3-storage.test.ts` | S3 storage is out of v0.1 |
| `strands-ts/src/storage/__tests__/s3-storage.test.ts` | S3 storage is out of v0.1 |
| `strands-ts/src/storage/embeddings/__tests__/bedrock.test.ts` | embeddings are out of v0.1 |
| `strands-ts/src/storage/search/__tests__/s3-vector.test.ts` | vector search is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/handler.test.ts` | steering is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/llm.test.ts` | steering is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/tool-ledger.test.ts` | steering is out of v0.1 |
| `strands-ts/src/vended-memory-stores/bedrock-knowledge-base/__tests__/bedrock-knowledge-base-store.test.node.ts` | Bedrock Knowledge Base memory store is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/file-storage-sandbox.test.node.ts` | the ContextOffloader plugin is out of v0.1 (#61) |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.node.ts` | the ContextOffloader plugin is out of v0.1 (#61) |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.ts` | the ContextOffloader plugin is out of v0.1 (#61) |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts` | the ContextOffloader plugin is out of v0.1 (#61) |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.node.ts` | the ContextOffloader plugin is out of v0.1 (#61) |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.ts` | the ContextOffloader plugin is out of v0.1 (#61) |
| `strands-ts/src/vended-plugins/goal/__tests__/plugin.test.ts` | GoalLoop is out of v0.1 |
| `strands-ts/src/vended-tools/a2a-client/__tests__/a2a-client.test.ts` | a2a_client tool is out of v0.1 |
| `strands-ts/src/vended-tools/file-editor/__tests__/file-editor.test.node.ts` | deprecated fileEditor (#24) |
| `strands-ts/src/vended-tools/graph/__tests__/graph.test.ts` | graph tool is out of v0.1 |
| `strands-ts/src/vended-tools/swarm/__tests__/swarm.test.ts` | swarm tool is out of v0.1 |
| `strands-ts/src/storage/search/__tests__/qmd.test.node.ts` | QMD (SQLite BM25 via `@tobilu/qmd`) is out of v0.1; `KeywordSearchStrategy` is the v0.1 search strategy |
| `strands-ts/src/vended-memory-stores/test-memory-store/__tests__/test-memory-store.test.node.ts` | `TestMemoryStore` is not in the v0.1 scope; `FileMemoryStore` is the v0.1 local store |
