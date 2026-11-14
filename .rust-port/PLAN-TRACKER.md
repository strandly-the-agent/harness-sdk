# Plan tracker


## [x] Phase 0: setup and guardrails
- [x] Branch `feat/rust-sdk` from `7296caba7`, `/tmp/ts-spec` export, toolchains and cargo tools, `.rust-port/` run folder
- [x] Workspace skeleton: root `Cargo.toml`, three crates, features, lints, `deny.toml`, rustfmt/clippy config
- [x] AGENTS.md files (root layout, strands-rs, harness-rs), `docs/TESTING.md`, `docs/PARITY.md`
- [x] CI workflows and `ci.yml` path filters (test-lint, MSRV, deny, doctests)
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
- [x] `strands::testing` mock model (the hook recorder and test agent moved to Phase 3, section 12 Phase 2 detail 9)
- [x] Phase review gate and fixes

## [ ] Phase 3: agent loop, hooks and tools
- [x] `Agent`, `AgentConfig`, builder, lazy default model, `invoke`/`stream`/`AgentStream`, `AgentResult`
- [x] Serializable stream event records (fixture-checked)
- [x] Hook events, setters, `HookOrder`, `HookCleanup`, three registration forms
- [x] `LocalAgent`, `ToolAgent`, `AppState`, `ModelState`, `InvocationState` handles
- [x] `Tool`, `ToolContext`, `ToolOutcome`, `ToolRegistry`, `FunctionTool`, `ToolSource`, `tool_list!`, `ToolError`
- [x] `#[tool]` and `#[tools]` macros with schema normalization, validation, trybuild tests
- [x] Concurrent and sequential executors
- [x] Loop semantics: deferred append, limits, `InvocationContext`, structured output, `end_turn`/`resume`
- [x] Direct tool calls, `as_tool`, agents as tools, printer
- [x] `strands::testing` hook recorder and test agent
- [ ] Phase review gate and fixes
- [x] `invoke`, `invoke_with`, `stream`, `stream_with`, `AgentStream<'_>` (named, `Unpin`), the `InvokeArgs` conversions, and `InvokeOptions` (structured output schema via `structured_output_schema::<T>()`, cancel signal, invocation state, invocation context, limits, and every other TS invoke option).
- [x] `AgentResult` with `Display`, `structured_output::<T>()`, `stop_reason`, `interrupts`, `AgentMetrics`, `AgentTrace`.
- [x] Stream items: the 16 owned records, checked against the fixtures.
- [x] Hooks: the 17 events, getters, setters per 8.5, `HookOrder`, reverse order on `After*`, `HookCleanup`, `add_hook`, `add_async_hook::<E>`, `add_hook_impl`, interrupt collection, duplicate-name errors.
- [x] Agent views and handles per 8.3 and 8.7.
- [x] Tools per 8.8, including `ToolRegistry` and `FunctionTool` (`FunctionTool::builder(..)` with a typed handler: schemars schema plus jsonschema validation).
- [x] `strands-macros`:
- [x] Executors per 8.8.
- [x] Loop semantics: deferred append, limits in the TS check order, `InvocationContext` (nested limits and usage), structured output through `strands_structured_output` with retries, `end_turn` and `resume`, and `model-proxy.ts`.
- [x] Direct tool calls: `agent.tool("name").invoke(input).await` (TS `agent.tool.<name>.invoke()`), recorded into history as TS does (`agent/tool-caller.ts`).
- [x] `as_tool` with `preserve_context` and `delegate`; agents accepted in `.tools(..)`.
- [x] The stdout printer (#54): on by default, `.printer(false)` to turn it off, matching TS output (`agent/printer.ts`, `printer.test.ts`).
- [x] Wire the retry strategy into `AfterModelCallEvent` and port the remaining retry tests.
- [ ] TS specs: `agent/__tests__/{agent, agent.hook, agent.concurrent (adapt it: concurrent invocation is a compile error here, so document that gap), agent.invocation-context, agent.invocation-state, agent.model-retry, agent-as-tool, agent-as-tool.invocation-context, agent-as-tool.invocation-state, agent-delegation, agent.continuation, invocation-context, printer, tool-caller}.test.ts`, `hooks/__tests__/{events, registry}.test.ts`, `tools/__tests__/{tool, tool-factory, structured-output-tool, zod-tool (map it to the macro and FunctionTool)}.test.ts`, the parts of `tools/executors/__tests__/executor.cancel-signal.test.ts` that don't need Phase 4, `registry/__tests__/tool-registry.test.ts`, `types/__tests__/agent.test.ts`, `__tests__/{queue, default-slot, index}.test.ts` where they apply, and `models/__tests__/model-proxy.test.ts`.

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
- [ ] `create_harness(options).await -> Result<Agent>`, returning a plain SDK `Agent`. `HarnessAgentOptions` has every TS field plus `AgentConfig` passthrough, in struct and builder form. Decide whether the options builder's `build()` is fallible, consistently with the SDK, and log it. Use one enum per polymorphic option (`On`/`Off`/`Config`/`Instance`), with an explicit `Auto` where TS distinguishes unset from explicit.
- [ ] The `AgentConfig` passthrough mirrors TS `HarnessAgentOptions extends Omit<AgentConfig, 'model' | 'tools' | 'plugins' | 'interventions' | 'backgroundTasks' | 'contextManager'>` (`harness-ts/src/agent.ts`): those six are harness fields, so setting them in the passthrough returns a typed `HarnessError` (log the choice). Harness-resolved `session_manager`, `storage` and `memory_manager` override passthrough values, following TS's spread order. With an explicit sandbox, remove its vended tools after initialize as TS does. Test precedence per field.
- [ ] Defaults (#70): model `bedrock/global.anthropic.claude-opus-5`; effort and caching `auto`; `ContextManager` `auto`; the 8 built-in tools (`shell`, `read`, `write`, `edit`, `web_fetch`, `web_search`, `programmatic_tool_caller`, `subagent`) and 2 plugins (`Todos`, `EnvironmentContext`); sessions under `./.agent/sessions` through `LocalFileStorage::new("./.agent").namespace("sessions")`, with an 8-hex id and `save_latest_on: message`; the stash at `<dir>/<id>`; skills at `./.agent/skills`; memory at `./.agent/memory` with `ModelExtractor` on a small per-provider model; background tasks on, with `subagent` always run in the background; subagent max depth 2; telemetry from `OTEL_TRACES_EXPORTER`; `web_search` silently disabled on Bedrock.
- [ ] Models: the model-string grammar for `bedrock/`, `bedrock-mantle/`, `anthropic/`, `openai/`, with bare ids meaning Bedrock; the error texts in section 10 for disabled features, unsupported providers and unknown providers (`harness-ts/src/models.ts`); effort tables per provider and family (adaptive versus budget thinking, max-tokens tables); caching rules; `web_fetch` model selection (`resolveWebFetchModel`, `supportsMedia`); `supports_thinking` and `supports_web_search`.
- [ ] Prompt: `HARNESS_CONTRACT` byte-identical, `build_system_prompt`, and `instructions` versus `system_prompt`. Add a parity test that reads the TS prompt constants from `/tmp/ts-spec` when present.
- [ ] Built-in tools:
- [ ] Plugins: `Todos` (`strands:todos`, the `todo_write` tool storing items under the TS app-state key, `activeForm` in camelCase, the every-turn `<system-reminder>` injection with TS strings) and `EnvironmentContext` (the TS discovery rules and caps, the `AGENTS.md` cap with char-boundary-safe truncation, nearby `AGENTS.md`/`README.md` links with the TS depth and skip rules, all through the sandbox, native date and platform, PowerShell shell detection on Windows).
- [ ] Interventions grammar: `off`, `ask`, `smart`, `*.cedar`, free text, handler instances, arrays, duplicate names (TS errors). `ask` without a callback interrupts on the first gated call, so `invoke` returns an interrupt.
- [ ] `mcp_servers`: a path or an inline map, `continue_on_error`, prefixes. Connected clients are reused for subagents.
- [ ] Portable config: `HarnessAgentConfig` (24 fields at the pin: `name`, `description`, `instructions`, `model`, `modelModule`, `effort`, `tools`, `subagents`, `mcpServers`, `builtinTools`, `caching`, `contextManager`, `session`, `skills`, `memory`, `memoryStores`, `plugins`, `builtinPlugins`, `interventions`, `interventionModules`, `sandbox`, `agentConfigModules`, `dependencies`, `agentConfig`; the `*Module`/`*Modules` fields are module references), `DEFAULT_HARNESS_AGENT_CONFIG`, normalization with unknown-key warnings and JSON-path errors (TS texts), and `harness_agent_options_from_config(config, base_dir)` with path and env expansion. Module references anywhere, including the nested `builtinTools.web_fetch.model`, produce the unsupported error from section 10 (`language` is optional and is `typescript` or `python`).
- [ ] Telemetry setup from `OTEL_TRACES_EXPORTER`, idempotent.
- [ ] Port the harness-ts specs (`harness-ts/test/*.test.ts`, `plugins/*`, `tools/*`), with the integration folder as the live checklist.
- [ ] Live end-to-end per provider (Bedrock, Mantle, Anthropic, OpenAI), each covering a tool call, an MCP tool (`harness-ts/test/tools/echo-mcp-server.mjs` via node), an interrupt and resume (`interventions: "ask"`), and a session resume. Run them in a temp directory, never inside the repo.
- [ ] Cross-SDK check: a session written by TS `createHarness` (harness-ts at the pin run via tsx, against a mock or real model, in a temp dir) is resumed by Rust, and the reverse, on the same `./.agent` directory. Record the commands and results in `harness-rs/docs/reviews/live.md`.

## [ ] Phase 13: final checkpoint and handoff
- [ ] Full workspace gates and `cargo package --list` dry checks
- [ ] `IMPLEMENTATION-REPORT.md` with pending decisions, divergences and TS bugs
- [ ] `FOLLOW-UPS.md`, `STATE.md` and tracker finalized; working tree clean
