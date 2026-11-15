# Test parity - Strands Rust SDK

This file tracks how the TypeScript SDK test suite, the behavioural spec for this crate, is ported to Rust. Paths are relative to `strands-ts/src/`. Both `*.test.ts` and node-only `*.test.node.ts` specs count; browser-only `*.test.browser.ts` specs don't apply. See [TESTING.md](TESTING.md) for how tests are written and named.

Each row is one TypeScript spec file at commit `7296caba7`, the TypeScript revision this port follows. `Rust tests` names the Rust test module or file that ports it. `ported/total` counts the TypeScript cases ported against the cases in the file: `total` counts the `it(...)` and `test(...)` call sites, including `it.each`, `it.for`, `it.skipIf` and `it.runIf`. A parameterised case (`it.each`, `it.for`) counts as one until it is ported; its row then counts each table entry. A case that is not ported is listed under `gaps` with its reason (out of scope, a TypeScript-only mechanism, or deprecated). Rows marked `n/a` are out of scope for v0.1 as a whole.

## Root modules

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `__tests__/default-slot.test.ts` | - | 0/4 | - |
| `__tests__/errors.test.ts` | `error::tests` | 20/29 | `isFetchNetworkError` (3 cases) and `normalizeError` (6 cases): TypeScript-only mechanism, they classify untyped JavaScript values and are not exported |
| `__tests__/index.test.ts` | - | 0/9 | - |
| `__tests__/interrupt.test.ts` | `interrupt::tests` | 7/26 | `InterruptError` `creates catchable error with single interrupt` and `creates error with multiple interrupts`: TypeScript-only mechanism, interrupts are not errors |
| `__tests__/mime.test.ts` | `mime::tests` | 52/52 | - |
| `__tests__/queue.test.ts` | - | 0/10 | - |
| `__tests__/state-store.test.ts` | `state_store::tests` | 31/49 | 18 cases: TypeScript-only mechanism, functions, symbols and `undefined` in state, a `null` key, the symbol serialization helpers and `isStateSerializable` guard JavaScript values that `JsonValue` can't hold |

## Types

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `types/__tests__/agent.test.ts` | - | 0/30 | `defaults checkpoint to undefined`, `accepts a checkpoint`, `includes the serialized checkpoint in toJSON when present`, `omits checkpoint from toJSON when absent`: out of scope, checkpointing is not in v0.1 |
| `types/__tests__/citations.test.ts` | `types::citations::tests` | 5/6 | `creates block with correct type discriminator`: TypeScript-only mechanism, the `type` discriminator is a TypeScript runtime field |
| `types/__tests__/json.test.ts` | - | 0/44 | all 44 cases: TypeScript-only mechanism, `deepCopy` guards JavaScript values that JSON can't hold; `JsonValue::clone` is already a deep copy |
| `types/__tests__/media.test.ts` | `types::media::tests` | 51/51 | - |
| `types/__tests__/messages.test.ts` | `types::messages::tests` | 105/105 | - |
| `types/__tests__/validation.test.ts` | `types::validation::tests` | 4/5 | `when value is null throws error with field name`: TypeScript-only mechanism, Rust has one `None` for TypeScript `null` and `undefined` |

## Models

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `models/__tests__/anthropic.test.ts` | - | 0/186 | - |
| `models/__tests__/bedrock.test.ts` | - | 0/275 | - |
| `models/__tests__/cache-portability.test.ts` | `models::model::tests` | 7/7 | - |
| `models/__tests__/defaults.test.ts` | `models::defaults::tests` | 7/7 | - |
| `models/__tests__/google.test.ts` | n/a | n/a | out of scope: the Google and Vercel providers are not in v0.1 |
| `models/__tests__/model-info-lookup.test.ts` | `models::model_info::tests` | 9/9 | - |
| `models/__tests__/model-info.test.node.ts` | `models::model_info::tests` | 1/1 | - |
| `models/__tests__/model-info.test.ts` | `models::model_info::tests` | 4/5 | `returns a frozen record`: TypeScript-only mechanism, Rust values are immutable without `Object.freeze` |
| `models/__tests__/model-proxy.test.ts` | - | 0/19 | - |
| `models/__tests__/model.test.ts` | `models::model::tests`, `models::summarize::tests` | 72/72 | - |
| `models/__tests__/native-output.test.ts` | `models::native_output::tests` | 37/37 | - |
| `models/__tests__/server-side-summary-capability.test.ts` | - | 0/2 | - |
| `models/__tests__/streaming.test.ts` | `models::streaming::tests` | 14/14 | - |
| `models/__tests__/strict-schema.test.ts` | `models::strict_schema::tests` | 24/24 | - |
| `models/__tests__/vercel.test.ts` | n/a | n/a | out of scope: the Google and Vercel providers are not in v0.1 |
| `models/openai/__tests__/chat.test.ts` | - | 0/96 | - |
| `models/openai/__tests__/errors.test.ts` | - | 0/7 | - |
| `models/openai/__tests__/mantle.test.ts` | - | 0/30 | - |
| `models/openai/__tests__/responses-cache.test.ts` | - | 0/17 | - |
| `models/openai/__tests__/responses.test.ts` | - | 0/72 | - |

