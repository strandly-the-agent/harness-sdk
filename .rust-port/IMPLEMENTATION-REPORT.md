# Implementation report

Markers to fill:

- TODO(lead): final test counts from the gate script
- TODO(lead): harness parity totals
- TODO(lead): harness live results
- TODO(lead): Phase 12 review dispositions
- TODO(lead): last commit
- TODO(lead): commit count

This draft was written on the worktree branch `wip/p13` at `ba501ced3` (2026-11-16). Every number below was read from the files of that commit or computed from them with a script; numbers that depend on work still in progress carry a marker.

## What was built

The branch adds a Cargo workspace at the repository root (`Cargo.toml`, resolver 3, edition 2024, minimum supported Rust version 1.96, licence Apache-2.0) with three crates, all at version `0.1.0-preview.1` and all with `publish = false`:

- `strands` (`strands-rs/strands`): the SDK.
- `strands-macros` (`strands-rs/strands-macros`): the procedural macros `#[tool]` and `#[tools]`, re-exported from `strands`.
- `strands-harness` (`harness-rs/strands-harness`): the harness, built on `strands`.

Nothing was published. The root also carries `deny.toml`, `rustfmt.toml` and `clippy.toml`, and the gate script is `.rust-port/gates.sh`.

### The `strands` crate

The crate mirrors the TypeScript SDK (`@strands-agents/sdk`) concept for concept with Rust naming, and reads and writes the same JSON for messages, sessions and snapshots. At `ba501ced3` its `src/` tree holds 368 Rust files (unit tests included).

Public modules, as declared in `strands-rs/strands/src/lib.rs`:

- `agent`: `Agent`, `AgentBuilder`, `AgentConfig`, `AgentStream`, direct tool calls (`tool_caller`), snapshots, agents as tools (`agent_as_tool`), delegation, the invocation context and the printer.
- `models`: the `Model` trait, the model stream events, model info, client-side summarization, native structured output, the strict-schema transform, the HTTP transport, the providers `BedrockModel`, `AnthropicModel` and `OpenAIModel` (Responses, Chat Completions and Amazon Bedrock Mantle), and `models::routing` with the model router and its fallback and classifier strategies.
- `tools`: the `Tool` trait, `ToolContext`, `FunctionTool`, `ToolRegistry`, the concurrent and sequential executors, schema generation and structured output.
- `hooks`: typed lifecycle events, the hook registry, `HookOrder` and removal.
- `plugins`, `middleware`, `interrupt`, `cancel` (`CancelSignal`, `CancelHandle`, `CancelOptions`), `retry` (model retry strategies and backoffs), `background_tasks`, `state_store` (`StateStore`).
- `conversation_manager`: the null, sliding-window and summarizing managers, proactive compression and the token-usage middleware.
- `experimental`: `ContextManager` with its presets and strategies, `Offload`, the stash and the related config types.
- `session` (`SessionManager` over snapshots), `storage` (the `Storage` trait, `InMemoryStorage`, `LocalFileStorage`, keyword search), `memory` (`MemoryManager`, extraction, the `MemoryStore` trait) and `vended_memory_stores` (`FileMemoryStore`).
- `injection`, `vended_plugins` (`ContextInjector`, `AgentSkills`).
- `interventions` (the `InterventionHandler` trait and registry) and `vended_interventions` (`HumanInTheLoop`, `CedarAuthorization`).
- `sandbox`: the `Sandbox` trait and the default local environment with POSIX shell and PowerShell support.
- `vended_tools`: `bash`, `edit`, `handoff_to_user`, `http_request`, `mcp_router`, `notebook`, `python_repl`, `read`, `shell`, `sleep`, `stop`, `subagent`, `web_fetch`, `write`.
- `mcp` (behind the `mcp` feature): `McpClient` over stdio, streamable HTTP and a legacy SSE transport, with filters, auth, elicitation, config loading and trace-context propagation.
- `telemetry`: `AgentMetrics` and `AgentTrace` on every result, the `tracing` spans, the meter, and OpenTelemetry setup behind `otel`.
- `types` (messages, content blocks, media, citations, tool types, snapshots, JSON helpers), `error` (`Error`, `Result` and the error types behind each variant) and `testing` (a scripted model and a hook recorder for code built on Strands).

