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

Six workflow files for the Rust workspace are staged in `.rust-port/ci-workflows/` (`ci.yml`, `strands-rs-pr-and-push.yml`, `strands-rs-test-lint.yml`, `harness-rs-pr-and-push.yml`, `harness-rs-test-lint.yml`, `rust-security-audit.yml`). They are not under `.github/workflows/` because the token that persists the branch cannot write there (P-13). They were checked with PyYAML and actionlint, and none of them has run.

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

`.rust-port/DECISIONS-pending.md` holds 485 entries at `ba501ced3`, numbered P-1 to P-1111 with gaps between the ranges that parallel tracks reserved. Each entry gives the context, the options, the choice, the TS reference and the files. Later entries amend or supersede earlier ones and say so in their titles.

### Entries marked as owner decisions

These entries end with a question the run left open.

- P-1023 (with P-815): module shape. The file modules `hooks::{events, registry, types}`, `interventions::{actions, handler}`, the four `retry` file modules, `telemetry::{meter, tracer}`, `memory::extraction`, `agent::invocation_context` and `tools::{executors, structured_output}` are public but `#[doc(hidden)]`, and every item in them is also in the parent's flat re-export. The reviewer's judgement is that the hidden file-stem paths are acceptable while the crate is unpublished and become a semver commitment to the file layout in a published 0.1.0. The open call is whether they become `pub(crate)` before publishing, and whether the families that keep two documented paths by decision (`tools::{tool, registry, function_tool}`, `models::{model, streaming}`, the provider modules) should drop one.
- P-26 and P-646 (with P-5 and P-416): Cedar as a non-optional dependency. `cedar-policy` is compiled into every `strands` build, where TS makes the Cedar packages optional peers, and `strands` carries the exact requirement `cedar-policy = "=4.14.0"` that the schema generator crate sets, so a downstream crate cannot use another 4.x patch until the generator republishes. A default-on `cedar` feature, or a feature around schema generation, would change the five-feature list of PROMPT.md section 8.1, so neither was applied. Moving Cedar behind a feature after 0.1 would break `default-features = false` users.
- P-832: the harness `supports_media` judges a model instance by its model id, because `Model` has no downcast. An `OpenAIModel` on Bedrock Mantle passed as an instance with an `openai.*` id gets `false` where TS answers `true`, so `read` describes media in text for it unless `builtinTools.read.media` overrides. An SDK accessor would remove the difference.
- P-873 and P-991: programmatic tool callers are recognised by name, because the `Tool` trait has no identity or downcast. A live caller anywhere in the process hides a same-named ordinary tool from every caller. Matching by identity needs a marker or downcast on the SDK `Tool`.
- P-913: `EnvironmentContext` probes the platform and working directory through the sandbox, as TS at the pin does and as its tests assert. PROMPT.md section 11 item 40 says to use the native platform (`std::env::consts::OS`). The port follows TS; only the date is native.
- P-938: the portable config bridge applies a fixed list of `agentConfig` keys; any other key is dropped with one warning. The open call is whether an unsupported key should be an error.
- P-958: built-in plugins are recognised by `name()`, because `dyn Plugin` has no downcast. A consumer `Todos` built with a custom name is not recognised, and a second default one is added. An `as_any` on `Plugin` would restore the TS rule; that is an SDK change.

P-993 (three subagent constants public at the harness crate root) was closed by P-1110, which made them crate-internal as in harness-ts.

### Entries that change behaviour or public API the most

- P-49 and P-412: hook callbacks return `()` and cannot fail. TS callbacks may throw. Interrupts are recorded on the event (`BeforeToolCallEvent::interrupt`, `BeforeToolsEvent::interrupt`) and the registry runs every callback. An intervention handler with `on_error: throw` fails the event through a crate-internal channel, so the fail-closed default holds. Every user-written hook has a different shape from TS because of this.
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
- P-870, P-871, P-891, P-990 and P-992: public SDK additions made for the harness: `ToolAgent::call_tool`, `ToolCallOutcome` (with `HookCancelled`), `InvocationState::detached()`, `CancelSignal::abort_only` and a tool-use id on inner programmatic calls.
- P-930 to P-933: the shape of `HarnessAgentOptions`: one enum per polymorphic TS option and `Setting<C>` in place of TS `ToolConfig<K>`.
- P-923: `shutdown_telemetry` in the harness has no TS counterpart; without it the batch exporter's pending spans are lost at process exit.

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

TODO(lead): Phase 12 review dispositions (add the decision entries the Phase 12 review gate produces to the harness rows above)

## Divergences