## Model routing

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `models/routing/__tests__/classifier-strategy.decision-model.test.ts` | n/a | n/a | out of scope: the decision-model classifier is not in v0.1 |
| `models/routing/__tests__/classifier-strategy.test.ts` | - | 0/25 | `routes a Model the same way as an LLMDecisionModel wrapping it`, `rejects a value that is neither a Model nor a DecisionModel`: out of scope, decision models are not in v0.1 |
| `models/routing/__tests__/fallback-strategy.test.ts` | - | 0/5 | - |
| `models/routing/__tests__/router.test.ts` | - | 0/22 | - |

## Retry

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `retry/__tests__/backoff-strategy.test.ts` | `retry::backoff_strategy::tests` | 15/19 | the four `rejects ...` cases: TypeScript-only mechanism, `BackoffContext::attempt` is a `NonZeroU32`, so an invalid count can't be built |
| `retry/__tests__/default-model-retry-strategy.test.ts` | `retry::default_model_retry_strategy::tests` | 12/15 | `rejects maxAttempts below 1`: TypeScript-only mechanism, `max_attempts` is a `NonZeroU32` |

## Logging

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `logging/__tests__/logger.test.ts` | - | 0/6 | `allows custom logger injection`, `passes multiple arguments to logger`: out of scope, `configureLogging` is replaced by `tracing` subscribers |
| `logging/__tests__/warn-once.test.ts` | `logging::tests` | 3/3 | - |

## Agent

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `agent/__tests__/agent-as-tool.interrupt.test.ts` | - | 0/15 | - |
| `agent/__tests__/agent-as-tool.invocation-context.test.ts` | - | 0/3 | - |
| `agent/__tests__/agent-as-tool.invocation-state.test.ts` | - | 0/1 | - |
| `agent/__tests__/agent-as-tool.test.ts` | - | 0/31 | - |
| `agent/__tests__/agent-delegation.test.ts` | - | 0/17 | - |
| `agent/__tests__/agent.cancel.test.ts` | - | 0/28 | - |
| `agent/__tests__/agent.checkpoint.test.ts` | n/a | n/a | out of scope: checkpointing is not in v0.1 |
| `agent/__tests__/agent.concurrent.test.ts` | - | 0/22 | - |
| `agent/__tests__/agent.context-manager.test.ts` | - | 0/16 | - |
| `agent/__tests__/agent.continuation.test.ts` | - | 0/6 | - |
| `agent/__tests__/agent.hook.test.ts` | - | 0/70 | - |
| `agent/__tests__/agent.interrupt.test.ts` | - | 0/32 | - |
| `agent/__tests__/agent.invocation-context.test.ts` | - | 0/16 | - |
| `agent/__tests__/agent.invocation-state.test.ts` | - | 0/11 | - |
| `agent/__tests__/agent.model-retry.test.ts` | - | 0/12 | - |
| `agent/__tests__/agent.model-routing.test.ts` | - | 0/10 | - |
| `agent/__tests__/agent.stateful-model.test.ts` | - | 0/12 | - |
| `agent/__tests__/agent.storage.test.ts` | - | 0/7 | - |
| `agent/__tests__/agent.test.ts` | - | 0/147 | `counts a failed attempt on a turn that pauses at a checkpoint`: out of scope, checkpointing is not in v0.1 |
| `agent/__tests__/agent.tracer.test.node.ts` | - | 0/47 | - |
| `agent/__tests__/invocation-context.test.ts` | - | 0/2 | - |
| `agent/__tests__/printer.test.ts` | - | 0/9 | - |
| `agent/__tests__/snapshot.test.ts` | - | 0/33 | - |
| `agent/__tests__/tool-caller.test.ts` | - | 0/26 | - |

