# Phase 2 plan: strands-rs → harness-ready, then harness-rs

Goal: `harness-rs` reproduces `harness-ts` **default** behaviour (`createHarness()` with no options).

## What the default harness turns on (harness-ts/src/agent.ts + defaults.ts)

| Default | SDK feature required | strands-rs status |
|---|---|---|
| model `bedrock/global.anthropic.claude-opus-5`, effort `auto` → thinking/effort request fields | `BedrockModel` + `additional_request_fields` | ✅ |
| caching `auto` | Bedrock `CacheConfig` strategy auto | ✅ |
| `systemPrompt = buildSystemPrompt(instructions)` (HARNESS_CONTRACT) | — | harness |
| `contextManager: 'auto'` | **`context-manager/`** (ContextManager plugin: compaction, stash) | ❌ port |
| `session: true` → `SessionManager` + `FileStorage(./.agent/sessions)` + `LocalFileStorage` storage | session ✅; **`storage/` (Storage trait, LocalFileStorage)** | ❌ port storage |
| `skills: true` (if `./.agent/skills` exists) → `AgentSkills` | **`vended-plugins/skills`** (+ `context-injector`) | ❌ port |
| `memory: true` → `MemoryManager` + file memory store + `ModelExtractor` | **`memory/`, `vended-memory-stores/file-memory-store`** | ❌ port |
| builtin plugins `todos`, `environment` | plugin trait ✅ | harness |
| builtin tools `shell` | **`vended-tools/bash`** | ❌ port |
| builtin tools `read`/`write`/`edit`, `web_fetch`, `web_search` (exa, only w/ key), `subagent` | agent-as-tool / Agent spawn | harness (+ SDK `Agent` ✅) |
| builtin tool `programmatic_tool_caller` (Monty Python sandbox) | — | **not portable → disabled by default in Rust**, documented |
| `backgroundTasks` default `{ agentic: ['*'] }`, `subagent` forced background | **`background-tasks/`** | ❌ port |
| `interventions` default none | `interventions/` + hitl | defer (optional) |
| telemetry (env-gated) | `telemetry/` | defer (optional) |
| `mcpServers` (config only, default none) | `mcp/` | defer (next phase) |
| other providers (anthropic/openai/gemini/mantle/litellm/openrouter) | — | defer; `resolve_model` returns a clear error for unsupported providers |

## Waves

- **W1 (SDK, parallel branches):** `storage` (Storage trait + LocalFileStorage), `bash` vended tool,
  `background-tasks`, `skills` + `context-injector` plugins, `context-manager`.
- **W2 (SDK):** `memory` (MemoryManager, ModelExtractor, file store). Agent wiring for
  `context_manager`, `storage`, `memory_manager`, `background_tasks`, `sandbox`-less.
- **W3 (harness-rs, parallel):** `defaults`, `prompt` (vend the same contract text), `models`
  (`resolve_model` for bedrock incl. effort/thinking + caching), `builtin_tools` resolution,
  tools: `file_tools` (read/write/edit), `web_fetch`, `web_search`, `subagent`; plugins `todos`,
  `environment`; `memory`, `interventions` (skeleton), `create_harness`.
- **W4:** validation (unit + live Bedrock e2e of `create_harness()` defaults), reviews, docs,
  PORT_REPORT update.

Branch scheme unchanged: `agent-tasks/strands-rs-<module>` → `agent-tasks/strands-rs`;
harness on `agent-tasks/harness-rs-<module>` → merged into the same integration branch under
`strands-rs/crates/strands-harness` (workspace member).
