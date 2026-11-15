# Parity - Rust SDK

This file maps each TypeScript spec file in `strands-ts/` to the Rust tests that port it. The TypeScript tests are the behavioural spec (see [TESTING.md](TESTING.md)).

Columns: `TS spec` is the TypeScript test file, `Rust tests` is where its ported tests live, `ported/total` counts ported cases against the TypeScript `it(...)` and `test(...)` cases in that file (an `it.each` table counts as one case), and `gaps` gives the reason for every case that isn't ported. A row whose gap reason covers the whole file is not ported at all: out of scope, a TypeScript-only mechanism, or deprecated.

Summary: 150 spec files in scope with 4223 cases, plus 64 files out of scope. The live checklist has 35 files in scope with 270 cases, plus 24 out of scope.

## Unit specs

Spec files under `strands-ts/src/**/__tests__/`, including the Node-only `*.test.node.ts` files.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/__tests__/default-slot.test.ts` | - | 0/4 | - |
| `strands-ts/src/__tests__/errors.test.ts` | - | 0/29 | - |
| `strands-ts/src/__tests__/index.test.ts` | - | 0/9 | - |
| `strands-ts/src/__tests__/interrupt.test.ts` | - | 0/26 | - |
| `strands-ts/src/__tests__/mime.test.ts` | - | 0/6 | - |
| `strands-ts/src/__tests__/queue.test.ts` | - | 0/10 | - |
| `strands-ts/src/__tests__/state-store.test.ts` | - | 0/49 | - |
| `strands-ts/src/a2a/__tests__/a2a-agent.test.ts` | - | 0/22 | out of scope: A2A |
| `strands-ts/src/a2a/__tests__/adapters.test.ts` | - | 0/11 | out of scope: A2A |
| `strands-ts/src/a2a/__tests__/async-lock.test.ts` | - | 0/3 | out of scope: A2A |
| `strands-ts/src/a2a/__tests__/events.test.ts` | - | 0/6 | out of scope: A2A |
| `strands-ts/src/a2a/__tests__/executor.test.ts` | - | 0/16 | out of scope: A2A |
| `strands-ts/src/a2a/__tests__/server.test.node.ts` | - | 0/6 | out of scope: A2A |
| `strands-ts/src/a2a/__tests__/server.test.ts` | - | 0/6 | out of scope: A2A |
| `strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts` | - | 0/15 | - |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-context.test.ts` | - | 0/3 | - |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-state.test.ts` | - | 0/1 | - |
| `strands-ts/src/agent/__tests__/agent-as-tool.test.ts` | - | 0/31 | - |
| `strands-ts/src/agent/__tests__/agent-delegation.test.ts` | - | 0/17 | - |
| `strands-ts/src/agent/__tests__/agent.cancel.test.ts` | - | 0/28 | - |
| `strands-ts/src/agent/__tests__/agent.checkpoint.test.ts` | - | 0/15 | out of scope: checkpointing |
| `strands-ts/src/agent/__tests__/agent.concurrent.test.ts` | - | 0/22 | - |
| `strands-ts/src/agent/__tests__/agent.context-manager.test.ts` | - | 0/16 | - |
| `strands-ts/src/agent/__tests__/agent.continuation.test.ts` | - | 0/6 | - |
| `strands-ts/src/agent/__tests__/agent.hook.test.ts` | - | 0/70 | - |
| `strands-ts/src/agent/__tests__/agent.interrupt.test.ts` | - | 0/32 | - |
| `strands-ts/src/agent/__tests__/agent.invocation-context.test.ts` | - | 0/16 | - |
| `strands-ts/src/agent/__tests__/agent.invocation-state.test.ts` | - | 0/11 | - |
| `strands-ts/src/agent/__tests__/agent.model-retry.test.ts` | - | 0/12 | - |
| `strands-ts/src/agent/__tests__/agent.model-routing.test.ts` | - | 0/10 | - |
| `strands-ts/src/agent/__tests__/agent.stateful-model.test.ts` | - | 0/12 | - |
| `strands-ts/src/agent/__tests__/agent.storage.test.ts` | - | 0/7 | - |
| `strands-ts/src/agent/__tests__/agent.test.ts` | - | 0/147 | - |
| `strands-ts/src/agent/__tests__/agent.tracer.test.node.ts` | - | 0/47 | - |
| `strands-ts/src/agent/__tests__/invocation-context.test.ts` | - | 0/2 | - |
| `strands-ts/src/agent/__tests__/printer.test.ts` | - | 0/9 | - |
| `strands-ts/src/agent/__tests__/snapshot.test.ts` | - | 0/32 | - |
| `strands-ts/src/agent/__tests__/tool-caller.test.ts` | - | 0/26 | - |
| `strands-ts/src/background-tasks/__tests__/background-tasks.test.ts` | - | 0/9 | - |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-execution.test.ts` | - | 0/5 | - |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-transitions.test.ts` | - | 0/7 | - |
| `strands-ts/src/background-tasks/in-process/__tests__/manager.test.ts` | - | 0/9 | - |
| `strands-ts/src/context-manager/__tests__/context-manager.test.ts` | - | 0/20 | - |
| `strands-ts/src/context-manager/__tests__/presets.test.ts` | - | 0/17 | - |
| `strands-ts/src/context-manager/__tests__/retrieval-tool.test.ts` | - | 0/9 | - |
| `strands-ts/src/context-manager/__tests__/stash-integration.test.ts` | - | 0/17 | - |
| `strands-ts/src/context-manager/__tests__/stash.test.ts` | - | 0/16 | - |
| `strands-ts/src/context-manager/strategies/__tests__/offload-strategy.test.ts` | - | 0/54 | - |
| `strands-ts/src/context-manager/strategies/__tests__/summarize-strategy.test.ts` | - | 0/54 | - |
| `strands-ts/src/conversation-manager/__tests__/agentic-context.test.ts` | - | 0/30 | - |
| `strands-ts/src/conversation-manager/__tests__/context-compression.test.ts` | - | 0/32 | - |
| `strands-ts/src/conversation-manager/__tests__/conversation-manager.test.ts` | - | 0/19 | - |
| `strands-ts/src/conversation-manager/__tests__/null-conversation-manager.test.ts` | - | 0/4 | - |
| `strands-ts/src/conversation-manager/__tests__/pin.test.ts` | - | 0/16 | - |
| `strands-ts/src/conversation-manager/__tests__/sliding-window-conversation-manager.test.ts` | - | 0/65 | - |
| `strands-ts/src/conversation-manager/__tests__/summarizing-conversation-manager.test.ts` | - | 0/20 | - |
| `strands-ts/src/conversation-manager/__tests__/token-usage-middleware.test.ts` | - | 0/15 | - |
| `strands-ts/src/decisions/__tests__/decision-model.test.ts` | - | 0/8 | out of scope: decision models |
| `strands-ts/src/decisions/__tests__/llm-decision-model.test.ts` | - | 0/24 | out of scope: decision models |
| `strands-ts/src/decisions/__tests__/llm-decision-model.tracing.test.node.ts` | - | 0/2 | out of scope: decision models |
| `strands-ts/src/decisions/__tests__/system-one-decision-models.test.node.ts` | - | 0/23 | out of scope: decision models |
| `strands-ts/src/experimental/__tests__/checkpoint.test.ts` | - | 0/6 | out of scope: checkpointing |
| `strands-ts/src/hooks/__tests__/events.test.ts` | - | 0/71 | - |
| `strands-ts/src/hooks/__tests__/registry.test.ts` | - | 0/25 | - |
| `strands-ts/src/injection/__tests__/message-injection.test.ts` | - | 0/31 | - |
| `strands-ts/src/interventions/__tests__/handler.test.ts` | - | 0/2 | - |
| `strands-ts/src/interventions/__tests__/registry.test.ts` | - | 0/44 | - |
| `strands-ts/src/logging/__tests__/logger.test.ts` | - | 0/6 | - |
| `strands-ts/src/logging/__tests__/warn-once.test.ts` | - | 0/3 | - |
| `strands-ts/src/mcp/__tests__/client-annotations.test.ts` | - | 0/4 | - |
| `strands-ts/src/mcp/__tests__/client-progress.test.ts` | - | 0/2 | - |
| `strands-ts/src/mcp/__tests__/client.test.ts` | - | 0/126 | - |
| `strands-ts/src/mcp/__tests__/config.test.browser.ts` | - | 0/1 | TS-only mechanism: browser runtime build |
| `strands-ts/src/mcp/__tests__/config.test.node.ts` | - | 0/48 | - |
| `strands-ts/src/mcp/__tests__/task-client-legacy.test.node.ts` | - | 0/1 | out of scope: MCP tasks |
| `strands-ts/src/mcp/__tests__/task-client-stdio.test.node.ts` | - | 0/1 | out of scope: MCP tasks |
| `strands-ts/src/mcp/__tests__/task-client.test.ts` | - | 0/34 | out of scope: MCP tasks |
| `strands-ts/src/mcp/__tests__/task-transport.test.ts` | - | 0/12 | out of scope: MCP tasks |
| `strands-ts/src/memory/__tests__/memory-manager.test.ts` | - | 0/81 | - |
| `strands-ts/src/memory/__tests__/memory-tracing.test.ts` | - | 0/17 | - |
| `strands-ts/src/memory/extraction/__tests__/extraction.test.ts` | - | 0/42 | - |
| `strands-ts/src/memory/extraction/__tests__/model-extractor.test.ts` | - | 0/14 | - |
| `strands-ts/src/memory/extraction/__tests__/resolve-extraction-config.test.ts` | - | 0/13 | - |
| `strands-ts/src/middleware/__tests__/agent-middleware.test.ts` | - | 0/62 | - |
| `strands-ts/src/middleware/__tests__/copy-on-input.test.ts` | - | 0/21 | - |
| `strands-ts/src/middleware/__tests__/custom-stages.test.ts` | - | 0/5 | - |
| `strands-ts/src/middleware/__tests__/middleware-interrupts.test.ts` | - | 0/15 | - |
| `strands-ts/src/middleware/__tests__/registry.test.ts` | - | 0/24 | - |
| `strands-ts/src/models/__tests__/anthropic.test.ts` | - | 0/186 | - |
| `strands-ts/src/models/__tests__/bedrock.test.ts` | - | 0/274 | - |
| `strands-ts/src/models/__tests__/bedrock.transport.test.browser.ts` | - | 0/2 | TS-only mechanism: browser runtime build |
| `strands-ts/src/models/__tests__/cache-portability.test.ts` | - | 0/1 | - |
| `strands-ts/src/models/__tests__/defaults.test.ts` | - | 0/7 | - |
| `strands-ts/src/models/__tests__/google.test.ts` | - | 0/87 | out of scope: Google provider |
| `strands-ts/src/models/__tests__/model-info-lookup.test.ts` | - | 0/9 | - |
| `strands-ts/src/models/__tests__/model-info.test.node.ts` | - | 0/1 | - |
| `strands-ts/src/models/__tests__/model-info.test.ts` | - | 0/5 | - |
| `strands-ts/src/models/__tests__/model-proxy.test.ts` | - | 0/19 | - |
| `strands-ts/src/models/__tests__/model.test.ts` | - | 0/68 | - |
| `strands-ts/src/models/__tests__/native-output.test.ts` | - | 0/12 | - |
| `strands-ts/src/models/__tests__/server-side-summary-capability.test.ts` | - | 0/1 | - |
| `strands-ts/src/models/__tests__/streaming.test.ts` | - | 0/14 | - |
| `strands-ts/src/models/__tests__/strict-schema.test.ts` | - | 0/24 | - |
| `strands-ts/src/models/__tests__/vercel.test.ts` | - | 0/104 | out of scope: Vercel provider |
| `strands-ts/src/models/openai/__tests__/chat.test.ts` | - | 0/96 | - |
| `strands-ts/src/models/openai/__tests__/errors.test.ts` | - | 0/7 | - |
| `strands-ts/src/models/openai/__tests__/mantle.test.ts` | - | 0/30 | - |
| `strands-ts/src/models/openai/__tests__/responses-cache.test.ts` | - | 0/17 | - |
| `strands-ts/src/models/openai/__tests__/responses.test.ts` | - | 0/72 | - |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.decision-model.test.ts` | - | 0/21 | out of scope: the decision-model classifier |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.test.ts` | - | 0/25 | - |
| `strands-ts/src/models/routing/__tests__/fallback-strategy.test.ts` | - | 0/5 | - |
| `strands-ts/src/models/routing/__tests__/router.test.ts` | - | 0/22 | - |
| `strands-ts/src/multiagent/__tests__/agent-tool-streamer.test.ts` | - | 0/11 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/events.test.ts` | - | 0/38 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/graph.invocation-context.test.ts` | - | 0/10 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/graph.invocation-state.test.ts` | - | 0/3 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/graph.test.ts` | - | 0/54 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/graph.tracer.test.ts` | - | 0/13 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/interrupts.test.ts` | - | 0/11 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/nodes.test.ts` | - | 0/23 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/plugins.test.ts` | - | 0/1 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/snapshot.test.ts` | - | 0/9 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/spec.test.ts` | - | 0/26 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/state.test.ts` | - | 0/44 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-context.test.ts` | - | 0/6 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-state.test.ts` | - | 0/2 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/swarm.test.ts` | - | 0/47 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/multiagent/__tests__/swarm.tracer.test.ts` | - | 0/13 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/src/plugins/__tests__/plugin.test.ts` | - | 0/7 | - |
| `strands-ts/src/plugins/__tests__/registry.test.ts` | - | 0/10 | - |
| `strands-ts/src/registry/__tests__/tool-registry.test.ts` | - | 0/33 | - |
| `strands-ts/src/retry/__tests__/backoff-strategy.test.ts` | - | 0/19 | - |
| `strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts` | - | 0/15 | - |
| `strands-ts/src/sandbox/__tests__/default.test.browser.ts` | - | 0/1 | TS-only mechanism: browser runtime build |
| `strands-ts/src/sandbox/__tests__/default.test.node.ts` | - | 0/4 | - |
| `strands-ts/src/sandbox/__tests__/docker.test.node.ts` | - | 0/9 | out of scope: Docker and SSH sandboxes |
| `strands-ts/src/sandbox/__tests__/errors.test.node.ts` | - | 0/9 | - |
| `strands-ts/src/sandbox/__tests__/not-a-sandbox-local-environment.test.node.ts` | - | 0/27 | - |
| `strands-ts/src/sandbox/__tests__/posix-shell.test.node.ts` | - | 0/55 | - |
| `strands-ts/src/sandbox/__tests__/read-file-errors.test.node.ts` | - | 0/6 | - |
| `strands-ts/src/sandbox/__tests__/ssh.test.node.ts` | - | 0/21 | out of scope: Docker and SSH sandboxes |
| `strands-ts/src/sandbox/__tests__/stream-process.test.node.ts` | - | 0/5 | - |
| `strands-ts/src/session/__tests__/file-storage.test.node.ts` | - | 0/26 | deprecated: session `FileStorage` (replaced by `LocalFileStorage`) |
| `strands-ts/src/session/__tests__/s3-storage.test.ts` | - | 0/35 | out of scope: S3 storage |
| `strands-ts/src/session/__tests__/session-manager.test.ts` | - | 0/71 | - |
| `strands-ts/src/session/__tests__/snapshot-storage-adapter.test.ts` | - | 0/24 | - |
| `strands-ts/src/session/__tests__/validation.test.ts` | - | 0/10 | - |
| `strands-ts/src/storage/__tests__/in-memory-storage.test.ts` | - | 0/25 | - |
| `strands-ts/src/storage/__tests__/local-file-storage.test-d.ts` | - | 0/4 | TS-only mechanism: compile-time type assertions for the out-of-scope QMD strategy |
| `strands-ts/src/storage/__tests__/local-file-storage.test.node.ts` | - | 0/31 | - |
| `strands-ts/src/storage/__tests__/namespaced-storage.test.ts` | - | 0/10 | - |
| `strands-ts/src/storage/__tests__/s3-storage.test.ts` | - | 0/27 | out of scope: S3 storage |
| `strands-ts/src/storage/embeddings/__tests__/bedrock.test.ts` | - | 0/9 | out of scope: embeddings |
| `strands-ts/src/storage/search/__tests__/keyword.test.ts` | - | 0/13 | - |
| `strands-ts/src/storage/search/__tests__/qmd.test.node.ts` | - | 0/16 | out of scope: QMD search needs the Node-only `@tobilu/qmd` peer dependency |
| `strands-ts/src/storage/search/__tests__/s3-vector.test.ts` | - | 0/19 | out of scope: vector search |
| `strands-ts/src/telemetry/__tests__/config.test.node.ts` | - | 0/18 | - |
| `strands-ts/src/telemetry/__tests__/config.test.ts` | - | 0/6 | - |
| `strands-ts/src/telemetry/__tests__/json.test.ts` | - | 0/15 | - |
| `strands-ts/src/telemetry/__tests__/local-trace.test.ts` | - | 0/16 | - |
| `strands-ts/src/telemetry/__tests__/meter.test.ts` | - | 0/62 | - |
| `strands-ts/src/telemetry/__tests__/tracer.test.node.ts` | - | 0/100 | - |
| `strands-ts/src/tools/__tests__/structured-output-tool.test.ts` | - | 0/12 | - |
| `strands-ts/src/tools/__tests__/tool-factory.test.ts` | - | 0/8 | - |
| `strands-ts/src/tools/__tests__/tool.test.ts` | - | 0/46 | - |
| `strands-ts/src/tools/__tests__/zod-tool.test-d.ts` | - | 0/20 | TS-only mechanism: compile-time type assertions on Zod schema inference |
| `strands-ts/src/tools/__tests__/zod-tool.test.ts` | - | 0/32 | - |
| `strands-ts/src/tools/executors/__tests__/executor.cancel-signal.test.ts` | - | 0/3 | - |
| `strands-ts/src/types/__tests__/agent.test.ts` | - | 0/30 | - |
| `strands-ts/src/types/__tests__/citations.test.ts` | - | 0/6 | - |
| `strands-ts/src/types/__tests__/json.test.ts` | - | 0/44 | - |
| `strands-ts/src/types/__tests__/media.test.ts` | - | 0/51 | - |
| `strands-ts/src/types/__tests__/messages.test.ts` | - | 0/86 | - |
| `strands-ts/src/types/__tests__/validation.test.ts` | - | 0/5 | - |
| `strands-ts/src/vended-interventions/cedar/__tests__/cedar.test.node.ts` | - | 0/39 | - |
| `strands-ts/src/vended-interventions/hitl/__tests__/hitl.test.ts` | - | 0/36 | - |
| `strands-ts/src/vended-interventions/steering/__tests__/handler.test.ts` | - | 0/7 | out of scope: steering |
| `strands-ts/src/vended-interventions/steering/__tests__/llm.test.ts` | - | 0/5 | out of scope: steering |
| `strands-ts/src/vended-interventions/steering/__tests__/tool-ledger.test.ts` | - | 0/4 | out of scope: steering |
| `strands-ts/src/vended-memory-stores/bedrock-knowledge-base/__tests__/bedrock-knowledge-base-store.test.node.ts` | - | 0/84 | out of scope: Bedrock Knowledge Base store |
| `strands-ts/src/vended-memory-stores/file-memory-store/__tests__/file-memory-store.test.node.ts` | - | 0/52 | - |
| `strands-ts/src/vended-memory-stores/test-memory-store/__tests__/test-memory-store.test.node.ts` | - | 0/36 | - |
| `strands-ts/src/vended-plugins/context-injector/__tests__/plugin.test.ts` | - | 0/19 | - |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/file-storage-sandbox.test.node.ts` | - | 0/12 | out of scope: context offloader plugin |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.node.ts` | - | 0/1 | out of scope: context offloader plugin |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.ts` | - | 0/51 | out of scope: context offloader plugin |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts` | - | 0/26 | out of scope: context offloader plugin |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.node.ts` | - | 0/14 | out of scope: context offloader plugin |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.ts` | - | 0/32 | out of scope: context offloader plugin |
| `strands-ts/src/vended-plugins/goal/__tests__/plugin.test.ts` | - | 0/40 | out of scope: goal loop |
| `strands-ts/src/vended-plugins/skills/__tests__/agent-skills.test.node.ts` | - | 0/39 | - |
| `strands-ts/src/vended-plugins/skills/__tests__/skill.test.node.ts` | - | 0/46 | - |
| `strands-ts/src/vended-tools/__tests__/paths.test.ts` | - | 0/7 | - |
| `strands-ts/src/vended-tools/a2a-client/__tests__/a2a-client.test.ts` | - | 0/16 | out of scope: graph, swarm and A2A tools |
| `strands-ts/src/vended-tools/bash/__tests__/bash.test.node.ts` | - | 0/46 | - |
| `strands-ts/src/vended-tools/edit/__tests__/edit.test.node.ts` | - | 0/13 | - |
| `strands-ts/src/vended-tools/file-editor/__tests__/file-editor.test.node.ts` | - | 0/67 | deprecated: `fileEditor` (replaced by `read`, `write` and `edit`) |
| `strands-ts/src/vended-tools/graph/__tests__/graph.test.ts` | - | 0/15 | out of scope: graph, swarm and A2A tools |
| `strands-ts/src/vended-tools/handoff-to-user/__tests__/handoff-to-user.test.ts` | - | 0/9 | - |
| `strands-ts/src/vended-tools/http-request/__tests__/http-request.test.ts` | - | 0/12 | - |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.node.ts` | - | 0/1 | - |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.ts` | - | 0/28 | - |
| `strands-ts/src/vended-tools/notebook/__tests__/notebook.test.ts` | - | 0/62 | - |
| `strands-ts/src/vended-tools/python-repl/__tests__/python-repl.test.node.ts` | - | 0/12 | - |
| `strands-ts/src/vended-tools/read/__tests__/read.test.node.ts` | - | 0/34 | - |
| `strands-ts/src/vended-tools/shell/__tests__/shell.test.node.ts` | - | 0/18 | - |
| `strands-ts/src/vended-tools/sleep/__tests__/sleep.test.ts` | - | 0/19 | - |
| `strands-ts/src/vended-tools/stop/__tests__/stop.test.ts` | - | 0/18 | - |
| `strands-ts/src/vended-tools/subagent/__tests__/subagent.test.ts` | - | 0/33 | - |
| `strands-ts/src/vended-tools/swarm/__tests__/swarm.test.ts` | - | 0/14 | out of scope: graph, swarm and A2A tools |
| `strands-ts/src/vended-tools/web-fetch/__tests__/extract.test.ts` | - | 0/15 | - |
| `strands-ts/src/vended-tools/web-fetch/__tests__/web-fetch.test.ts` | - | 0/30 | - |
| `strands-ts/src/vended-tools/write/__tests__/write.test.node.ts` | - | 0/11 | - |

## Live checklist

Integration tests under `strands-ts/test/integ/`, ported as `live_*` tests (see [TESTING.md](TESTING.md#live-tests)).

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/test/integ/a2a/a2a-agent.test.ts` | - | 0/2 | out of scope: A2A |
| `strands-ts/test/integ/a2a/express-server.test.node.ts` | - | 0/4 | out of scope: A2A |
| `strands-ts/test/integ/agent-as-tool.test.ts` | - | 0/1 | - |
| `strands-ts/test/integ/agent.cancel.test.ts` | - | 0/5 | - |
| `strands-ts/test/integ/agent.checkpoint.test.node.ts` | - | 0/3 | out of scope: checkpointing |
| `strands-ts/test/integ/agent.test.ts` | - | 0/18 | - |
| `strands-ts/test/integ/cedar.test.node.ts` | - | 0/3 | - |
| `strands-ts/test/integ/conversation-manager/agentic-context.test.ts` | - | 0/11 | - |
| `strands-ts/test/integ/conversation-manager/context-manager-message-reduction.test.ts` | - | 0/1 | - |
| `strands-ts/test/integ/conversation-manager/context-manager-summarize-tool-calls.test.ts` | - | 0/1 | - |
| `strands-ts/test/integ/conversation-manager/summarizing-conversation-manager.test.ts` | - | 0/2 | - |
| `strands-ts/test/integ/decisions/decision-model.test.ts` | - | 0/2 | out of scope: decision models |
| `strands-ts/test/integ/decisions/system-one-decision-models.test.node.ts` | - | 0/1 | out of scope: decision models |
| `strands-ts/test/integ/environment.test.browser.ts` | - | 0/5 | TS-only mechanism: browser runtime build |
| `strands-ts/test/integ/environment.test.node.ts` | - | 0/2 | - |
| `strands-ts/test/integ/environment.test.ts` | - | 0/3 | - |
| `strands-ts/test/integ/goal/goal.test.node.ts` | - | 0/2 | out of scope: goal loop |
| `strands-ts/test/integ/interrupt.test.ts` | - | 0/3 | - |
| `strands-ts/test/integ/interventions.test.ts` | - | 0/52 | - |
| `strands-ts/test/integ/mcp/mcp-connect-abort.test.node.ts` | - | 0/1 | - |
| `strands-ts/test/integ/mcp/mcp-legacy-fallback.test.node.ts` | - | 0/4 | - |
| `strands-ts/test/integ/mcp/mcp-reconnect.test.node.ts` | - | 0/3 | - |
| `strands-ts/test/integ/mcp/mcp-sep2663.test.node.ts` | - | 0/10 | - |
| `strands-ts/test/integ/mcp/mcp-tasks-stdio.test.node.ts` | - | 0/2 | out of scope: MCP tasks |
| `strands-ts/test/integ/mcp/mcp-tasks.test.node.ts` | - | 0/8 | out of scope: MCP tasks |
| `strands-ts/test/integ/mcp/mcp-v2.test.node.ts` | - | 0/7 | - |
| `strands-ts/test/integ/mcp/mcp.test.node.ts` | - | 0/7 | - |
| `strands-ts/test/integ/memory/bedrock-knowledge-base-memory-manager.test.node.ts` | - | 0/8 | out of scope: Bedrock Knowledge Base store |
| `strands-ts/test/integ/memory/bedrock-knowledge-base-store.test.node.ts` | - | 0/13 | out of scope: Bedrock Knowledge Base store |
| `strands-ts/test/integ/models/anthropic.test.ts` | - | 0/13 | - |
| `strands-ts/test/integ/models/bedrock.test.node.ts` | - | 0/3 | - |
| `strands-ts/test/integ/models/bedrock.test.ts` | - | 0/29 | - |
| `strands-ts/test/integ/models/classifier-strategy.test.ts` | - | 0/1 | - |
| `strands-ts/test/integ/models/context-windows.test.ts` | - | 0/1 | - |
| `strands-ts/test/integ/models/google.test.ts` | - | 0/7 | out of scope: Google provider |
| `strands-ts/test/integ/models/model-smoke.test.ts` | - | 0/2 | - |
| `strands-ts/test/integ/models/openai/chat.test.ts` | - | 0/7 | - |
| `strands-ts/test/integ/models/openai/mantle-routing.test.node.ts` | - | 0/1 | - |
| `strands-ts/test/integ/models/openai/mantle.test.node.ts` | - | 0/4 | - |
| `strands-ts/test/integ/models/openai/responses.test.ts` | - | 0/12 | - |
| `strands-ts/test/integ/models/openai/session-cache-key.test.ts` | - | 0/6 | - |
| `strands-ts/test/integ/multiagent/graph.test.ts` | - | 0/5 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/test/integ/multiagent/interrupt-hook.test.node.ts` | - | 0/3 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/test/integ/multiagent/interrupt-node.test.node.ts` | - | 0/2 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/test/integ/multiagent/interrupt-session.test.node.ts` | - | 0/1 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/test/integ/multiagent/session-manager.test.node.ts` | - | 0/3 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/test/integ/multiagent/swarm.test.ts` | - | 0/2 | out of scope: multi-agent (graph, swarm) |
| `strands-ts/test/integ/qmd-search-strategy.test.node.ts` | - | 0/3 | out of scope: QMD search needs the Node-only `@tobilu/qmd` peer dependency |
| `strands-ts/test/integ/s3-vector-search-strategy.test.node.ts` | - | 0/3 | out of scope: vector search |
| `strands-ts/test/integ/sandbox/docker.test.node.ts` | - | 0/11 | out of scope: Docker and SSH sandboxes |
| `strands-ts/test/integ/sandbox/ssh.test.node.ts` | - | 0/20 | out of scope: Docker and SSH sandboxes |
| `strands-ts/test/integ/session-manager.test.node.ts` | - | 0/9 | - |
| `strands-ts/test/integ/skills/agent-skills.test.node.ts` | - | 0/4 | - |
| `strands-ts/test/integ/telemetry.test.node.ts` | - | 0/36 | - |
| `strands-ts/test/integ/tools/bash.test.node.ts` | - | 0/4 | - |
| `strands-ts/test/integ/tools/file-editor.test.node.ts` | - | 0/7 | deprecated: `fileEditor` (replaced by `read`, `write` and `edit`) |
| `strands-ts/test/integ/tools/http-request.test.ts` | - | 0/1 | - |
| `strands-ts/test/integ/tools/notebook.test.ts` | - | 0/3 | - |
| `strands-ts/test/integ/vended-interventions/steering/steering.test.node.ts` | - | 0/2 | out of scope: steering |