## Hooks

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `hooks/__tests__/events.test.ts` | - | 0/71 | - |
| `hooks/__tests__/registry.test.ts` | - | 0/25 | - |

## Tools

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `registry/__tests__/tool-registry.test.ts` | - | 0/33 | - |
| `tools/__tests__/structured-output-tool.test.ts` | - | 0/12 | - |
| `tools/__tests__/tool-factory.test.ts` | - | 0/8 | - |
| `tools/__tests__/tool.test.ts` | - | 0/46 | - |
| `tools/__tests__/zod-tool.test.ts` | - | 0/32 | - |
| `tools/executors/__tests__/executor.cancel-signal.test.ts` | - | 0/3 | - |

## Plugins, middleware and background tasks

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `background-tasks/__tests__/background-tasks.test.ts` | - | 0/9 | - |
| `background-tasks/in-process/__tests__/engine-execution.test.ts` | - | 0/5 | - |
| `background-tasks/in-process/__tests__/engine-transitions.test.ts` | - | 0/7 | - |
| `background-tasks/in-process/__tests__/manager.test.ts` | - | 0/9 | - |
| `middleware/__tests__/agent-middleware.test.ts` | - | 0/62 | - |
| `middleware/__tests__/copy-on-input.test.ts` | - | 0/21 | - |
| `middleware/__tests__/custom-stages.test.ts` | - | 0/5 | - |
| `middleware/__tests__/middleware-interrupts.test.ts` | - | 0/15 | - |
| `middleware/__tests__/registry.test.ts` | - | 0/24 | - |
| `plugins/__tests__/plugin.test.ts` | - | 0/7 | - |
| `plugins/__tests__/registry.test.ts` | - | 0/10 | - |

## Conversation manager

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `conversation-manager/__tests__/agentic-context.test.ts` | - | 0/30 | - |
| `conversation-manager/__tests__/context-compression.test.ts` | - | 0/32 | - |
| `conversation-manager/__tests__/conversation-manager.test.ts` | - | 0/19 | - |
| `conversation-manager/__tests__/null-conversation-manager.test.ts` | - | 0/4 | - |
| `conversation-manager/__tests__/pin.test.ts` | - | 0/16 | - |
| `conversation-manager/__tests__/sliding-window-conversation-manager.test.ts` | - | 0/65 | - |
| `conversation-manager/__tests__/summarizing-conversation-manager.test.ts` | - | 0/20 | - |
| `conversation-manager/__tests__/token-usage-middleware.test.ts` | - | 0/15 | - |

## Storage

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `storage/__tests__/in-memory-storage.test.ts` | - | 0/25 | - |
| `storage/__tests__/local-file-storage.test.node.ts` | - | 0/31 | - |
| `storage/__tests__/namespaced-storage.test.ts` | - | 0/10 | - |
| `storage/__tests__/s3-storage.test.ts` | n/a | n/a | out of scope: S3 storage is not in v0.1 |
| `storage/embeddings/__tests__/bedrock.test.ts` | n/a | n/a | out of scope: embeddings are not in v0.1 |
| `storage/search/__tests__/keyword.test.ts` | - | 0/13 | - |
| `storage/search/__tests__/qmd.test.node.ts` | n/a | n/a | out of scope: needs the optional `@tobilu/qmd` SQLite index package, which has no Rust counterpart; v0.1 search is `KeywordSearchStrategy` |
| `storage/search/__tests__/s3-vector.test.ts` | n/a | n/a | out of scope: vector search is not in v0.1 |