Both divergence files list every user-visible behaviour difference from TypeScript with its reason, one table row each, and both open with the rule that a divergence never changes a cross-SDK wire or persisted format.

### SDK

`strands-rs/docs/DIVERGENCES.md` has 196 rows at `ba501ced3`. Grouped here by subject:

| Subject | Rows |
|---|---|
| Hooks, tools, the agent loop, streams, lifecycle, middleware, plugins, background tasks | 42 |
| Providers (Bedrock 10, Anthropic 8, OpenAI and Bedrock Mantle 6) | 24 |
| Memory | 19 |
| Wire and persisted data, types | 17 |
| MCP | 17 |
| Telemetry and logging | 17 |
| Sandbox and vended tools | 16 |
| Interventions, human-in-the-loop, Cedar | 11 |
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

### Harness

`harness-rs/docs/DIVERGENCES.md` has 48 rows at `ba501ced3`: model strings and providers that Rust does not build (7), the file tools (2), `web_fetch` and `web_search` (7), `programmatic_tool_caller` (12, including the Monty runtime in place of the TS one), `subagent` (2), `EnvironmentContext` (1), interventions (2), telemetry without the `otel` feature (1), the portable config and `HarnessAgentOptions` (7), and notes on `create_harness`, the built-in tool resolution, the shell probe cache and JS `trim` (7).

TODO(lead): Phase 12 review dispositions (recount the harness rows after the Phase 12 review fixes)

## TS bugs

`.rust-port/ts-bugs.md` records 39 defects found in the TypeScript sources while porting, each with `file:line`, what is wrong, a minimal repro, and how Rust handles it. No TypeScript file was changed. By area:

- Types and wire (3): `Uint8Array` fields are written two ways and the index-object form corrupts on reload; an `UnknownBlock` can be built around a known block; a block after unparseable tool input turns into a stale tool use.
- Agent loop, hooks and tools (8): concurrent tool interrupts fire `InterruptEvent` for one tool only; a hook-cancelled invocation skips `MessageAddedEvent` and `AgentResultEvent` hooks; concurrent tools end the wrong local trace; duplicate retry strategy types warn and then fail; delegation re-dispatches the child's events to the parent's hooks; `AgentToolStreamer` classifies a nested delegate's result; `agent.cancel()` during `initialize()` is accepted and then discarded; a snapshot's interrupt `context` aliases the live state.
- Providers (5): a throttled Bedrock request is not retried unless the throttle arrives inside the stream; unknown `additionalArgs` keys are dropped silently; Chat Completions parallel tool-call deltas interleave across content blocks; Chat Completions drops tool calls whose arguments arrive before the id; a Responses stream that ends without a terminal event reads as a complete answer.
- Sandbox and vended tools (3): `buildShellEnvPrefix` and `shellQuote` are documented for implementers but not exported; an empty context `Choice` passes `makeSubagent` validation and yields an empty enum; `mcp_router` `call_tool` errors reach the model with a doubled `Error: ` prefix.
- Conversation and context management (3): tool-result truncation drops the message metadata; `compressionThreshold: NaN` passes validation; `pin_context` advertises `action` as required although it has a default.
- Storage (2): a generic namespace view with a trailing-slash prefix lists truncated keys; sandboxed `LocalFileStorage` writes skip the search strategy's `index`.
- Skills (2): the skill location is always `<path>/SKILL.md`, also for a skill loaded from `skill.md`; a bare `SKILL.md` path source never loads in strict mode.
- Memory (3): `FileMemoryStore.add` drops a second heading-less entry written in the same millisecond; `ModelExtractor` forwards non-object `metadata` from the model; `search_memory` accepts any number for `maxSearchResults`.
- Interventions (4): `CedarAuthorization` leaks a call count when the context enricher throws; the `HumanInTheLoop` approval prompt loses the classifier reason on resume; a hook that throws at a tool gate still runs the next model call; intervention gates run on the pass an earlier hook pauses, and again on resume.
- Harness (6): the `edit` tool expands `$` patterns in `new_str`; the `edit` tool accepts an empty `old_str` on some files; the `read` tool reads a negative `limit` as an end offset; `web_fetch` can cut a surrogate pair; the `subagent` tool does not forward the invocation context; the `web_fetch` URL cache is keyed by URL alone and never shrinks.

Seven entries say that Rust keeps the TS behaviour so the two SDKs act alike: the sandboxed `LocalFileStorage` writes, the `CedarAuthorization` call count, the `HumanInTheLoop` prompt on resume, the intervention gates on a paused pass, the empty `old_str` and the negative `limit` of the harness file tools, and the harness `subagent` invocation context. The other entries say how Rust handles the case differently; the user-visible ones are also rows in `DIVERGENCES.md`.

