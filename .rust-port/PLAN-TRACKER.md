# Plan tracker

## [ ] Phase 0: setup and guardrails
- [x] Branch `feat/rust-sdk` from `7296caba7`, `/tmp/ts-spec` export, toolchains and cargo tools, `.rust-port/` run folder
- [x] Workspace skeleton: root `Cargo.toml`, three crates, features, lints, `deny.toml`, rustfmt/clippy config
- [ ] AGENTS.md files (root layout, strands-rs, harness-rs), `docs/TESTING.md`, `docs/PARITY.md`
- [x] CI workflows and `ci.yml` path filters (test-lint, MSRV, deny, doctests)
- [ ] Conformance fixture generator (tsx) and committed fixtures
- [ ] Run logs and `DIVERGENCES.md` files created
- [ ] Phase review gate and fixes

## [ ] Phase 1: types, wire and errors
- [ ] Messages, 14 content blocks, `JsonBlock`, media and citations
- [ ] Tool types, `StopReason`, `Usage`, `Metrics`, snapshot and interrupt types
- [ ] `strands::Error` and TS-mirrored error types with exact messages
- [ ] Constructors for non-exhaustive wire structs
- [ ] Fixture round-trips byte-identical
- [ ] Phase review gate and fixes

## [ ] Phase 2: model abstraction, streaming and retry
- [ ] `ModelStreamEvent`, `ModelRequest`, `ModelState`, `BaseModelConfig`, `CacheConfig`
- [ ] `Model` trait with `stream_aggregated`, `summarize`, `count_tokens`, `estimate_utilization`, model info
- [ ] Retry strategy plugin, `DefaultModelRetryStrategy`, backoffs and jitter
- [ ] `strands::testing` mock model, hook recorder, test agent
- [ ] Phase review gate and fixes

## [ ] Phase 3: agent loop, hooks and tools
- [ ] `Agent`, `AgentConfig`, builder, lazy default model, `invoke`/`stream`/`AgentStream`, `AgentResult`
- [ ] Serializable stream event records (fixture-checked)
- [ ] Hook events, setters, `HookOrder`, `HookCleanup`, three registration forms
- [ ] `LocalAgent`, `ToolAgent`, `AppState`, `ModelState`, `InvocationState` handles
- [ ] `Tool`, `ToolContext`, `ToolOutcome`, `ToolRegistry`, `FunctionTool`, `ToolSource`, `tool_list!`, `ToolError`
- [ ] `#[tool]` and `#[tools]` macros with schema normalization, validation, trybuild tests
- [ ] Concurrent and sequential executors
- [ ] Loop semantics: deferred append, limits, `InvocationContext`, structured output, `end_turn`/`resume`
- [ ] Direct tool calls, `as_tool`, agents as tools, printer
- [ ] Phase review gate and fixes

## [ ] Phase 4: interrupts, cancellation, lifecycle, plugins and middleware
- [ ] Interrupts and resume with `pendingToolExecution`
- [ ] `CancelHandle`, `CancelOptions`, `CancelSignal`, TS cancel semantics
- [ ] Drop guard history repair, deferred cleanup, `shutdown()`, drop warning
- [ ] Snapshots (`take_snapshot`/`load_snapshot`)
- [ ] `Plugin` trait and built-in registration order
- [ ] Middleware stages and phases
- [ ] Background tasks
- [ ] Conversation managers (trait, Null, SlidingWindow, Summarizing)
- [ ] Phase review gate and fixes

## [ ] Phase 5: providers
- [ ] `BedrockModel` (ConverseStream, caching, guardrails, throttling)
- [ ] `AnthropicModel` (reqwest SSE, transport retry, error mapping, thinking, caching, compaction, server tools)
- [ ] `OpenAIModel` (Responses, Chat, stateful, params, caching, error mapping, transport retry)
- [ ] Bedrock Mantle (region, routing, SigV4 bearer token)
- [ ] Live tests for Bedrock, Mantle, Anthropic, OpenAI
- [ ] Phase review gate and fixes

## [ ] Phase 6: storage, sessions, context and memory
- [ ] `Storage`, `InMemoryStorage`, `LocalFileStorage`, namespaces, `KeywordSearchStrategy`
- [ ] `SessionManager` with TS layout and cross-SDK fixtures
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

## [ ] Phase 8: sandbox and vended tools
- [ ] `Sandbox` trait, protocol, errors, default local sandbox (POSIX and PowerShell)
- [ ] `shell`, `read`, `write`, `edit`, `bash`
- [ ] `python_repl`, `notebook`, `http_request`, `sleep`, `stop`, `handoff_to_user`
- [ ] `web_fetch` and `subagent`
- [ ] Phase review gate and fixes

## [ ] Phase 9: MCP
- [ ] `McpClient` stdio and streamable HTTP
- [ ] Custom legacy SSE transport
- [ ] Filters, prefixes, auth, elicitation, prompts, resources, list-changed, progress, logging, trace context
- [ ] `load_servers` config loading
- [ ] `mcp_router` tool
- [ ] Phase review gate and fixes

## [ ] Phase 10: telemetry
- [ ] Spans and attributes with semconv modes
- [ ] Meter counters and histograms
- [ ] `otel` feature exporters and env configuration
- [ ] Phase review gate and fixes

## [ ] Phase 11: SDK checkpoint
- [ ] Full gates, MSRV, deny, doctests, all live tests green
- [ ] Parity sweep complete in `docs/PARITY.md`
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