## Session

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `session/__tests__/file-storage.test.node.ts` | n/a | n/a | out of scope: deprecated (the session `FileStorage`) |
| `session/__tests__/s3-storage.test.ts` | n/a | n/a | out of scope: S3 storage is not in v0.1, and the session `S3Storage` is deprecated |
| `session/__tests__/session-manager.test.ts` | - | 0/71 | the 12 multi-agent cases at lines 1311-1517 (multi-agent `saveSnapshot` and `restoreSnapshot`, `AfterMultiAgentInvocationEvent` and `BeforeMultiAgentInvocationEvent` handling, scope isolation): out of scope, they drive Graph and Swarm orchestrators, which are not in v0.1 |
| `session/__tests__/snapshot-storage-adapter.test.ts` | - | 0/24 | - |
| `session/__tests__/validation.test.ts` | - | 0/10 | - |

## Context manager (`strands::experimental`)

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `context-manager/__tests__/context-manager.test.ts` | - | 0/20 | - |
| `context-manager/__tests__/presets.test.ts` | - | 0/17 | - |
| `context-manager/__tests__/retrieval-tool.test.ts` | - | 0/9 | - |
| `context-manager/__tests__/stash-integration.test.ts` | - | 0/17 | - |
| `context-manager/__tests__/stash.test.ts` | - | 0/16 | - |
| `context-manager/strategies/__tests__/offload-strategy.test.ts` | - | 0/54 | - |
| `context-manager/strategies/__tests__/summarize-strategy.test.ts` | - | 0/54 | - |

## Injection

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `injection/__tests__/message-injection.test.ts` | - | 0/31 | - |

## Memory

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `memory/__tests__/memory-manager.test.ts` | - | 0/81 | - |
| `memory/__tests__/memory-tracing.test.ts` | - | 0/17 | - |
| `memory/extraction/__tests__/extraction.test.ts` | - | 0/42 | - |
| `memory/extraction/__tests__/model-extractor.test.ts` | - | 0/14 | - |
| `memory/extraction/__tests__/resolve-extraction-config.test.ts` | - | 0/13 | - |

## Vended memory stores

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `vended-memory-stores/bedrock-knowledge-base/__tests__/bedrock-knowledge-base-store.test.node.ts` | n/a | n/a | out of scope: the Bedrock Knowledge Base memory store is not in v0.1 |
| `vended-memory-stores/file-memory-store/__tests__/file-memory-store.test.node.ts` | - | 0/52 | `warns that the search option is deprecated and still delegates to it`, `warns about the deprecated search option once per store name`: deprecated (`FileMemoryStore` search option) |
| `vended-memory-stores/test-memory-store/__tests__/test-memory-store.test.node.ts` | n/a | n/a | out of scope: not in the v0.1 memory store set; `FileMemoryStore` covers local persistence |

## Vended plugins

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `vended-plugins/context-injector/__tests__/plugin.test.ts` | - | 0/19 | - |
| `vended-plugins/context-offloader/__tests__/file-storage-sandbox.test.node.ts` | n/a | n/a | out of scope: the `ContextOffloader` plugin is not in v0.1 |
| `vended-plugins/context-offloader/__tests__/plugin.test.node.ts` | n/a | n/a | out of scope: the `ContextOffloader` plugin is not in v0.1 |
| `vended-plugins/context-offloader/__tests__/plugin.test.ts` | n/a | n/a | out of scope: the `ContextOffloader` plugin is not in v0.1 |
| `vended-plugins/context-offloader/__tests__/search.test.ts` | n/a | n/a | out of scope: the `ContextOffloader` plugin is not in v0.1 |
| `vended-plugins/context-offloader/__tests__/storage.test.node.ts` | n/a | n/a | out of scope: the `ContextOffloader` plugin is not in v0.1 |
| `vended-plugins/context-offloader/__tests__/storage.test.ts` | n/a | n/a | out of scope: the `ContextOffloader` plugin is not in v0.1 |
| `vended-plugins/goal/__tests__/plugin.test.ts` | n/a | n/a | out of scope: GoalLoop is not in v0.1 |
| `vended-plugins/skills/__tests__/agent-skills.test.node.ts` | - | 0/39 | - |
| `vended-plugins/skills/__tests__/skill.test.node.ts` | - | 0/46 | - |

