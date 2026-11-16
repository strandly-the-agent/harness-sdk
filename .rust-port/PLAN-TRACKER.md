# Plan tracker

Tick an item when finished; tick a phase heading when the phase and its review gate are done.

## [x] Phase 0: setup and guardrails

- [x] Branch `feat/rust-sdk` from `7296caba7`, `/tmp/ts-spec` export, toolchains and cargo tools, `.rust-port/` run folder
- [x] Workspace skeleton: root `Cargo.toml`, three crates, features, lints, `deny.toml`, rustfmt/clippy config
- [x] AGENTS.md files (root layout, strands-rs, harness-rs), `docs/TESTING.md`, `docs/PARITY.md`
- [x] CI workflows and `ci.yml` path filters (test-lint, MSRV, deny, doctests) (staged in `.rust-port/ci-workflows/`, P-13)
- [x] Conformance fixture generator (tsx) and committed fixtures
- [x] Run logs and `DIVERGENCES.md` files created
- [x] Phase review gate and fixes

## [x] Phase 1: types, wire and errors

- [x] Messages, 14 content blocks, `JsonBlock`, media and citations
- [x] Tool types, `StopReason`, `Usage`, `Metrics`, snapshot and interrupt types
- [x] `strands::Error` and TS-mirrored error types with exact messages
- [x] Constructors for non-exhaustive wire structs
- [x] Fixture round-trips byte-identical
- [x] Phase review gate and fixes

## [x] Phase 2: model abstraction, streaming and retry

- [x] `ModelStreamEvent`, `ModelRequest`, `ModelState`, `BaseModelConfig`, `CacheConfig`
- [x] `Model` trait with `stream_aggregated`, `summarize`, `count_tokens`, `estimate_utilization`, model info
- [x] Retry strategy plugin, `DefaultModelRetryStrategy`, backoffs and jitter
- [x] `strands::testing` mock model, hook recorder, test agent (Phase 2: `MockMessageModel`, `TestModelProvider`; `HookRecorder` and `test_agent` come in Phase 3)
- [x] Phase review gate and fixes

## [x] Phase 3: agent loop, hooks and tools

- [x] `Agent`, `AgentConfig`, builder, lazy default model, `invoke`/`stream`/`AgentStream`, `AgentResult` (deferred config keys in P-67)
- [x] Serializable stream event records (fixture-checked)
- [x] Hook events, setters, `HookOrder`, `HookCleanup`, three registration forms
- [x] `LocalAgent`, `ToolAgent`, `AppState`, `ModelState`, `InvocationState` handles (landed with the hook events, P-50; the registry view joins `ToolAgent` with tools)
- [x] `Tool`, `ToolContext`, `ToolOutcome`, `ToolRegistry`, `FunctionTool`, `ToolSource`, `tool_list!`, `ToolError`
- [x] `#[tool]` and `#[tools]` macros with schema normalization, validation, trybuild tests
- [x] Concurrent and sequential executors
- [x] Loop semantics: deferred append, limits, `InvocationContext`, structured output, `end_turn`/`resume`
- [x] Direct tool calls, `as_tool`, agents as tools, printer
- [x] Phase review gate and fixes

## [x] Phase 4: interrupts, cancellation, lifecycle, plugins and middleware

- [x] Interrupts and resume with `pendingToolExecution`
- [x] `CancelHandle`, `CancelOptions`, `CancelSignal`, TS cancel semantics
- [x] Drop guard history repair, deferred cleanup, `shutdown()`, drop warning
- [x] Snapshots (`take_snapshot`/`load_snapshot`)
- [x] `Plugin` trait and built-in registration order
- [x] Middleware stages and phases
- [x] Background tasks
- [x] Conversation managers (trait, Null, SlidingWindow, Summarizing)
- [x] Phase review gate and fixes

## [x] Phase 5: providers