The crate root re-exports `Agent`, `AgentConfig`, `AgentStream`, `AgentResult`, `AgentStreamEvent`, `InvocationState`, `InvokeArgs`, `InvokeOptions`, `CancelHandle`, `CancelOptions`, `CancelSignal`, `Error`, `Result`, `InterruptResponse`, `StateStore`, `Tool`, `ToolContext`, `ToolError`, `ToolOutcome`, `ToolSource`, `JsonValue`, `ContentBlock`, `Message`, `StopReason`, the macros `tool` and `tools`, `async_trait`, `schemars` and `indexmap`, and, with the `bedrock` feature, `BedrockModel` and its config types.

Cargo features of `strands`:

| Feature | Default | Enables |
|---|---|---|
| `bedrock` | yes | The Amazon Bedrock provider, which is also the default model |
| `anthropic` | no | The Anthropic provider |
| `openai` | no | The OpenAI provider (Responses and Chat Completions); with `bedrock`, also Amazon Bedrock Mantle |
| `mcp` | no | The Model Context Protocol client |
| `otel` | no | OpenTelemetry export |

The crate ships ten examples under `strands-rs/strands/examples/` (`basic`, `streaming`, `tools`, `hooks`, `structured_output`, `sessions`, `mcp`, `bedrock`, `anthropic`, `openai`), a README (`strands-rs/README.md`), crate-level documentation with a module overview and a table that maps TypeScript names to Rust, cross-SDK conformance fixtures under `strands-rs/conformance/`, and the documents `strands-rs/docs/{PARITY,DIVERGENCES,TESTING}.md`.

### The `strands-macros` crate

Two attribute macros: `#[tool]` turns an async or sync function into a tool type (the doc comments become the description and the parameter descriptions), and `#[tools]` does the same for the methods of an `impl` block. The source is `lib.rs`, `args.rs`, `signature.rs` and `expand.rs`; compile-fail cases run under trybuild.

### The `strands-harness` crate

`create_harness` builds a ready-to-run agent on `strands` with the defaults of the TypeScript harness (`@strands-agents/harness`). The modules are private and the crate root re-exports every public name, as the TypeScript package entry point does. The root exports at `ba501ced3`:

- `create_harness`, `HarnessAgentOptions` and its option types (`BuiltinToolsOption`, `CachingOption`, `McpServersOption`, `MemoryOption`, `SessionOption`, `SkillsOption`, `InterventionsOption`, `ContextManagerOption`, `Effort`, `Setting`).
- The portable config: `HarnessAgentConfig` and its `HarnessConfig*` types, `define_harness_agent_config`, `harness_agent_options_from_config`, `DEFAULT_HARNESS_AGENT_CONFIG`, `BUILTIN_TOOL_NAMES`, `BUILTIN_PLUGIN_NAMES`, `HarnessModuleReference`.
- The prompt: `HARNESS_CONTRACT` and `build_system_prompt`.
- Tools: `read`, `write`, `edit`, `make_web_fetch`, `make_subagent` with its presets and axes (`Preset`, `Choice`, `Fixed`, `Inherit`, `Open`, `ContextAxis`, the `*Mode` enums), `make_programmatic_tool_caller` and `programmatic_tool_caller` (on Monty), and, with `mcp`, `exa_web_search` and `make_exa_web_search`.
- Plugins: `Todos` and `EnvironmentContext` with their config types.
- Model helpers `supports_thinking` and `supports_web_search`, and the typed `HarnessError`.

The harness features `bedrock`, `anthropic`, `openai`, `mcp` and `otel` each forward to the `strands` feature of the same name; all five are on by default. The harness documents are `harness-rs/README.md` and `harness-rs/docs/{PARITY,DIVERGENCES}.md`.

### CI

Six workflow files for the Rust workspace are staged in `.rust-port/ci-workflows/` (`ci.yml`, `strands-rs-pr-and-push.yml`, `strands-rs-test-lint.yml`, `harness-rs-pr-and-push.yml`, `harness-rs-test-lint.yml`, `rust-security-audit.yml`). They are not under `.github/workflows/` because the token that persists the branch cannot write there (P-13), so none of them has run.

## Test counts and parity coverage

### Test counts

TODO(lead): final test counts from the gate script

The counts recorded in `.rust-port/STATE.md` so far:

- At the tagged commit `fa45de4be`, `gates.sh full` was green with 4590 tests with all features and 67 ignored live tests, plus the MSRV build, each feature alone, `cargo deny` and the conformance check.
- At `da2fc2e0a` (Phase 12 in progress), the default `gates.sh` was green with 5101 tests with all features and 67 ignored live tests; `gates.sh full` was last green at `0ef5deaaa` with 5080 tests.
- `cargo package --list --allow-dirty` worked at the tagged commit for `strands` (426 files) and `strands-macros` (78 files).

### SDK parity

`strands-rs/docs/PARITY.md` maps every spec file of `strands-ts/src/` to Rust tests. Summed over its "In scope" table with a script: 150 spec files, 3901 of 4223 TS test cases ported, 322 cases recorded as gaps with a reason. A further 59 spec files are listed as out of scope. The title checker (`strands-rs/docs/check-test-titles.mjs`) exits 0 and prints `ok` for all 150 rows.

| Area (directory under `strands-ts/src/`) | Spec files | Ported/total |
|---|---|---|
| root `__tests__` | 7 | 91/133 |
| `agent` | 23 | 527/572 |
| `background-tasks` | 4 | 30/30 |
| `context-manager` | 7 | 183/188 |
| `conversation-manager` | 8 | 200/201 |
| `hooks` | 2 | 89/96 |
| `injection` | 1 | 30/31 |
| `interventions` | 2 | 41/46 |
| `logging` | 2 | 3/9 |
| `mcp` | 4 | 172/180 |
| `memory` | 5 | 165/167 |
| `middleware` | 5 | 124/127 |
| `models` | 21 | 890/904 |
| `plugins` | 2 | 17/17 |
| `registry` | 1 | 30/33 |
| `retry` | 2 | 29/34 |
| `sandbox` | 6 | 101/106 |
| `session` | 3 | 87/105 |
| `storage` | 4 | 71/79 |
| `telemetry` | 6 | 202/217 |
| `tools` | 5 | 72/101 |
| `types` | 6 | 167/222 |
| `vended-interventions` | 2 | 67/75 |
| `vended-memory-stores` | 1 | 48/52 |
| `vended-plugins` | 4 | 127/130 |
| `vended-tools` | 17 | 338/368 |
| Total | 150 | 3901/4223 |

Every gap row carries its reason in `PARITY.md`. The reasons fall into three kinds:

- Cases about a JavaScript runtime mechanism that Rust types rule out. The largest rows are `types/__tests__/json.test.ts` (0/44: `deepCopy` and its rejection of functions, symbols, `undefined` and cycles, none of which a `JsonValue` can hold), `__tests__/state-store.test.ts` (30/49), `tools/__tests__/tool.test.ts` (29/46: async generator callbacks, circular and non-serializable values, non-`Error` throws) and `__tests__/errors.test.ts` (20/29: `isFetchNetworkError` and `normalizeError`). Smaller rows of the same kind cover arguments that are typed in Rust (for example `NonZeroU32` attempts in `retry`), hook callbacks and predicates that cannot throw, and prototype-based override detection.
- Cases that belong to features outside v0.1 inside an otherwise in-scope spec: the 18 multi-agent cases of `session/__tests__/session-manager.test.ts`, `__tests__/queue.test.ts` (0/10, used only by multi-agent graphs), MCP task timeouts, and checkpointing cases in the agent specs.
- Cases about APIs that TS marks deprecated.

The 59 out-of-scope spec files are multi-agent and A2A (24 files with the `graph`, `swarm` and `a2a_client` tools), the `ContextOffloader` plugin (5), decision models (5), MCP tasks (4), steering (3), checkpointing (2), S3 storage (2), the Google and Vercel providers, the Docker and SSH sandboxes, embeddings, vector search, the Bedrock Knowledge Base memory store, GoalLoop, QMD search, `TestMemoryStore`, and the deprecated session `FileStorage` and `fileEditor`.

The "Live checklist" table of `PARITY.md` lists the TS integration specs under `strands-ts/test/integ/`: 24 are out of scope, 15 are covered by Rust live tests, the others are covered in part, covered offline, or TS-only, each with its note.

### Harness parity

TODO(lead): harness parity totals