## Interventions

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `interventions/__tests__/handler.test.ts` | - | 0/2 | - |
| `interventions/__tests__/registry.test.ts` | - | 0/44 | - |
| `vended-interventions/cedar/__tests__/cedar.test.node.ts` | - | 0/39 | - |
| `vended-interventions/hitl/__tests__/hitl.test.ts` | - | 0/36 | the 8 `decision model classifier` cases: out of scope, the decision-model classifier is not in v0.1 |
| `vended-interventions/steering/__tests__/handler.test.ts` | n/a | n/a | out of scope: steering is not in v0.1 |
| `vended-interventions/steering/__tests__/llm.test.ts` | n/a | n/a | out of scope: steering is not in v0.1 |
| `vended-interventions/steering/__tests__/tool-ledger.test.ts` | n/a | n/a | out of scope: steering is not in v0.1 |

## Sandbox

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `sandbox/__tests__/default.test.node.ts` | - | 0/4 | - |
| `sandbox/__tests__/docker.test.node.ts` | n/a | n/a | out of scope: Docker and SSH sandboxes are not in v0.1 |
| `sandbox/__tests__/errors.test.node.ts` | `error::sandbox::tests` | 9/9 | - |
| `sandbox/__tests__/not-a-sandbox-local-environment.test.node.ts` | - | 0/27 | - |
| `sandbox/__tests__/posix-shell.test.node.ts` | - | 0/55 | - |
| `sandbox/__tests__/read-file-errors.test.node.ts` | - | 0/6 | - |
| `sandbox/__tests__/ssh.test.node.ts` | n/a | n/a | out of scope: Docker and SSH sandboxes are not in v0.1 |
| `sandbox/__tests__/stream-process.test.node.ts` | - | 0/5 | - |

## Vended tools

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `vended-tools/__tests__/paths.test.ts` | - | 0/7 | - |
| `vended-tools/a2a-client/__tests__/a2a-client.test.ts` | n/a | n/a | out of scope: depends on Graph, Swarm or A2A, which are not in v0.1 |
| `vended-tools/bash/__tests__/bash.test.node.ts` | - | 0/46 | - |
| `vended-tools/edit/__tests__/edit.test.node.ts` | - | 0/13 | - |
| `vended-tools/file-editor/__tests__/file-editor.test.node.ts` | n/a | n/a | out of scope: deprecated (`fileEditor`) |
| `vended-tools/graph/__tests__/graph.test.ts` | n/a | n/a | out of scope: depends on Graph, Swarm or A2A, which are not in v0.1 |
| `vended-tools/handoff-to-user/__tests__/handoff-to-user.test.ts` | - | 0/9 | - |
| `vended-tools/http-request/__tests__/http-request.test.ts` | - | 0/12 | - |
| `vended-tools/mcp-router/__tests__/mcp-router.test.node.ts` | - | 0/1 | - |
| `vended-tools/mcp-router/__tests__/mcp-router.test.ts` | - | 0/28 | - |
| `vended-tools/notebook/__tests__/notebook.test.ts` | - | 0/62 | - |
| `vended-tools/python-repl/__tests__/python-repl.test.node.ts` | - | 0/12 | - |
| `vended-tools/read/__tests__/read.test.node.ts` | - | 0/34 | - |
| `vended-tools/shell/__tests__/shell.test.node.ts` | - | 0/18 | `keeps the pre-rename tool name`, `matches makeShell apart from the name`, `an explicit name still wins`, `still exposes every shell name`: deprecated (`makeBash`) |
| `vended-tools/sleep/__tests__/sleep.test.ts` | - | 0/19 | - |
| `vended-tools/stop/__tests__/stop.test.ts` | - | 0/18 | - |
| `vended-tools/subagent/__tests__/subagent.test.ts` | - | 0/33 | - |
| `vended-tools/swarm/__tests__/swarm.test.ts` | n/a | n/a | out of scope: depends on Graph, Swarm or A2A, which are not in v0.1 |
| `vended-tools/web-fetch/__tests__/extract.test.ts` | - | 0/15 | - |
| `vended-tools/web-fetch/__tests__/web-fetch.test.ts` | - | 0/30 | - |
| `vended-tools/write/__tests__/write.test.node.ts` | - | 0/11 | - |