- [x] `BedrockModel` (ConverseStream, caching, guardrails, throttling)
- [x] `AnthropicModel` (reqwest SSE, transport retry, error mapping, thinking, caching, compaction, server tools)
- [x] `OpenAIModel` (Responses, Chat, stateful, params, caching, error mapping, transport retry)
- [x] Bedrock Mantle (region, routing, SigV4 bearer token)
- [x] Live tests for Bedrock, Mantle, Anthropic, OpenAI (Bedrock and Mantle run green; Anthropic and OpenAI skip without keys, see `strands-rs/docs/reviews/live.md`)
- [x] Phase review gate and fixes

## [ ] Phase 6: storage, sessions, context and memory

- [x] `Storage`, `InMemoryStorage`, `LocalFileStorage`, namespaces, `KeywordSearchStrategy`
- [x] `SessionManager` with TS layout and cross-SDK fixtures
- [ ] Experimental `ContextManager` (presets, offload strategies, stash, `retrieve_context`, pinning, agentic tools)
- [ ] `ContextInjector` and injection with `InjectedBlock`
- [ ] `MemoryManager`, `MemoryStore`, `FileMemoryStore`, `ModelExtractor`
- [ ] `AgentSkills` and `Skill`
- [ ] Phase review gate and fixes

## [ ] Phase 7: interventions and routing

- [ ] `InterventionHandler` and registry
- [ ] `HumanInTheLoop` with classifiers
- [ ] `CedarAuthorization`
- [ ] `ModelRouter` with `FallbackStrategy` and `ClassifierStrategy`
- [ ] Phase review gate and fixes

## [x] Phase 8: sandbox and vended tools

- [x] `Sandbox` trait, protocol, errors, default local sandbox (POSIX and PowerShell)
- [x] `shell`, `read`, `write`, `edit`, `bash`
- [x] `python_repl`, `notebook`, `http_request`, `sleep`, `stop`, `handoff_to_user`
- [x] `web_fetch` and `subagent`
- [x] Phase review gate and fixes

## [x] Phase 9: MCP

- [x] `McpClient` stdio and streamable HTTP
- [x] Custom legacy SSE transport
- [x] Filters, prefixes, auth, elicitation, prompts, resources, list-changed, progress, logging, trace context
- [x] `load_servers` config loading
- [x] `mcp_router` tool
- [x] Phase review gate and fixes

## [ ] Phase 10: telemetry

- [ ] Spans and attributes with semconv modes
- [ ] Meter counters and histograms
- [ ] `otel` feature exporters and env configuration
- [ ] Phase review gate and fixes

## [ ] Phase 11: SDK checkpoint

- [ ] Full gates, MSRV, deny, doctests, all live tests green
- [ ] Parity sweep complete in `docs/PARITY.md`
  - [ ] Rename Phase 1-3 tests to the snake_case TS `it(...)` titles and add a mechanical title-mapping check
- [ ] Crate docs, README and examples
- [ ] Whole-SDK review gate and fixes
- [ ] Local tag `strands-rs-v0.1.0-preview.1`

## [ ] Phase 12: harness

- [ ] `create_harness`, `HarnessAgentOptions`, defaults, sessions layout (#70)
- [ ] Model strings, effort, caching, provider tables
- [ ] `HARNESS_CONTRACT` and `build_system_prompt`
- [ ] Harness file tools, `shell`, `web_fetch`, `web_search` (native and Exa)
- [ ] `programmatic_tool_caller` on Monty
- [ ] `subagent` with presets and axes
- [ ] `Todos` and `EnvironmentContext` plugins
- [ ] Interventions grammar and `mcp_servers`
- [ ] Portable `HarnessAgentConfig` parsing and module-reference errors
- [ ] Telemetry setup from env
- [ ] Live end-to-end per provider and cross-SDK session resume
- [ ] Phase review gate and fixes

## [ ] Phase 13: final checkpoint and handoff

- [ ] Full workspace gates and `cargo package --list` dry checks
- [ ] `IMPLEMENTATION-REPORT.md` with pending decisions, divergences and TS bugs
- [ ] `FOLLOW-UPS.md`, `STATE.md` and tracker finalized; working tree clean