TODO(lead): Phase 12 review dispositions (add the TS bugs the Phase 12 review gate records, if any)

## Known gaps

### Out of v0.1 by design

PROMPT.md section 5.2 lists what was not built: Graph, Swarm and A2A with their vended tools; bidirectional streaming; the `ContextOffloader` plugin, GoalLoop, steering and checkpointing; decision models and the decision-model classifier; Docker, SSH and OS-level sandboxes; S3 storage, vector search, embeddings and the Bedrock Knowledge Base memory store; MCP tasks; the Google, Vercel, Ollama and LiteLLM providers; module references in portable harness config (they produce an "unsupported" error); everything TS marks `@deprecated`; `configureLogging`; a Rust CLI; the docs-site Rust tabs; and parity automation. `FOLLOW-UPS.md` carries one line for each.

### Tests and live coverage

- 322 TS test cases of in-scope SDK specs are recorded as gaps with reasons (see "SDK parity" above). The harness figure is under the harness parity marker.
- No Anthropic or OpenAI request was sent: the 13 live tests of those two providers are written and skip without their keys. The Bedrock API-key path, guardrails and audio have no live run either, and the other uncovered cases are listed under "Live results".
- One live test depends on the model's wording and failed in 2 of 35 runs (see "Live results").
- The six CI workflows were checked with PyYAML and actionlint (P-13) and have never run, because they cannot be pushed under `.github/workflows/` from this environment.
- The tag `strands-rs-v0.1.0-preview.1` is local only; tags were not pushed. In a fresh clone it has to be recreated on `fa45de4be`.
- Rust code is not scored by the repository's `pr-metrics` complexity script; no adapter was written because a new CI binary could not be validated here (P-12).

### Review findings that were not fixed as proposed

Every phase closed with three reviewers (correctness, API, parity) and a disposition for each finding in `strands-rs/docs/reviews/phase-N.md`. The dispositions that are not a plain fix, with their reasons:

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
- Phase 10, correctness-5: fixed in the documentation (a provider `shutdown()` blocks and belongs in `spawn_blocking`). Left open there: the harness exit path has to do the same. At `ba501ced3` the harness has `shutdown_telemetry` (P-923) and nothing calls it.
- Phase 11, api-b4 and api-c8: the file modules of six families are `#[doc(hidden)]` and not private; pending for the owner (P-815, P-1023).
- Phase 11, root exports: `SnapshotManifest` and `SnapshotLocation` stay crate-private, out of v0.1 with `SnapshotStorage` (P-766).

The first review of the harness leaf modules (`phase12a`, 16 findings) has every finding fixed at `ba501ced3` (`.rust-port/handoff/phase12a-dispositions.md`).

TODO(lead): Phase 12 review dispositions (the findings of the three-reviewer gate over the whole harness that were not fixed, with reasons, from `harness-rs/docs/reviews/phase-12.md`)

### Where a binding decision and the evidence disagreed

- Skipped because it conflicted with a binding decision: a default-on `cedar` feature, and a feature around Cedar schema generation (P-26, P-646; section 8.1 fixes the dependency as non-optional and the feature list at five).
- Applied against the prompt text, each logged for review: the Cedar pin moved to `cedar-policy = "=4.14.0"` because the schema generator crate requires it (P-416); the token estimate counts UTF-16 code units where section 7.3 says `char` (P-43); `EnvironmentContext` probes the platform through the sandbox where section 11 item 40 says the native platform (P-913).
- Followed the design where TS differs: `sandbox: false` fails with `DefaultNotConfiguredError` (P-264), and there is no process-global default sandbox (P-263).

## Branch and commits

- Branch: `feat/rust-sdk`, pushed to the fork remote as `feat/rust-sdk-run-20261115`. Nothing was pushed to `strands-agents/harness-sdk`, and no PR or issue was opened.
- Base (the pin): `7296caba7` on `main`.
- First commit: `3a67c206f` ("chore(rust-port): create the run folder").
- Last commit: TODO(lead): last commit
- Commit count (`git log --oneline 7296caba7..HEAD | wc -l`): TODO(lead): commit count
- Tagged commit: `fa45de4be` ("chore(strands): set the crate versions to 0.1.0-preview.1"), the 609th commit after the base, carries the annotated local tag `strands-rs-v0.1.0-preview.1`.

When this draft was written, the worktree branch `wip/p13` was at `ba501ced3`, 739 commits after the base, with the harness test, live and review tracks still on their own branches.