## MCP

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `mcp/__tests__/client-annotations.test.ts` | - | 0/4 | - |
| `mcp/__tests__/client-progress.test.ts` | - | 0/2 | - |
| `mcp/__tests__/client.test.ts` | - | 0/126 | `still lists tools when tasksConfig is set`: out of scope, MCP tasks are not in v0.1 |
| `mcp/__tests__/config.test.node.ts` | - | 0/48 | `skips invalid task settings when that server allows errors and keeps valid servers`, `rejects invalid task settings when that server overrides lenient defaults`: out of scope, MCP tasks are not in v0.1; `lets requestTimeout take precedence over the deprecated ttl alias`: deprecated (`ttl`) |
| `mcp/__tests__/task-client-legacy.test.node.ts` | n/a | n/a | out of scope: MCP tasks are not in v0.1 |
| `mcp/__tests__/task-client-stdio.test.node.ts` | n/a | n/a | out of scope: MCP tasks are not in v0.1 |
| `mcp/__tests__/task-client.test.ts` | n/a | n/a | out of scope: MCP tasks are not in v0.1 |
| `mcp/__tests__/task-transport.test.ts` | n/a | n/a | out of scope: MCP tasks are not in v0.1 |

## Telemetry

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `telemetry/__tests__/config.test.node.ts` | - | 0/18 | - |
| `telemetry/__tests__/config.test.ts` | - | 0/6 | - |
| `telemetry/__tests__/json.test.ts` | - | 0/15 | - |
| `telemetry/__tests__/local-trace.test.ts` | - | 0/16 | - |
| `telemetry/__tests__/meter.test.ts` | - | 0/62 | - |
| `telemetry/__tests__/tracer.test.node.ts` | - | 0/100 | - |

## Out of scope

None of these areas are in v0.1.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `a2a/__tests__/a2a-agent.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/__tests__/adapters.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/__tests__/async-lock.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/__tests__/events.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/__tests__/executor.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/__tests__/server.test.node.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/__tests__/server.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `decisions/__tests__/decision-model.test.ts` | n/a | n/a | out of scope: decision models are not in v0.1 |
| `decisions/__tests__/llm-decision-model.test.ts` | n/a | n/a | out of scope: decision models are not in v0.1 |
| `decisions/__tests__/llm-decision-model.tracing.test.node.ts` | n/a | n/a | out of scope: decision models are not in v0.1 |
| `decisions/__tests__/system-one-decision-models.test.node.ts` | n/a | n/a | out of scope: decision models are not in v0.1 |
| `experimental/__tests__/checkpoint.test.ts` | n/a | n/a | out of scope: checkpointing is not in v0.1 |
| `multiagent/__tests__/agent-tool-streamer.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/events.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/graph.invocation-context.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/graph.invocation-state.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/graph.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/graph.tracer.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/interrupts.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/nodes.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/plugins.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/snapshot.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/spec.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/state.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/swarm.invocation-context.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/swarm.invocation-state.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/swarm.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/__tests__/swarm.tracer.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |

## Integration (live checklist)

