# Implementation report

Markers to fill:

- TODO(lead): final test counts from the gate script
- TODO(lead): `cargo package --list` file counts for the three crates at the last commit
- TODO(lead): last commit
- TODO(lead): commit count

Every number below was read from the files of the branch or computed from them with a script, on 2026-11-17, after the Phase 12 test, live and review-fix tracks were integrated. The four numbers that depend on the last commit carry a marker.

## What was built

The branch adds a Cargo workspace at the repository root (`Cargo.toml`, resolver 3, edition 2024, minimum supported Rust version 1.96, licence Apache-2.0) with three crates, all at version `0.1.0-preview.1` and all with `publish = false`:

- `strands` (`strands-rs/strands`): the SDK.
- `strands-macros` (`strands-rs/strands-macros`): the procedural macros `#[tool]` and `#[tools]`, re-exported from `strands`.
- `strands-harness` (`harness-rs/strands-harness`): the harness, built on `strands`.

Nothing was published. The root also carries `deny.toml`, `rustfmt.toml` and `clippy.toml`, and the gate script is `.rust-port/gates.sh`.

### The `strands` crate

The crate mirrors the TypeScript SDK (`@strands-agents/sdk`) concept for concept with Rust naming, and reads and writes the same JSON for messages, sessions and snapshots. Its `src/` tree holds 368 Rust files (unit tests included).

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

Two attribute macros: `#[tool]` turns an async or sync function into a tool type (the doc comment becomes the description, and a doc comment on a parameter describes it), and `#[tools]` turns the `#[tool]` methods of an `impl` block into tools that share the value's state. The source is `lib.rs`, `args.rs`, `signature.rs` and `expand.rs`; compile-fail cases run under trybuild.

### The `strands-harness` crate

`create_harness` builds a ready-to-run agent on `strands` with the defaults of the TypeScript harness (`@strands-agents/harness`). The crate root re-exports every public name, as the TypeScript package entry point does. The modules are private except `builders`, which re-exports the twenty builder types and their state modules so that `cargo doc` shows their setters (P-1154). The root exports:

- `create_harness`, `HarnessAgentOptions` and its option types (`BuiltinToolsOption`, `CachingOption`, `McpServersOption`, `MemoryOption`, `SessionOption`, `SkillsOption`, `InterventionsOption`, `ContextManagerOption`, `Effort`, `Setting`). The builder takes tools and plugins as the SDK builder does (`.tool`, `.tools`, `.plugin`, `.plugins`, each call extending the list), and `agent_config` passes the other agent fields through.
- The portable config: `HarnessAgentConfig` and its `HarnessConfig*` types, `define_harness_agent_config`, `harness_agent_options_from_config`, `DEFAULT_HARNESS_AGENT_CONFIG`, `BUILTIN_TOOL_NAMES`, `BUILTIN_PLUGIN_NAMES`, `HarnessModuleReference`.
- The prompt: `HARNESS_CONTRACT` and `build_system_prompt`.
- Tools: `read`, `write`, `edit`, `make_web_fetch`, `make_subagent` with its presets and axes (`Preset`, `Choice`, `Fixed`, `Inherit`, `Open`, `ContextAxis`, the `*Mode` enums), `make_programmatic_tool_caller` and `programmatic_tool_caller` (on Monty), and, with `mcp`, `exa_web_search` and `make_exa_web_search`.
- Plugins: `Todos` and `EnvironmentContext` with their config types, and `TodoItem` and `TodoStatus`.
- Model helpers `supports_thinking` and `supports_web_search`, `shutdown_telemetry` (flushes the spans still in the batch before the process exits, P-1130), and the typed `HarnessError`.

The harness features `bedrock`, `anthropic`, `openai`, `mcp` and `otel` each forward to the `strands` feature of the same name; all five are on by default. The harness documents are `harness-rs/README.md`, `harness-rs/docs/{PARITY,DIVERGENCES}.md` and, for the review and live results, `harness-rs/docs/reviews/{phase-12,live}.md`.

The tracker lists twelve Phase 12 items. Eleven are implementation items, and all eleven are ticked: `create_harness` with `HarnessAgentOptions`, its defaults and the sessions layout; model strings, effort, caching and the provider tables; `HARNESS_CONTRACT` and `build_system_prompt`; the file tools, `shell`, `web_fetch` and `web_search` (native and Exa); `programmatic_tool_caller` on Monty; `subagent` with presets and axes; the `Todos` and `EnvironmentContext` plugins; the interventions grammar and `mcp_servers`; portable `HarnessAgentConfig` parsing with the module-reference errors; telemetry setup from the environment; and the live end-to-end tests per provider with the cross-SDK session resume (Bedrock and Bedrock Mantle ran; Anthropic and OpenAI skip without keys).

The twelfth item is the phase review gate. Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed the `strands-harness` crate and the SDK changes made for it, in three parts: an early review of the leaf modules at `087a6f158`, before `create_harness` was written (16 findings from the correctness reviewer and the parity validator); the full gate over the whole crate, at `c2c6d557d` for the correctness reviewer and the API bar-raiser and at `24242240e` for the parity validator (9, 21 and 14 findings); and a verification pass over the fix diff. The reviewers' reports are `.rust-port/reviews/phase12*-review-*.md`. Every finding has a disposition, with its commit, in `harness-rs/docs/reviews/phase-12.md`; the dispositions of the verification pass are in that file's last section. The findings that were not fixed as proposed are listed under "Known gaps" below.

### CI

Six workflow files for the Rust workspace are staged in `.rust-port/ci-workflows/` (`ci.yml`, `strands-rs-pr-and-push.yml`, `strands-rs-test-lint.yml`, `harness-rs-pr-and-push.yml`, `harness-rs-test-lint.yml`, `rust-security-audit.yml`). They are not under `.github/workflows/` because the token that persists the branch cannot write there (P-13). They were checked with PyYAML and actionlint, and none of them has run.

## Test counts and parity coverage

### Test counts

TODO(lead): final test counts from the gate script

The counts recorded during the run:

- At the tagged commit `fa45de4be`, `gates.sh full` was green with 4590 tests with all features and 67 ignored live tests, plus the MSRV build, each feature alone, `cargo deny` and the conformance check.
- At `da2fc2e0a`, during the Phase 12 implementation, the default `gates.sh` was green with 5101 tests with all features and 67 ignored live tests.
- At `24242240e`, the parity validator's own `gates.sh full` run for the Phase 12 gate was all 0: 5160 tests with all features and 83 ignored live tests, plus the MSRV build, each feature alone, `cargo deny` and the conformance check.
- At `6dc02c342`, after the Phase 12 review fixes, the default `gates.sh` was green with 5241 tests with all features and 84 ignored live tests (67 in the SDK and 17 in the harness).
- `cargo package --list --allow-dirty` worked at the tagged commit for `strands` (426 files) and `strands-macros` (78 files).

TODO(lead): `cargo package --list` file counts for the three crates at the last commit

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
- Cases that belong to features outside v0.1 inside an otherwise in-scope spec: the 18 multi-agent cases of `session/__tests__/session-manager.test.ts`, `__tests__/queue.test.ts` (0/10, used only by multi-agent graphs), and the MCP task timeout cases.
- Cases about APIs that TS marks deprecated: the `FileMemoryStore` search option, the `makeBash` alias and the MCP `ttl` alias.

The 59 out-of-scope spec files are multi-agent and A2A (26 files, including the `graph`, `swarm` and `a2a_client` tools), the `ContextOffloader` plugin (5), decision models (5), MCP tasks (4), steering (3), checkpointing (2), S3 storage (2), the Google and Vercel providers, the Docker and SSH sandboxes, embeddings, vector search, the Bedrock Knowledge Base memory store, GoalLoop, QMD search, `TestMemoryStore`, and the deprecated session `FileStorage` and `fileEditor`.

The "Live checklist" table of `PARITY.md` lists the TS integration specs under `strands-ts/test/integ/`: 24 are out of scope, 15 are covered by Rust live tests, the others are covered in part, covered offline, or TS-only, each with its note.

### Harness parity

`harness-rs/docs/PARITY.md` maps the 20 spec files of `harness-ts/test/` to Rust tests. Summed over its "In scope" table with a script: 456 of 485 TS test cases ported, 29 cases recorded as gaps with a reason. The harness title checker (`node strands-rs/docs/check-test-titles.mjs --parity harness-rs/docs/PARITY.md`) exits 0 and prints `ok` for all 20 rows with no case missing: 449 Rust tests carry the snake_case form of the TS title, 7 are mapped by name in `harness-rs/docs/test-title-map.json`, and the 29 gaps are declared there.

| Area (directory under `harness-ts/test/`) | Spec files | Ported/total |
|---|---|---|
| root | 12 | 292/317 |
| `plugins` | 2 | 19/20 |
| `tools` | 6 | 145/148 |

Ten spec files are ported in full. The ten with gaps are `agent.test.ts` (95/100), `builtin-tools.test.ts` (15/22), `config.test.ts` (28/32), `interventions.test.ts` (9/11), `memory.test.ts` (18/19), `models.test.ts` (105/111), `plugins/todos.test.ts` (8/9), `tools/programmatic-tool-caller.test.ts` (42/43), `tools/subagent.test.ts` (52/53) and `tools/web-fetch.test.ts` (18/19). The 29 declared gaps, by reason:

- A value the typed `BuiltinToolsOption` cannot hold: unknown names, a config object on a tool that takes none, unknown config keys and non-boolean values (11 cases: 4 in `agent.test.ts`, 7 in `builtin-tools.test.ts`; P-950). The portable config rejects the same shapes in `config/normalize.rs`.
- Google, Ollama and LiteLLM are not Rust v0.1 providers (6 cases of `models.test.ts`; decision #60).
- Other values a typed option or enum cannot hold (5 cases): `contextManager: null`, `memory: null` (P-1072), an unknown built-in plugin name (P-1073), an `interventions` value that is neither text nor a handler (P-919), and an unknown `web_fetch` transport.
- Module loading (4 cases of `config.test.ts`; decision #31): Rust loads no modules, and a module reference in any of the nine positions is the unsupported-module-reference error, which `rejects_a_module_reference_in_every_position` asserts (P-1201).
- States that cannot be built in Rust (2 cases): `ToolAgent::call_tool` always reaches the hooks, and an `Agent` stream always ends with a result or an error (P-898).
- `cedar-policy` is an unconditional dependency of `strands`, so there is no install on which the Cedar loader could fail (1 case; P-919).

The six `programmatic_tool_caller over MCP` cases need `node` on `PATH` and the repository root's `node_modules`; without them each prints a `SKIPPED` line and passes (P-1198). The staged harness CI workflow installs the Node dependencies before `cargo test`. The "Live checklist" table of the harness `PARITY.md` lists the three TS integration specs, all covered by Rust live tests: `e2e.test.ts` (10 of 10 cases), `caching.test.ts` (1 of 1) and `web-search.test.ts` (1 of 1).

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

`harness-rs/docs/reviews/live.md` records the 17 harness live tests (`harness-rs/strands-harness/tests/live_e2e.rs` and `tests/cross_sdk/`). The sweep ran four times: twice at `7273873c6` with 13 tests (11 passed, 2 skipped) and twice at `87521c86a` with 16 tests (14 passed, 2 skipped). No test failed in any run, and the 2 skips are the missing keys. The seventeenth test, the environment-context case, was added on 2026-11-17 and ran once on its own (1 passed). AWS credentials were used for Bedrock and Bedrock Mantle model calls only, in `us-west-2`; no request was sent to Anthropic or OpenAI.

One flow runs per provider: a `write` call gated by `interventions: "ask"`, a call of the `fixture_echo` tool of the MCP fixture server, the interrupt and its resume, and a second `create_harness` on the same session id that restores an equal history and reads the file.

| Provider | Model | Result |
|---|---|---|
| Bedrock | `bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0`, effort `off` | pass |
| Bedrock Mantle | `bedrock-mantle/openai.gpt-oss-120b`, effort `low` | pass |
| Anthropic | `anthropic/claude-haiku-4-5`, effort `off` | skipped: `ANTHROPIC_API_KEY is not set` |
| OpenAI | `openai/gpt-5.4-mini`, effort `off` | skipped: `OPENAI_API_KEY is not set` |

The Bedrock Mantle test runs at effort `low` (P-1092). With effort `off` the harness sends `reasoning.effort: "none"`, as the TypeScript harness does, and `openai.gpt-oss-120b` answered every such request with a server error (3 of 3 from Rust, 1 of 1 from the TypeScript harness); this is in the TS bug list. One earlier run of the Mantle test at `low` failed with a 503 capacity error and passed when repeated.

Nine tests run on Bedrock only, on the same Claude Haiku 4.5 model, and all passed: the shell tool; the file and web tools, the todos plugin and the subagent in one turn; a denied tool call through interventions; a pending interrupt approved by a second agent on the same session; a session with a minted id; memory across agents; cache reads that grow across turns; Exa web search through the public tool, keyless; and the environment context read from a project `AGENTS.md` through a sandbox whose working directory is the temporary directory (P-1180).

The cross-SDK session check has four tests, with both SDKs on the same Bedrock model and the same `./.agent/sessions` directory; the TypeScript side is `createHarness` from a scratch copy of `harness-ts` at the pin. TypeScript writes a session and Rust resumes it; Rust writes and TypeScript resumes; TypeScript stops on a tool approval and Rust approves it; Rust stops and TypeScript approves. All four passed in every run that had them. Histories are compared as JSON values of the messages in their wire shape, and no format difference was found in either direction, in the history or in the stored interrupt state.

Not covered, as `live.md` lists: Anthropic and OpenAI (the two tests are kept and skip without their keys); the environment plugin in the six-tool and caching tests, where TS enables it (P-1094); and, in the cross-SDK check, the context manager's stash files, memory files and multi-agent sessions. The Exa test skips when the keyless rate limit is hit. The first runs found three errors in the new tests themselves and no harness defect.

## Pending decisions for review

`.rust-port/DECISIONS-pending.md` holds 535 entries, numbered P-1 to P-1201 with gaps between the ranges that parallel tracks reserved. Each entry gives the context, the options, the choice, the TS reference and the files. Later entries amend or supersede earlier ones and say so in their titles.

### Entries marked as owner decisions

These entries end with a question the run left open.

- P-1023 (with P-815): module shape. The file modules `hooks::{events, registry, types}`, `interventions::{actions, handler}`, the four `retry` file modules, `telemetry::{meter, tracer}`, `memory::extraction`, `agent::invocation_context` and `tools::{executors, structured_output}` are public but `#[doc(hidden)]`, and every item in them is also in the parent's flat re-export. The reviewer's judgement is that the hidden file-stem paths are acceptable while the crate is unpublished and become a semver commitment to the file layout in a published 0.1.0. The open call is whether they become `pub(crate)` before publishing, and whether the families that keep two documented paths by decision (`tools::{tool, registry, function_tool}`, `models::{model, streaming}`, the provider modules) should drop one.
- P-26 and P-646 (with P-5 and P-416): Cedar as a non-optional dependency. `cedar-policy` is compiled into every `strands` build, where TS makes the Cedar packages optional peers, and `strands` carries the exact requirement `cedar-policy = "=4.14.0"` that the schema generator crate sets, so a downstream crate cannot use another 4.x patch until the generator republishes. A default-on `cedar` feature, or a feature around schema generation, would change the five-feature list of PROMPT.md section 8.1, so neither was applied. Moving Cedar behind a feature after 0.1 would break `default-features = false` users.
- P-832: the harness `supports_media` judges a model instance by its model id, because `Model` has no downcast. An `OpenAIModel` on Bedrock Mantle passed as an instance with an `openai.*` id gets `false` where TS answers `true`, so `read` describes media in text for it unless `builtinTools.read.media` overrides. An SDK accessor would remove the difference.
- P-873 and P-991: programmatic tool callers are recognised by name, because the `Tool` trait has no identity or downcast. A live caller anywhere in the process hides a same-named ordinary tool from every caller. Matching by identity needs a marker or downcast on the SDK `Tool`.
- P-913: `EnvironmentContext` probes the platform and working directory through the sandbox, as TS at the pin does and as its tests assert. PROMPT.md section 11 item 40 says to use the native platform (`std::env::consts::OS`). The port follows TS; only the date is native.
- P-1120 (replaces P-938): the portable config bridge applies the `agentConfig` keys that have a JSON form, with the TS precedence between `agentConfig` and the top-level keys. Keys whose TS values are live objects or that have no Rust field (`tools`, `plugins`, `conversationManager`, `structuredOutputSchema`, `sessionManager`, `memoryManager`, `checkpointing`, `storage`) are the error `UnsupportedAgentConfigKeys`. The six keys the top-level config always sets (`model`, `effort`, `contextManager`, `session`, `skills`, `memory`) are skipped without a warning, and a key neither SDK reads is dropped with one warning. The open call is whether a key neither SDK reads should be an error too, and whether the six always-overridden keys should warn.
- P-958 and P-1142: built-in plugins are recognised by what they contribute, because decision #15 keeps downcasting out of the public API. A consumer plugin stands in for a built-in one when it has the built-in's name or vends the same tool specs (which recognises a `Todos` under a custom name), and the built-in `EnvironmentContext` injects nothing into a turn that already holds an `<environment>` block. What remains: the built-in `EnvironmentContext` is registered and idle next to a consumer one with a custom name, and a plugin of another type that vends an identical `todo_write` spec, or injects a block with the same opening, is taken for the built-in type. The open call is to accept this or to add a typed marker to the SDK `Plugin` trait (for example a `kind()` string defaulting to `name()`), which is an SDK API addition.
- P-1140: an agent that lists an `McpClient`'s tools adds its own change queue to the client and leaves a callback the consumer set with `set_on_tools_changed` in place, where TS replaces it when an agent initializes. The open call is to confirm that keeping the consumer callback is wanted.
- P-1141: when the caller's answers resume a background task after the model ended its turn, Rust waits for the task and delivers its result before the next model call. With `wait_for_completion: false` nothing may wait, so the model is still called with a history that ends in an assistant message, as in TS. The entry gives three options for that case: the agent loop skips the model call of a pass that added no message (a loop change, for both SDKs); the scheduler returns the interrupt only on the caller's next invocation; or the behaviour is kept and documented.
- P-1168: the harness list constants over `#[non_exhaustive]` enums became slices. The SDK has the same shape in `SnapshotField::ALL: [Self; 5]` and `SnapshotPreset::ALL: [Self; 1]`, which were not changed; the entry recommends `&'static [Self]` before the first release.
- P-1169: `strands-harness` was removed from `[workspace.dependencies]`, because no member depends on it and cargo warned about the unused entry on every command. PROMPT.md section 8.1 says to declare all three crates there. The alternative is to restore the line and accept the warning until a member depends on the harness.
- P-1172: `HarnessError::ModelFeatureDisabled` says `requires the strands feature`, a text fixed by PROMPT.md section 10, although the gate is the `strands-harness` feature of the same name. An application with `strands/anthropic` on and `strands-harness` built without default features is told to enable a feature it already has. The reviewer proposes `requires the strands-harness feature` and folding the variant into `FeatureDisabled`; only the rustdoc was changed.
- P-1180: the live environment-context test defines its own sandbox, because `NotASandboxLocalEnvironment` has no public working-directory setting. The open call is whether it should get one (TS has none).

Closed since they were raised: P-993 (three subagent constants public at the harness crate root) by P-1110, which made them crate-internal as in harness-ts; P-923 (`shutdown_telemetry` not reachable by a caller) by P-1130, which made it public; and the question P-1166 left about `InvalidEffort.supported`, which became a `Vec<String>` in `89e0cb4db`.

### Entries that change behaviour or public API the most

- P-49 and P-412: hook callbacks return `()` and cannot fail. TS callbacks may throw. Interrupts are recorded on the event (`BeforeToolCallEvent::interrupt`, `BeforeToolsEvent::interrupt`) and the registry runs every callback. An intervention handler with `on_error: throw` fails the event through a crate-internal channel, so the fail-closed default holds. A TS hook that throws has no direct Rust form.
- P-91 (with P-98, P-740, P-792): dropping an `AgentStream` is the Rust form of cancelling. A drop guard repairs the agent synchronously, the hooks the dropped stream skipped are deferred to the next use, `Agent::shutdown()` stands in for TS `await using`, and a drop with pending work logs a warning. This is behaviour TS does not have in this form.
- P-68: `Agent::builder().build()` is synchronous and loads no AWS configuration; the default `BedrockModel` is built on the first model call. A missing `bedrock` feature therefore fails at the first call, not at construction.
- P-63: a panicking tool becomes an error result that still goes through `AfterToolCallEvent` hooks and retries, with a warning logged.
- P-57: `#[tool]` on a function without a doc comment is a compile error. In TS a tool without a description fails when it is added to an agent.
- P-16 and P-38: TS string unions that accept any string at runtime are `#[non_exhaustive]` enums with an `Other` variant (`StopReason`, the media formats, `ToolResultStatus` and others); struct-shaped variants are `#[non_exhaustive]` with constructors. This fixes the matching and construction style of the public types.
- P-43: the token estimate counts UTF-16 code units, as TS `text.length` does, where PROMPT.md section 7.3 says to count by `char`. The estimates are numbers that thresholds compare across SDKs.
- P-212: Bedrock Mantle is the constructor `OpenAIModel::with_bedrock_mantle(options, config)`, not an options field, so turning the `bedrock` feature on does not break struct literals written against `openai` alone.
- P-263 and P-264: there is no process-global default sandbox; each agent builds its own `NotASandboxLocalEnvironment`, which is public. `sandbox: SandboxSource::Off` fails with `DefaultNotConfiguredError` when a tool needs a sandbox, where TS on Node treats `false` like an unset key and runs on the host.
- P-237: MCP has no `TasksConfig`; a 60 second default applies to every request and a per-call timeout also caps the total.
- P-318 and P-384: two parsers differ from the TS libraries. `retrieve_context` patterns compile with the `regex` crate (no backreferences or lookaround), and `SKILL.md` frontmatter is read by a YAML subset parser with no YAML dependency (extended by P-603).
- P-411: when any intervention handler is configured, all five gates are registered and every handler is called, because a Rust trait cannot tell an overridden default method. The outcome is the same; the callback count differs.
- P-456: `ClassifierStrategy` takes a plain `Model` and makes the one classification call itself, because decision models are out of v0.1.
- P-480 (with P-481 to P-497): agent spans are `tracing` spans exported through `tracing-opentelemetry`, where TS starts OpenTelemetry spans directly. P-487 keeps `AgentMetrics` serialize-only.
- P-644: Cedar is evaluated through `cedar_policy::ffi` with the JSON that TS sends, which gives the same answers and error texts as the WASM build.
- P-681: `Agent::as_tool` returns a `Result`, so the TS session-manager validation has a place to fail.
- P-799: a signalled cancel no longer waits for the next item of a silent model stream.
- P-1000: the interrupts of a tool batch leave the cycle as an error, so the cycle span ends as it does in TS.
- P-870, P-871, P-891, P-990, P-1050, P-1150 and P-1165 (which replaces P-992): public SDK additions made for the harness: `ToolAgent::call_tool(name, id_prefix, input, invocation_state)`, which mints the tool-use id `<id_prefix><hyphenated uuid>` on every call (the harness passes `ptc_`), `ToolCallOutcome` (with `HookCancelled`), `InvocationState::detached()`, `CancelSignal::abort_only`, `Agent::interventions()` and `SharedSource::into_arc`.
- P-1195: typed tool input drops the keys its schema does not declare before validation, as zod's default `strip` does, so a call with an extra argument runs the tool. The change is in the one validator behind `FunctionTool::typed`, `#[tool]`, the vended tools and the harness tools. Published schemas are unchanged, an input type with `#[serde(deny_unknown_fields)]` still rejects, and a key inside a union branch, a record value or a recursive `$ref` type is still a validation error.
- P-1140: tools-list changes reach every agent on an `McpClient`. In TS the client has one callback slot, so a delegate built on the parent's client stops the parent's refresh; this is in the TS bug list.
- P-1141: a background task resumed after the model ended its turn is waited for before the next model call when `wait_for_completion` is on, so the request ends in the delivered tool result where TS calls the model with nothing new. This is in the TS bug list as well.
- P-1131: the model call reads the tool registry after the `BeforeModelCallEvent` hooks and on every attempt, as TS does, so a tool a hook adds or removes is in that call's request. The token estimate keeps the earlier read, as in TS.
- P-930 to P-933, P-1151 and P-1154: the shape of `HarnessAgentOptions`: one enum per polymorphic TS option and `Setting<C>` in place of TS `ToolConfig<K>`; `tools` and `plugins` are builder fields with `.tool`, `.tools`, `.plugin` and `.plugins`, each call extending the list; and the twenty builders are public in one module, `strands_harness::builders`, the crate's only public module.
- P-1120: `agentConfig` keys in a portable config that TS would act on and Rust cannot apply are errors; under P-938 they were dropped with a warning (see the owner decisions above).
- P-1152 and P-1153: each nested portable config type deserializes through the normalizer of its config key, so a value read on its own fails with the same text as inside a whole config; `ProgrammaticToolCallerConfig::timeout` is an `Option<Option<Duration>>` written in seconds.
- P-1166 and P-1171: `HarnessError` variants carry typed fields (`Effort`, `Vec<String>`, an optional `serde_json::Error` source) with unchanged message texts, and `AgentSpec` and `TodoItem` are `#[non_exhaustive]`.
- P-1196 and P-1200: the Exa notice and the two MCP-client warnings are logged on every call, as in TS, and the `direct` transport of `web_fetch` refuses a URL that carries credentials, and a redirect to one, before any request.
- P-1130 (closes P-923): `shutdown_telemetry` is public in the harness, with and without the `otel` feature, and has no TS counterpart. TS flushes the batch exporter on Node's `beforeExit`; Rust has no exit hook, so a process that exits right after an invocation exports nothing unless it awaits this call.

### The rest, by area

| Area | Entries |
|---|---|
| Run mechanics, workspace, dependencies, lints, CI staging | P-1 to P-13, P-27, P-28 |
| Types, wire format and errors | P-14 to P-26, P-30 to P-39 |
| Model abstraction, streaming and retry | P-29, P-40 to P-47, P-52 to P-56 |
| Hooks, tools, macros and the agent loop | P-48 to P-51, P-57 to P-89 |
| Interrupt resume, cancellation, lifecycle, plugins, middleware, background tasks, conversation managers | P-90 to P-102 |
| Bedrock provider | P-150 to P-161 |
| Anthropic provider | P-180 to P-188 |
| OpenAI provider and Bedrock Mantle | P-200 to P-207 |
| Provider review fixes | P-208 to P-220 |
| MCP and `mcp_router` | P-230 to P-259, P-341 |
| Sandbox and vended tools | P-260 to P-286 |
| Storage and sessions | P-300 to P-304 |
| Context manager | P-310 to P-322, P-620 to P-626 |
| Memory, injection and skills | P-350 to P-358, P-380 to P-386, P-390 to P-394, P-600 to P-609 |
| A plugin initialization failure (P-340) and a compile-fail test case (P-342) | P-340, P-342 |
| Interventions, human-in-the-loop and Cedar | P-410 to P-416, P-640 to P-649 |
| Model routing | P-450 to P-457, P-660, P-661 |
| Telemetry | P-480 to P-497, P-700 to P-704, P-720 to P-728 |
| Crate docs and examples | P-520 to P-525 |
| Parity sweep and the title checker | P-540 to P-544, P-580 to P-586, P-683 |
| Live tests | P-560, P-561 |
| Whole-SDK review fixes | P-680 to P-684, P-740 to P-745, P-760 to P-772, P-790 to P-804, P-810 to P-818, P-1000 to P-1005, P-1020 to P-1027, P-1040 to P-1042 |
| Harness model strings and prompt | P-830 to P-838 |
| Harness file tools, shell, `web_fetch` and `web_search` | P-850 to P-856 |
| Harness `programmatic_tool_caller` | P-870 to P-876 |
| Harness `subagent` | P-890 to P-900 |
| Harness plugins, interventions and telemetry | P-910 to P-926 |
| Harness options and portable config | P-930 to P-941 |
| `create_harness` and the built-in tools | P-950 to P-963 |
| Harness test infrastructure | P-980 to P-983 |
| Harness leaf-module review | P-990 to P-993, P-1110, P-1111 |
| Harness test porting (`create_harness`, built-in tools, delegates, memory, plugins, portable config end to end) | P-1050 to P-1055, P-1070 to P-1075 |
| Harness live tests and the cross-SDK session check | P-1090 to P-1095, P-1180 |
| Harness review gate, correctness findings | P-1120 to P-1122, P-1130, P-1131, P-1140 to P-1142 |
| Harness review gate, API findings | P-1150 to P-1154, P-1165 to P-1172, P-1181 to P-1183 |
| Harness review gate, parity findings | P-1195 to P-1201 |

The last five rows hold the 50 entries logged by the Phase 12 test-porting, live and review-fix tracks. Each entry of the three review-gate rows names the finding it answers; the disposition of that finding is in `harness-rs/docs/reviews/phase-12.md`.

## Divergences

Both divergence files list every user-visible behaviour difference from TypeScript with its reason, one table row each, and both open with the rule that a divergence never changes a cross-SDK wire or persisted format.

### SDK

`strands-rs/docs/DIVERGENCES.md` has 200 rows. Grouped here by subject:

| Subject | Rows |
|---|---|
| Hooks, tools, the agent loop, streams, lifecycle, middleware, plugins, background tasks | 44 |
| Providers (Bedrock 10, Anthropic 8, OpenAI and Bedrock Mantle 6) | 24 |
| Memory | 19 |
| MCP | 18 |
| Wire and persisted data, types | 17 |
| Telemetry and logging | 17 |
| Sandbox and vended tools | 16 |
| Interventions, human-in-the-loop, Cedar | 12 |
| Conversation managers and the context manager | 10 |
| Injection and skills | 9 |
| Model routing | 9 |
| Sessions and storage | 5 |

The rows a user of the TS SDK meets first:

- Concurrent invocations of one agent are a compile error (`invoke` and `stream` take `&mut self`), where TS raises `ConcurrentInvocationError` at run time (decision #22).
- The agent stream has no return value; its last item is `AgentStreamEvent::AgentResult`, and `invoke` drains the stream and returns it.
- Dropping an `AgentStream` is a cancel: the signal is cancelled, running tool tasks are aborted and an open user turn is closed with the cancel message.
- An agent that goes out of scope without `shutdown().await` saves nothing, because `Drop` cannot await; the drop logs a warning when the memory manager still holds unsaved conversation (decision #64).
- A hook callback cannot fail, and a hook raises an interrupt through the event.
- A tool that panics becomes an error result.
- There is no `configure_logging` and no `Logger`; the crate logs through `tracing` events in the TS text format and prints nothing until the application installs a subscriber.
- Span events and dynamic span attributes are recorded only with the `otel` feature; without it the `tracing` spans carry the static `gen_ai.*` fields.
- `sandbox: false` fails with `DefaultNotConfiguredError` when a tool needs a sandbox, and each agent has its own default sandbox instance.
- Saved JSON nested deeper than 500 levels is a parse error, where `JSON.parse` loads any depth.
- Message tracking ids are UUID v7, where TS uses UUID v4; any non-empty string is still accepted.
- `Uint8Array` fields that TS saved as index objects are read back as the original bytes (TS corrupts them; see the TS bug list).
- Typed tool input with a key the schema does not declare: the key is dropped under `properties` and `items`, as zod drops it, and the tool runs; inside a union branch, a record value or a recursive `$ref` type it is still a validation error, where zod drops it at every level.

Four of the 200 rows were added by the harness work, because the difference is in SDK behaviour: `Agent::interventions()` (P-1050), several agents on one `McpClient` (P-1140), a background task that asks for input after the model ended its turn (P-1141), and the typed tool input row above (P-1195).

### Harness

`harness-rs/docs/DIVERGENCES.md` has 52 rows, each with the area, the TS behaviour, the Rust behaviour and the reason (counted with a script; no row has fewer than four cells). By subject:

| Subject | Rows |
|---|---|
| `programmatic_tool_caller`, including the Monty runtime in place of the TS one | 12 |
| The portable config and `HarnessAgentOptions` | 8 |
| `create_harness` and the built-in tool resolution | 8 |
| Model strings and providers that Rust does not build | 7 |
| `web_fetch` and `web_search` | 7 |
| The file tools | 2 |
| `subagent` | 2 |
| Interventions | 2 |
| Telemetry | 2 |
| `EnvironmentContext` | 1 |
| Logging | 1 |

The `create_harness` group holds the shell probe cache, the warning for an unnamed MCP client, `web_search: 'exa'` and `mcp_servers` without the `mcp` feature, `caching: 'auto'` passed explicitly, JS `trim`, the absent `internal` entry point, and how a consumer plugin that stands in for a built-in one is recognised. The two telemetry rows are telemetry without the `otel` feature and flushing spans at exit, which is the caller's `shutdown_telemetry().await` in Rust and a `beforeExit` hook in TS. The logging row says that the harness logs through `tracing`, has no `configure_logging`, and prints nothing until the application installs a subscriber, where TS prints warnings to the console by default.

The Phase 12 review gate changed the file as follows. Five rows were added: the `internal` entry point, flushing spans at exit, plugin recognition, logging, and an explicit `allowedTools: null` that the normalized config does not write back. The rows for `effort` outside the enum, `agentConfig` and `subagent.maxDepth` were rewritten after the fixes to those areas. Four rows that had two cells were completed to four, and a fifth two-cell row, on plugin recognition, was removed because the new four-cell row covers it (P-1182).

## TS bugs

`.rust-port/ts-bugs.md` records 42 defects found in the TypeScript sources while porting, each with `file:line`, what is wrong, a minimal repro, and how Rust handles it. No TypeScript file was changed. By area:

- Types and wire (3): `Uint8Array` fields are written two ways and the index-object form corrupts on reload; an `UnknownBlock` can be built around a known block; a block after unparseable tool input turns into a stale tool use.
- Agent loop, hooks and tools (8): concurrent tool interrupts fire `InterruptEvent` for one tool only; a hook-cancelled invocation skips `MessageAddedEvent` and `AgentResultEvent` hooks; concurrent tools end the wrong local trace; duplicate retry strategy types warn and then fail; delegation re-dispatches the child's events to the parent's hooks; `AgentToolStreamer` classifies a nested delegate's result; `agent.cancel()` during `initialize()` is accepted and then discarded; a snapshot's interrupt `context` aliases the live state.
- Providers (5): a throttled Bedrock request is not retried unless the throttle arrives inside the stream; unknown `additionalArgs` keys are dropped silently; Chat Completions parallel tool-call deltas interleave across content blocks; Chat Completions drops tool calls whose arguments arrive before the id; a Responses stream that ends without a terminal event reads as a complete answer.
- Sandbox and vended tools (3): `buildShellEnvPrefix` and `shellQuote` are documented for implementers but not exported; an empty context `Choice` passes `makeSubagent` validation and yields an empty enum; `mcp_router` `call_tool` errors reach the model with a doubled `Error: ` prefix.
- Conversation and context management (3): tool-result truncation drops the message metadata; `compressionThreshold: NaN` passes validation; `pin_context` advertises `action` as required although it has a default.
- Storage (2): a generic namespace view with a trailing-slash prefix lists truncated keys; sandboxed `LocalFileStorage` writes skip the search strategy's `index`.
- Skills (2): the skill location is always `<path>/SKILL.md`, also for a skill loaded from `skill.md`; a bare `SKILL.md` path source never loads in strict mode.
- Memory (3): `FileMemoryStore.add` drops a second heading-less entry written in the same millisecond; `ModelExtractor` forwards non-object `metadata` from the model; `search_memory` accepts any number for `maxSearchResults`.
- Interventions (4): `CedarAuthorization` leaks a call count when the context enricher throws; the `HumanInTheLoop` approval prompt loses the classifier reason on resume; a hook that throws at a tool gate still runs the next model call; intervention gates run on the pass an earlier hook pauses, and again on resume.
- MCP and background tasks (2): a second agent on one `McpClient` stops the first from refreshing its MCP tools, which every harness delegation triggers; a background task that interrupts after the model ended its turn makes the resumed invocation call the model with nothing new.
- Harness (7): the `edit` tool expands `$` patterns in `new_str`; the `edit` tool accepts an empty `old_str` on some files; the `read` tool reads a negative `limit` as an end offset; `web_fetch` can cut a surrogate pair; the `subagent` tool does not forward the invocation context; the `web_fetch` URL cache is keyed by URL alone and never shrinks; effort `off` on Bedrock Mantle sends `reasoning.effort: "none"` to models that reject it.

Eight entries say that Rust keeps the TS behaviour so the two SDKs act alike: the sandboxed `LocalFileStorage` writes, the `CedarAuthorization` call count, the `HumanInTheLoop` prompt on resume, the intervention gates on a paused pass, the empty `old_str` and the negative `limit` of the harness file tools, the harness `subagent` invocation context, and effort `off` on Bedrock Mantle. The other entries say how Rust handles the case differently; the user-visible ones are also rows in `DIVERGENCES.md`. The background-task entry is in both groups: Rust waits for the resumed task when `wait_for_completion` is on and does what TS does when it is off (P-1141).

Three of the 42 entries were recorded in Phase 12. The Bedrock Mantle effort entry came from the harness live runs: `openai.gpt-oss-120b` answered every request with `reasoning.effort: "none"` with a server error, from both harnesses (P-1092). The `McpClient` and background-task entries came from the correctness reviewer of the review gate, whose TypeScript repro scripts are under `.rust-port/reviews/phase12-review-correctness-probe/ts-diff/` (P-1140, P-1141).

## Known gaps

### Out of v0.1 by design

PROMPT.md section 5.2 lists what was not built: Graph, Swarm and A2A with their vended tools; bidirectional streaming; the `ContextOffloader` plugin, GoalLoop, steering and checkpointing; decision models and the decision-model classifier; Docker, SSH and OS-level sandboxes; S3 storage, vector search, embeddings and the Bedrock Knowledge Base memory store; MCP tasks; the Google, Vercel, Ollama and LiteLLM providers; module references in portable harness config (they produce an "unsupported" error); everything TS marks `@deprecated`; `configureLogging`; a Rust CLI; the docs-site Rust tabs; and parity automation. `FOLLOW-UPS.md` carries one line for each.

### Tests and live coverage

- 322 TS test cases of in-scope SDK specs and 29 of the harness specs are recorded as gaps with reasons (see "SDK parity" and "Harness parity" above).
- No Anthropic or OpenAI request was sent: the 13 SDK live tests and the 2 harness live tests of those two providers are written and skip without their keys. The Bedrock API-key path, guardrails and audio have no live run either, and the other uncovered cases are listed under "Live results".
- One live test depends on the model's wording and failed in 2 of 35 runs (see "Live results").
- Some ported harness cases assert less than their TS counterparts, as their `PARITY.md` rows and the track handoffs (`.rust-port/handoff/h12a-handoff.md`, `h12b-handoff.md`) say: the provider-model cases check what `create_harness` asks `resolve_model` for and not fields of the built model object; the two `openai-cache.test.ts` cases run the `openai/` rows against a localhost server and not the `bedrock-mantle/` rows (P-1074); the durable-checkpoint case counts `<environment>` blocks where TS asserts the date of each turn; and the `createHarness` memory cases assert identity by behaviour (P-1071).
- The six `programmatic_tool_caller over MCP` cases of the harness pass without running where `node` or the repository's `node_modules` is absent (P-1198).
- The six CI workflows were checked with PyYAML and actionlint (P-13) and have never run, because they cannot be pushed under `.github/workflows/` from this environment.
- The tag `strands-rs-v0.1.0-preview.1` is local only; tags were not pushed. In a fresh clone it has to be recreated on `fa45de4be`.
- Rust code is not scored by the repository's `pr-metrics` complexity script; no adapter was written because a new CI binary could not be validated here (P-12).

### Review findings that were not fixed as proposed

Phases 0 to 11 each closed with three independent reviewers (correctness, API, parity) and a disposition for each finding in `strands-rs/docs/reviews/phase-N.md`. The dispositions that are not a plain fix, with their reasons:

- Phase 0, API 9, and Phase 7, api-3: `cedar-policy` is a non-optional dependency with an exact version requirement. Not applied, because PROMPT.md section 8.1 makes it non-optional and names five features; recorded for the owner (P-26, P-646). The documentation part of api-3 was done.
- Phase 2, Correctness 5: learned structured-output support is kept per provider instance, to avoid process-global mutable state; recorded as a divergence (P-53).
- Phase 2, API 3: `update_config` cannot clear a field; documented (`None` keeps the current value) and recorded as a divergence.
- Phase 3, API 17: TS `toolUsage` and `accumulatedData` are mapped to existing accessors; recorded as a divergence.
- Phase 4, api-3: the name `BackgroundTasks` is kept for the config-key union and recorded in the name map (P-101).
- Phase 5, correctness-4: `eventsource-stream` stays the SSE decoder, so an Anthropic `event: error` frame with no data is dropped; recorded as a divergence (P-211).
- Phase 5, api-7: `CacheTtl` and `BedrockCacheTtl` keep Rust acronym casing, because PROMPT.md names `CacheTtl` (P-216).
- Phase 6, correctness-6: fixed in part. The stash reads entries nested up to 500 levels. The proposed "skip and log an unreadable entry in `take_snapshot`" was not applied, because TS `takeSnapshot` rejects in that case and section 7.4 forbids Rust-only behaviour.
- Phase 7, correctness (gates run on the paused pass and again on resume): no code change; Rust does the same as TS and the case is in the TS bug list.
- Phase 9, api-7: `client_version`, `negotiated_protocol_version` and `McpTool::server_name` are kept as small accessors, each with its reason (P-259).
- Phase 10, correctness-5: fixed in the documentation (a provider `shutdown()` blocks and belongs in `spawn_blocking`). Left open there: the harness exit path has to do the same. The harness exports `shutdown_telemetry` for it, which runs the provider's `shutdown()` on `spawn_blocking` (P-1130); the caller awaits it before the process exits, and nothing in the harness calls it on its own.
- Phase 11, api-b4 and api-c8: the file modules of six families are `#[doc(hidden)]` and not private; pending for the owner (P-815, P-1023).
- Phase 11, root exports: `SnapshotManifest` and `SnapshotLocation` stay crate-private, out of v0.1 with `SnapshotStorage` (P-766).

Phase 12 has its dispositions in `harness-rs/docs/reviews/phase-12.md`, in three parts: the early review of the leaf modules (16 findings), the full gate over the whole crate (9 correctness, 21 API and 14 parity findings, and two items fixed alongside them), and the verification pass over the fix diff, whose dispositions are in that file's last section. The dispositions of the first two parts that are not a plain fix, with their reasons:

- Early review, correctness-a3: fixed in part (P-991, narrowing P-873). A dropped programmatic tool caller no longer reserves its name. A live caller still hides a same-named ordinary tool process-wide, because identity on the SDK `Tool` was not added; this is one of the owner decisions above.
- Early review, correctness-a6: fixed without a regression test, because a failure to build the `reqwest` client cannot be injected.
- correctness-3 and api-15: fixed without `Any`, `TypeId` or a downcast, which decision #15 keeps out of the public API. A consumer `Todos` or `EnvironmentContext` under its own name stands in for the built-in one by its name or by the tool specs it vends, and the built-in `EnvironmentContext` adds no second `<environment>` block. The differences that remain are in the divergence row and are left to the owner (P-1142).
- correctness-4 and api-7: fixed for every `Debug` reachable from `HarnessAgentOptions` and `HarnessAgentConfig`, and, alongside the findings, for the SDK's `McpServerConfig`, its builder and `McpServers::Json` (P-1199). Not changed: the SDK's `StdioServerParameters` (`strands-rs/strands/src/mcp/transport.rs`) derives `Debug` and prints its `env` values; the file was outside the fix track's files.
- correctness-5: fixed in part, in the SDK. With the default `wait_for_completion: true`, which the harness uses, a background task resumed after the model ended its turn is waited for and its result delivered before the next model call. With `wait_for_completion: false` nothing may wait, so the sequence stays as in TS; that case needs a decision about the agent loop and is left to the owner (P-1141).
- api-2: fixed with a public `builders` module (P-1154). Left: the crate rustdoc in `lib.rs` still says that the modules are private, and `harness-rs/AGENTS.md` does not name the module.
- api-9: fixed, with one variant done differently. The reviewer proposed replacing the `message` of `InvalidAgentConfigValue` by the serde error; the variant keeps `message` and gains an optional `serde_json::Error` source, because nine of its eleven call sites reject a value with a harness-written reason and have no serde error (P-1166).
- api-12: fixed for the two public harness constants, which are slices. The SDK's `SnapshotField::ALL` and `SnapshotPreset::ALL` keep their array types, because they are outside the Phase 12 diff; left to the owner (P-1168).
- api-13: fixed by removing the unused `strands-harness` entry from `[workspace.dependencies]`, where PROMPT.md section 8.1 lists the crate among those to declare there; the entry with the alternative is left to the owner (P-1169).
- api-14 and parity-9: fixed in the README, the crate docs and a divergence row. The sentences about the subscriber order when `OTEL_TRACES_EXPORTER` is set come from reading the SDK source; no test runs that path, because it needs the process environment changed.
- api-18: not changed in behaviour. The text of `HarnessError::ModelFeatureDisabled` is fixed by PROMPT.md section 10 and stays byte-identical; the rustdoc now says which feature is the gate, and the reviewer's proposal for the text is left to the owner (P-1172).
- api-20: fixed (P-1153), with one observable change recorded as a divergence row: an explicit `allowedTools: null` is not written back by the normalized config, where TS keeps it.
- api-21: fixed differently. `tools` and `plugins` are builder fields with `.tool`, `.tools`, `.plugin` and `.plugins`, not the reviewer's `#[builder(with = ..)]`, because a bon setter can be called once and could not extend the list. It needed `SharedSource::into_arc` public in the SDK (P-1150).
- parity-5: fixed with assertions on behaviour. The four background-task cases run one scripted tool call through a built harness; `Agent` has no accessor for the policy and none was added (P-1197).
- parity-12: fixed in the test file. The six `programmatic_tool_caller over MCP` cases find `node` and the repository's `node_modules` themselves; where either is absent, each case prints a `SKIPPED` line and passes (P-1198).
- parity-13: fixed in the SDK as proposed (P-1195). The strip of undeclared keys does not reach a union branch, a record value or a recursive `$ref` type, where the key is still a validation error; this is a row of the SDK divergence file.

### Where a binding decision and the evidence disagreed

- Skipped because it conflicted with a binding decision: a default-on `cedar` feature, and a feature around Cedar schema generation (P-26, P-646; section 8.1 fixes the dependency as non-optional and the feature list at five).
- Applied against the prompt text, each logged for review: the Cedar pin moved to `cedar-policy = "=4.14.0"` because the schema generator crate requires it (P-416); the token estimate counts UTF-16 code units where section 7.3 says `char` (P-43); `EnvironmentContext` probes the platform through the sandbox where section 11 item 40 says the native platform (P-913).
- Followed the design where TS differs: `sandbox: false` fails with `DefaultNotConfiguredError` (P-264), and there is no process-global default sandbox (P-263).

## Branch and commits

- Branch: `feat/rust-sdk`, pushed to the fork remote as `feat/rust-sdk-run-20261115`. Nothing was pushed to `strands-agents/harness-sdk`, and no PR or issue was opened.
- Base (the pin): `7296caba7` on `main`.
- Paths: the diff from the base touches `strands-rs/`, `harness-rs/`, `.rust-port/`, the root `Cargo.toml`, `Cargo.lock`, `deny.toml`, `clippy.toml`, `rustfmt.toml` and the root `AGENTS.md`. Nothing under `strands-ts/`, `harness-ts/`, `strands-py/`, `harness-py/`, `strands-cli/` or `site/` was changed.
- First commit: `3a67c206f` ("chore(rust-port): create the run folder").
- Last commit: TODO(lead): last commit
- Commit count (`git log --oneline 7296caba7..HEAD | wc -l`): TODO(lead): commit count
- Tagged commit: `fa45de4be` ("chore(strands): set the crate versions to 0.1.0-preview.1"), the 609th commit after the base, carries the annotated local tag `strands-rs-v0.1.0-preview.1`.

The Phase 12 test, live and review-fix tracks each ran in their own worktree on a local branch (`wip/<track>`), were integrated into the branch, and were not pushed on their own.