At `ba501ced3`, `harness-rs/docs/PARITY.md` sums to 411 of 485 TS test cases over 20 spec files (root specs 250/317 over 12 files, `plugins` 16/20 over 2, `tools` 145/148 over 6). The harness title checker (`--parity harness-rs/docs/PARITY.md`) exits 1 at that commit: 15 rows print `ok` and 5 print `FAIL` (`agent.test.ts`, `builtin-tools.test.ts`, `agent-generalist-memory.test.ts`, `plugins/environment.test.ts`, `plugins/todos.test.ts`), each because of open cases with no map entry. Other tracks were porting those cases when this draft was written.

## Live results

### SDK

Source: `strands-rs/docs/reviews/live.md`. The sweep command was `cargo test -p strands --all-features -- --ignored live_ --nocapture`. It ran four times; the last run (run 4) was at `34ae5dfeb`, after the telemetry review fixes and the two whole-SDK review fix rounds.

| Run | Commit | `live_` tests | Ran and passed | Skipped (key missing) | Failed |
|---|---|---|---|---|---|
| 1 | `5dbfffe94` | 66 | 53 | 13 | 0 |
| 2 | `5dbfffe94` | 66 | 53 | 13 | 0 |
| 3 | `d53f7a521` | 67 | 54 | 13 | 0 |
| 4 | `34ae5dfeb` | 67 | 54 | 13 | 0 |

Per provider:

| Provider | Test file | Tests | Result |
|---|---|---|---|
| Amazon Bedrock | `bedrock_live.rs` | 6 | all passed (region `us-west-2`, a Claude Haiku 4.5 inference profile): Converse and ConverseStream text, `maxTokens`, structured output, a tool round trip through an agent, a prompt cache hit, an invalid model id |
| Amazon Bedrock Mantle | `mantle_live.rs` | 9 | all passed (region `us-east-1`, `openai.gpt-oss-120b`): text and tool round trips on Responses and Chat Completions, streaming, structured output, a stateful conversation, multi-turn reasoning, a cache hit |
| Amazon Bedrock, cross-cutting | `live_agent.rs` (10), `live_interventions.rs` (9), `live_context.rs` (7), `live_mcp.rs` (5), `live_tools.rs` (5), `live_telemetry.rs` (2), `live_routing.rs` (1) | 39 | all passed in the four sweeps |
| Anthropic | `anthropic_live.rs` | 5 | not run: `ANTHROPIC_API_KEY` was not set, each test printed its skip line and returned, and no request was sent |
| OpenAI | `openai_live.rs` | 8 | not run: `OPENAI_API_KEY` was not set, each test printed its skip line and returned, and no request was sent |

AWS credentials came from the default chain and were used for model invoke calls only; the tests create, change and delete no AWS resource.

One test depends on the model's wording. `live_context.rs::live_summarizes_the_tool_call_in_a_single_summarizer_request` failed in 2 of 35 runs (never in the 26 runs of the test alone or with one other test; twice in 9 parallel runs of the whole file). Both failures were on its last assertion, which looks for a tracking number in the summary text the model wrote; the assertions on the request shape held in both.

The sweep found no SDK defect; four errors in the new tests themselves were corrected before the recorded runs.

Not covered by a Rust live test, as listed in `live.md`: everything in `anthropic_live.rs` and `openai_live.rs` (keys missing); Bedrock API-key authentication (no Bedrock API key in the environment), guardrails (they need a guardrail resource, and creating one was not allowed) and audio; the Mantle routing spec; the Bedrock string-model-id and region cases; the OpenAI session cache key and the stateful-model session case; PDF, document, video, citation and `Message[]` inputs of `agent.test.ts`; the timer cases of `agent.cancel.test.ts`; the streamable HTTP variants of the MCP specs (stdio only); parts of `telemetry.test.node.ts`; and the OpenAI and Anthropic cases of the summarize-tool-calls spec. The sandbox, `web_fetch` and `subagent` have no live tests and are covered by unit tests against the host, child processes, mock models and a local HTTP server.

### Harness

TODO(lead): harness live results

`harness-rs/docs/reviews/live.md` did not exist at `ba501ced3`; another track was writing it. `.rust-port/STATE.md` records that Anthropic and OpenAI keys are missing in this environment, so those two providers will be recorded as skipped there as well.

## Pending decisions for review

## Divergences

## TS bugs

## Known gaps

## Branch and commits