These specs live under `strands-ts/test/integ/` (paths below are relative to it) and call real models. They are ported as `live_*` tests, and results are recorded in [reviews/live.md](reviews/live.md).

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `a2a/a2a-agent.test.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `a2a/express-server.test.node.ts` | n/a | n/a | out of scope: A2A is not in v0.1 |
| `agent-as-tool.test.ts` | - | 0/1 | - |
| `agent.cancel.test.ts` | - | 0/5 | - |
| `agent.checkpoint.test.node.ts` | n/a | n/a | out of scope: checkpointing is not in v0.1 |
| `agent.test.ts` | - | 0/18 | - |
| `cedar.test.node.ts` | - | 0/3 | - |
| `conversation-manager/agentic-context.test.ts` | - | 0/11 | - |
| `conversation-manager/context-manager-message-reduction.test.ts` | - | 0/1 | - |
| `conversation-manager/context-manager-summarize-tool-calls.test.ts` | - | 0/1 | - |
| `conversation-manager/summarizing-conversation-manager.test.ts` | - | 0/2 | - |
| `decisions/decision-model.test.ts` | n/a | n/a | out of scope: decision models are not in v0.1 |
| `decisions/system-one-decision-models.test.node.ts` | n/a | n/a | out of scope: decision models are not in v0.1 |
| `environment.test.node.ts` | - | 0/2 | - |
| `environment.test.ts` | - | 0/3 | - |
| `goal/goal.test.node.ts` | n/a | n/a | out of scope: GoalLoop is not in v0.1 |
| `interrupt.test.ts` | - | 0/3 | - |
| `interventions.test.ts` | - | 0/52 | - |
| `mcp/mcp-connect-abort.test.node.ts` | - | 0/1 | - |
| `mcp/mcp-legacy-fallback.test.node.ts` | - | 0/4 | - |
| `mcp/mcp-reconnect.test.node.ts` | - | 0/3 | `runs SEP-2663 tasks again after disconnect and reconnect`: out of scope, MCP tasks are not in v0.1 |
| `mcp/mcp-sep2663.test.node.ts` | n/a | n/a | out of scope: every case drives SEP-2663 MCP tasks, which are not in v0.1 |
| `mcp/mcp-tasks-stdio.test.node.ts` | n/a | n/a | out of scope: MCP tasks are not in v0.1 |
| `mcp/mcp-tasks.test.node.ts` | n/a | n/a | out of scope: MCP tasks are not in v0.1 |
| `mcp/mcp-v2.test.node.ts` | - | 0/7 | - |
| `mcp/mcp.test.node.ts` | - | 0/7 | - |
| `memory/bedrock-knowledge-base-memory-manager.test.node.ts` | n/a | n/a | out of scope: the Bedrock Knowledge Base memory store is not in v0.1 |
| `memory/bedrock-knowledge-base-store.test.node.ts` | n/a | n/a | out of scope: the Bedrock Knowledge Base memory store is not in v0.1 |
| `models/anthropic.test.ts` | - | 0/13 | - |
| `models/bedrock.test.node.ts` | - | 0/3 | - |
| `models/bedrock.test.ts` | - | 0/29 | - |
| `models/classifier-strategy.test.ts` | - | 0/1 | - |
| `models/context-windows.test.ts` | - | 0/1 | the Gemini table rows: out of scope, the Google provider is not in v0.1 |
| `models/google.test.ts` | n/a | n/a | out of scope: the Google provider is not in v0.1 |
| `models/model-smoke.test.ts` | - | 0/2 | - |
| `models/openai/chat.test.ts` | - | 0/7 | - |
| `models/openai/mantle-routing.test.node.ts` | - | 0/1 | - |
| `models/openai/mantle.test.node.ts` | - | 0/4 | - |
| `models/openai/responses.test.ts` | - | 0/12 | - |
| `models/openai/session-cache-key.test.ts` | - | 0/6 | - |
| `multiagent/graph.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/interrupt-hook.test.node.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/interrupt-node.test.node.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/interrupt-session.test.node.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/session-manager.test.node.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `multiagent/swarm.test.ts` | n/a | n/a | out of scope: Graph and Swarm are not in v0.1 |
| `qmd-search-strategy.test.node.ts` | n/a | n/a | out of scope: needs the optional `@tobilu/qmd` SQLite index package, which has no Rust counterpart; v0.1 search is `KeywordSearchStrategy` |
| `s3-vector-search-strategy.test.node.ts` | n/a | n/a | out of scope: S3 storage and vector search are not in v0.1 |
| `sandbox/docker.test.node.ts` | n/a | n/a | out of scope: Docker and SSH sandboxes are not in v0.1 |
| `sandbox/ssh.test.node.ts` | n/a | n/a | out of scope: Docker and SSH sandboxes are not in v0.1 |
| `session-manager.test.node.ts` | - | 0/9 | the 4 `Session Management - S3Storage` cases: out of scope, S3 storage is not in v0.1 (and the session `S3Storage` is deprecated) |
| `skills/agent-skills.test.node.ts` | - | 0/4 | - |
| `telemetry.test.node.ts` | - | 0/36 | - |
| `tools/bash.test.node.ts` | - | 0/4 | - |
| `tools/file-editor.test.node.ts` | n/a | n/a | out of scope: deprecated (`fileEditor`) |
| `tools/http-request.test.ts` | - | 0/1 | - |
| `tools/notebook.test.ts` | - | 0/3 | - |
| `vended-interventions/steering/steering.test.node.ts` | n/a | n/a | out of scope: steering is not in v0.1 |
