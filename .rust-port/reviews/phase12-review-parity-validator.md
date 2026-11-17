# Phase 12 review gate: parity validator

Commit under review: `24242240e` (worktree `/mnt/workspace/review-h12p`). TS spec pin `7296caba7` at `/tmp/ts-spec`.

## Gate results

`GATE_OUT=/tmp/review12_parity/gates .rust-port/gates.sh full` from `/mnt/workspace/review-h12p`, `CARGO_TARGET_DIR=/tmp/ct-review12p` (log `/tmp/review12_parity_gates.log`). Every step exited 0; the worktree is unchanged afterwards (`git status --short` shows only the two expected untracked symlinks).

- fmt: 0
- clippy: 0
- clippy_nodef: 0
- test_all: 0 (all-features: 5160 passed, 0 failed, 83 ignored; `strands-harness` lib 551 passed; `live_e2e` 12 ignored, `cross_sdk` 4 ignored)
- test_nodef: 0 (3750 passed, 0 failed)
- doc: 0
- msrv: 0
- feat_bedrock: 0
- feat_anthropic: 0
- feat_openai: 0
- feat_mcp: 0
- feat_otel: 0
- deny: 0
- conformance: 0 (ran with the symlinked `node_modules` and `strands-ts/dist`)

The six cases of `tests/programmatic_tool_caller_mcp.rs` skipped inside `test_all` (see parity-12); run separately with the two variables set they pass.

## Findings

### parity-1. Three TS `logger.warn` sites are `warn_once` in Rust, and the difference is not in DIVERGENCES.md

`harness-rs/strands-harness/src/builtin_tools.rs:162` (the Exa privacy notice), `:287` (an unnamed `McpClient` in `tools`) and `:291` (two `McpClient`s sharing a name) call `logging::warn_once`, which logs a given text once per process (`harness-rs/strands-harness/src/logging.rs:8-16`).

Evidence: TS logs these on every call, with the plain logger: `harness-ts/src/builtin-tools.ts:121` (`logger.warn(` for the Exa notice in `webSearchMode`), `:205` and `:210` (`logger.warn(` in `mcpClientsByClientName`). `warnOnce` in TS is used only in `models.ts`, `config.ts` and `telemetry.ts` (`grep -n 'warnOnce(' harness-ts/src -r`). So a Rust process that builds a second harness with `web_search: 'exa'` (or a second agent with an unnamed MCP client) logs nothing, where TS repeats the notice per `createHarness` call. Observed in one process building two harnesses with `web_search: "exa"` (the run of 154 configs described under "Checked and found equal"): TS logged the Exa notice for both builds, Rust for the first only. `harness-rs/docs/DIVERGENCES.md` has no row for it (rows 48 and 49 cover the `application_name` spelling and the `mcp` feature only); the decision text at `.rust-port/DECISIONS-pending.md:989` mentions the dedupe only as a test constraint.

Fix: log the three with `tracing::warn!` directly (as `tools/programmatic_tool_caller.rs:346` does for the TS `logger.warn` at `programmatic-tool-caller.ts:414`), and assert them in the tests with the `WarnRecorder` instead of `logging::warned`; or keep `warn_once` and add a DIVERGENCES.md row with the reason.

### parity-2. Harness telemetry cannot be flushed at exit: `shutdown_telemetry` is crate-private and never called, where TS flushes on `beforeExit`

`harness-rs/strands-harness/src/telemetry.rs:126-143`: `pub(crate) async fn shutdown_telemetry()` carries `#[cfg_attr(not(test), expect(dead_code, reason = "called at exit by the harness's callers, which land separately"))]`. The whole crate has landed and nothing calls it; it is not re-exported from `lib.rs`. `setup_telemetry` keeps the `SdkTracerProvider` in the private `TELEMETRY` slot (`telemetry.rs:78`, `:120`), so a caller of `create_harness` has no handle to it.

Evidence: with `OTEL_TRACES_EXPORTER=otlp` the SDK wires a `BatchSpanProcessor` (`strands-rs/strands/src/telemetry/config/setup.rs:322`), which exports on a timer. TS flushes it when the process exits: `strands-ts/src/telemetry/config.ts:213-218` (`process.once('beforeExit', () => _provider?.forceFlush() ...)`). The SDK's own divergence row (`strands-rs/docs/DIVERGENCES.md:175`, "Flush on exit ... The caller calls `shutdown()` on the returned provider") does not apply to the harness, because the harness never returns the provider. A short harness run in Rust therefore drops the spans still in the batch; the same run in TS exports them. `harness-rs/docs/DIVERGENCES.md` has no row for it, and no test title covers it (`telemetry.test.ts` has no exit case).

Fix: make the flush reachable: export `shutdown_telemetry` from the crate root (`pub async fn`, with the `otel` feature; a no-op without it) and name it in the crate docs and README as the call to make before exit; drop the `expect(dead_code)`. Add the DIVERGENCES.md row (TS: automatic on `beforeExit`; Rust: the caller awaits `shutdown_telemetry()`).

### parity-3. `rejects an effort outside the enum by name` is matched to a test of dead code; the DIVERGENCES.md row for it names a function users cannot reach

`harness-rs/strands-harness/src/models/tests.rs:77-86` (`rejects_an_effort_outside_the_enum_by_name`) calls `parse_effort`. `harness-rs/strands-harness/src/models.rs:128-135` declares it `pub(crate)` with `#[cfg_attr(not(test), expect(dead_code, reason = "used by the config normalization, which lands separately"))]`: no non-test code calls it (`grep -rn parse_effort harness-rs/strands-harness/src` gives only the definition and that test; the gate's clippy step passes, so the `dead_code` expectation is fulfilled). The config normalization did land and has its own table and its own text (`config/normalize.rs:61`, `:327-336`: `effort must be one of: auto, off, ...`). `HarnessError::InvalidEffort` (`error.rs:42-43`), a public variant, is constructed nowhere else.

Evidence: the TS case (`harness-ts/test/models.test.ts:113-117`) asserts that `resolveModel` rejects `'bogus'` with `Effort "bogus" is not valid. Supported: ...`, which a TS caller reaches through `createHarness({ effort })`. In Rust no public path returns that text. `check-test-titles.mjs` reports the case as matched (models.test.ts 105/0/6/0), and `harness-rs/docs/DIVERGENCES.md:13` says "the same text comes from parsing a name (`parse_effort`)", which is true only inside the crate's own test.

Fix: delete `parse_effort` and `HarnessError::InvalidEffort` (TS config validation uses the `must be one of` text, which `normalize.rs` already has), declare the TS case a gap in `test-title-map.json` ("TS-only: `Effort` is an enum, so a value outside it does not compile (P-833)"), change the PARITY.md row to 104/111, and reword DIVERGENCES.md:13 to "Not reachable". If the text should stay reachable instead, give it a public entry point (`impl FromStr for Effort` returning `HarnessError::InvalidEffort`) and test that.

### parity-4. Stale `dead_code` expectations whose reason says a module "lands separately"; four default constants are never read

- `harness-rs/strands-harness/src/defaults.rs:2-5`: file-wide `#![expect(dead_code, reason = "each constant is used once the module that reads it lands; drop this when all are")]`. All modules have landed and the expectation still holds because `DEFAULT_BUILTIN_PLUGINS`, `DEFAULT_SESSION`, `DEFAULT_SKILLS` and `DEFAULT_MEMORY` are read nowhere (`grep -rnw` over `src/` finds only the definitions). TS reads all four in `createHarness` (`harness-ts/src/agent.ts:359-361`, `:468`). Rust takes the same values from other places (`SessionOption`/`SkillsOption`/`MemoryOption` `#[default]` variants, `BUILTIN_PLUGIN_NAMES.to_vec()` at `agent.rs:531`), so behaviour is the same today, but the file-wide expectation would also hide a default that stops being applied.
- `harness-rs/strands-harness/src/interventions.rs:32`: `#[expect(dead_code, reason = "named by create_harness, which lands separately")] pub(crate) type InterventionAsk = Ask;` is still unused after `create_harness` landed.
- `harness-rs/strands-harness/src/models.rs:128-134` and `telemetry.rs:127-133`: see parity-3 and parity-2.
- `harness-rs/strands-harness/src/test_support.rs:2-5`: file-wide `expect(dead_code, reason = "used once the modules that assert on warnings land; drop this when they have")`.

Fix: delete the four unused constants (or read them: `DEFAULT_BUILTIN_PLUGINS` at `agent.rs:531`), delete `InterventionAsk`, and replace the file-wide expectations in `defaults.rs` and `test_support.rs` with none (or with item-level ones that name a real reason).

### parity-5. The four Background Tasks cases of `agent.test.ts` test only `resolve_background_tasks`; nothing tests that `create_harness` gives the agent that policy

`harness-rs/strands-harness/src/agent/tests.rs:120-160`: `backgrounds_the_subagent_and_leaves_other_compatible_tools_agent_selectable`, `treats_background_tasks_true_the_same_as_the_default`, `keeps_the_subagent_backgrounded_under_a_custom_policy` and `disables_background_tasks_when_requested` call the private function `resolve_background_tasks(configured, &["subagent"])` with a hand-written `forced` list.

Evidence: the TS cases build the harness and read the agent's policy (`harness-ts/test/agent.test.ts:197-235`: `const agent = await createHarness(...)`; `expect([...backgroundTaskPolicy(agent)!]).toEqual([['*','agentic'],['subagent','always']])`). In Rust the part of `create_harness` they exercise is `agent.rs:584-598`: collecting `forced` from the assembled tools by `ALWAYS_BACKGROUND_TOOL_NAMES` and passing `background_tasks: Some(resolve_background_tasks(background_tasks, &forced))` to `Agent::new`. No non-live test reads a built agent's policy or runs a backgrounded `subagent` through a built agent: `grep -rn background src/agent/tests.rs src/agent/tests/*.rs src/builtin_tools/tests.rs` gives only the four calls above, the child-options case (`tests.rs:1093-1102`) and tests that turn background tasks off. The cases would still pass if `create_harness` passed an empty `forced` list, dropped the option, or lost `subagent` from `forced` when it is disabled. PARITY.md says "the 4 Background Tasks cases are on `resolve_background_tasks`" but counts them as ported.

The behaviour itself is right today: a harness agent on a scripted model that calls `subagent` gets `Background task dispatched. ... Tool: subagent ... Status: queued ...` as the tool result in both SDKs by default and with `backgroundTasks: true`, the child's answer with `backgroundTasks: false`, and a foreground result for `read` in all three (drivers `/tmp/review12_parity/hts/bg.mts`, `/tmp/review12_parity/dump/src/bin/bg_diff.rs`). The finding is that no test in the tree would notice if it stopped being so.

Fix: assert on a built agent, for example exactly that: `MockMessageModel` with a `subagent` tool-use turn, and the first tool result starts with `Background task dispatched.` (and is the child's text with background tasks off). Or add an accessor the tests can read (the SDK already gained `Agent::interventions()` for the same purpose, P-1050), so the custom-policy case can compare `always`, `never` and `agentic` as the TS case does.

### parity-6. `enables web_search when explicitly selected on a supported provider` does not assert that search is enabled

`harness-rs/strands-harness/src/agent/tests.rs:967-977`: the Rust test builds `anthropic/claude-opus-4-8` with `builtin_tools: [read, web_search]` and asserts `has_tool("read")` and `!has_tool("web_search")`.

Evidence: the TS case (`harness-ts/test/agent.test.ts:451-455`) asserts the model got the native search tool (`expect(agent.model.getConfig().builtInTools).toEqual([{ googleSearch: {} }])`) and then that no `web_search` tool is registered. The Rust test would pass if an explicit `web_search` on a supported provider enabled nothing. The sibling cases in `agent/tests/provider_models.rs` already have the means: `resolved(options)` returns the `ResolveModelOptions` `create_harness` resolved the model with, and `anthropic_config(...)` the config built from them.

Fix: move the case to `provider_models.rs` and assert `options.web_search` and `anthropic_config(&agent, spec, options).anthropic_tools == Some(vec![json!({"type":"web_search_20260318","name":"web_search","allowed_callers":["direct"]})])`, keeping the `!has_tool("web_search")` check.

### parity-7. Portable `agentConfig`: keys that are plain JSON in TS and typed in Rust's `AgentConfig` are rejected or dropped, and the DIVERGENCES.md reason for it is not true for them

`harness-rs/strands-harness/src/config/options.rs:186-200` (`apply_agent_config`): `"backgroundTasks"` is parsed with `parse::<bool>`, and every key outside the eleven listed is dropped with `Ignoring agentConfig keys that are not supported in Rust: ...`.

Evidence: TS spreads `agentConfig` into the options (`harness-ts/src/config.ts:371`, `...config.agentConfig`), and these `AgentConfig` keys take plain JSON there (`strands-ts/src/agent/agent.ts:162` ff.) and have a typed Rust field:
- `backgroundTasks: { agentic, always, never, waitForCompletion }` (string selectors): Rust has `BackgroundTasksConfig` with exactly these fields (`strands-rs/strands/src/background_tasks/types.rs:36-48`), but the object form fails with `HarnessError::InvalidAgentConfigValue` because only `bool` is parsed. A config file that works in TS is an error in Rust.
- `sandbox: false`: Rust has `SandboxSource::Off` (`strands-rs/strands/src/sandbox.rs:52`); the key is dropped with a warning, so the Rust agent keeps a sandbox the TS agent does not have.
- `traceAttributes: { "k": "v" }`: Rust has `AgentConfig.trace_attributes`; dropped with a warning.
- `toolExecutor: "sequential" | "concurrent"` (`agent.ts:157`): Rust has `ToolExecutor::{Sequential, Concurrent}` (`strands-rs/strands/src/tools/executors.rs:24-29`); dropped with a warning.
- `retryStrategy: null` (turns retries off): Rust has `retry_strategy: Some(vec![])`; dropped with a warning.

Run on both SDKs with the same portable config (`{"agentConfig": {"backgroundTasks": {"always": ["shell"], "never": ["read"], "waitForCompletion": false}}}` through normalize, options-from-config and `createHarness`/`create_harness`; scratch drivers under `/tmp/review12_parity/`): TS builds the agent; Rust returns `agentConfig.backgroundTasks is invalid: invalid type: map, expected a boolean`.

`harness-rs/docs/DIVERGENCES.md:41` gives the reason "`AgentConfig` is typed; the other keys have no JSON form in Rust", which does not hold for these five (P-938 lists the applied keys but gives no reason for leaving these out). No test covers them (`config/tests.rs` has only `"backgroundTasks": false`).

Fix: in `apply_agent_config`, parse `backgroundTasks` as `bool | { agentic?, always?, never?, waitForCompletion? }`, map `sandbox: false` to `SandboxSource::Off`, `traceAttributes` to `trace_attributes`, `toolExecutor` to the enum and `retryStrategy: null` to `Some(vec![])`, with a test per key; or keep the subset and correct the DIVERGENCES.md row to name each JSON-expressible key that is dropped, and say that the object form of `backgroundTasks` is an error.

### parity-8. `does not double-add a skills plugin supplied via plugins` never builds a harness

`harness-rs/strands-harness/src/agent/tests.rs:829-833`: the test asserts `has_skills(&[existing])` and `!has_skills(&select_builtin_plugins(&BUILTIN_PLUGIN_NAMES, &[]))`. It calls neither `create_harness` nor `skills_plugin`.

Evidence: the TS case (`harness-ts/test/agent.test.ts:745-751`) builds the harness with both `skills: dir` and `plugins: [existing]` and asserts the agent's skills plugins are exactly `[existing]`. The behaviour is the guard at `agent.rs:525` (`if !has_skills(&consumer_plugins) && let Some(skills) = skills_plugin(skills, &seams)`), which the Rust test does not run: it passes with the guard removed. No other test passes an `AgentSkills` in `plugins` (`grep -rn AgentSkills src/agent/tests.rs src/agent/tests/*.rs` gives only line 830). The other eight skills cases are on `skills_plugin` alone, which PARITY.md says; that one function is at least the code under test there, and `config_e2e.rs:73-120` covers the wiring of a `skills` path into a built agent.

Fix: build the harness with `skills: SkillsOption::Sources(vec![dir])` and `plugins: vec![existing]`, initialize it, and assert the skills it serves are those of `existing` (the helper at `agent/tests.rs:1173-1185` reads them with `get_available_skills`), for example by giving `existing` a different directory from the `skills` option and asserting only its skill is listed.

### parity-9. DIVERGENCES.md has no row for logging: TS exports `configureLogging` and prints warnings by default; Rust prints nothing

`harness-rs/docs/DIVERGENCES.md` (56 lines): no row mentions `configureLogging`, the logger or `tracing` (`grep -n -i 'console\.\|subscriber\|configureLogging\|logger' harness-rs/docs/DIVERGENCES.md` is empty). `harness-rs/README.md` does not mention a subscriber either.

Evidence: the TS harness exports `configureLogging` from its root (`harness-ts/src/index.ts:42`), and its default logger writes `warn` and `error` to the console (`harness-ts/src/logging.ts:11-16`, `warn` and `error` at `:14-15`). The Rust crate has no such export (`harness-rs/strands-harness/src/lib.rs:29-58`) and logs through `tracing` (`logging.rs:15`), which prints nothing until the application installs a subscriber. So by default a Rust user never sees the lines TS prints: the Exa third-party privacy notice (`builtin_tools.rs:162`), the caching and effort notices on a model instance (`models.rs:339-349`), `Ignoring unknown agent config keys` (`config/normalize.rs:89`), the MCP client warnings, and `Ignoring agentConfig keys that are not supported in Rust` (`config/options.rs:205`). With `OTEL_TRACES_EXPORTER` set, the SDK's `setup_tracer` installs the process's one global subscriber, which exports spans only (`strands-rs/strands/src/telemetry/config/setup.rs:205-209`, `:249-253`), so an application subscriber installed after `create_harness` fails and the lines stay unseen. The SDK documents the same difference for itself (`strands-rs/docs/DIVERGENCES.md:194`); the harness file, which must list every user-visible difference, does not.

Fix: add a "Logging" row to `harness-rs/docs/DIVERGENCES.md` (TS: `configureLogging`, console by default; Rust: `tracing` events, silent without a subscriber, and the subscriber must be installed before `create_harness` when `OTEL_TRACES_EXPORTER` is set), and say in the README and crate docs how to see the warnings (install a `tracing` subscriber first).

### parity-10. DIVERGENCES.md rows 48-52 have two cells in a four-column table

`harness-rs/docs/DIVERGENCES.md:48-52`: the table header is `| Area | TS behaviour | Rust behaviour | Reason |`. The five rows for `mcpClientsByClientName`, `webSearchMode` and `createHarness` (P-953, P-951, P-958, P-957, P-961) have only an area cell and one text cell, so the Rust behaviour renders under "TS behaviour" and the "Rust behaviour" and "Reason" cells are missing. Example, line 51: ``| `agent.ts` `createHarness` | `caching: 'auto'` passed explicitly behaves as not passed (warns, never errors) (P-957). |``: the TS behaviour (an explicit `'auto'` is an explicit request and throws on a provider or model without caching, `harness-ts/src/agent.ts:374-378`) is not stated, nor the reason.

Fix: write the four cells for each of the five rows (TS behaviour, Rust behaviour, reason).

### parity-11. `loads executable values through declared module references` is counted as ported, but the Rust test of that name asserts that nothing loads

`harness-rs/strands-harness/src/config/tests.rs:206-241` (`loads_executable_values_through_declared_module_references`): for a reference in each of nine positions the test asserts the error `module references are not supported in Rust yet (language "typescript")`.

Evidence: the TS case (`harness-ts/test/config.test.ts:84-111`) asserts the opposite: each referenced module is loaded and its export lands in the options. `check-test-titles.mjs` reports config.test.ts as 29 matched, 3 gaps, and PARITY.md as 29/32, because a Rust test with the snake_case title exists; PARITY.md calls it "Adapted". The three sibling module-loading cases of the same spec are declared gaps with reason #31; this one is the same gap under a name that says the reverse of what the test checks. (`fails_loudly_when_the_runtime_cannot_load_a_referenced_language`, also "adapted", does keep the TS meaning: a reference in a language the runtime cannot load is an error.)

Fix: rename the Rust test to what it asserts (for example `rejects_a_module_reference_in_every_position`), declare the TS case a gap in `test-title-map.json` with the #31 reason, and change the row to 28/32.

### parity-12. The gate passes the six `programmatic_tool_caller over MCP` cases without running them

`harness-rs/strands-harness/tests/programmatic_tool_caller_mcp.rs:36-43`: each case returns early (and passes) when `STRANDS_TEST_NODE` and `STRANDS_TEST_NODE_MODULES` are unset. `.rust-port/gates.sh` sets neither.

Evidence: in my gate run (`/tmp/review12_parity/gates/gate_test_all.txt`) the binary reports `running 6 tests ... test result: ok. 6 passed; ... finished in 0.00s`. Run with the variables set (`STRANDS_TEST_NODE=/mnt/workspace/.node/bin/node STRANDS_TEST_NODE_MODULES=/mnt/workspace/harness-sdk/node_modules cargo test --locked --workspace --all-features --test programmatic_tool_caller_mcp`, a local stdio server, no network) the same six pass in 1.84s, so the code is fine today; but the gate result says nothing about them, and PARITY.md counts the six as ported (42/43). The gate's `full` mode already requires `node_modules` (the conformance step), so the inputs are there.

Fix: in `gates.sh full`, export `STRANDS_TEST_NODE` (from `command -v node`) and `STRANDS_TEST_NODE_MODULES="$PWD/node_modules"` before `test_all` when both exist, and make the cases fail rather than skip when `STRANDS_TEST_REQUIRE_NODE=1` is set by the gate; or serve the fixture's four tools from an in-process `rmcp` server over an in-memory pipe, as `src/agent/tests/mcp_delegates.rs` does, so the cases need no Node.

### parity-13. A tool call with an undeclared argument runs in TS and is rejected in Rust (`read`, `write`, `edit`, `shell`, `todo_write`, `web_fetch`)

`harness-rs/strands-harness/src/tools/file_tools.rs` (`read`, `write`, `edit`), `tools/web_fetch.rs:478-491`, `tools/web_search.rs` (by reading; not called), `plugins/todos.rs:84-110` and the SDK's `vended_tools/shell.rs:121-136` publish `"additionalProperties": false` (to match what `z.toJSONSchema` prints) and validate the model's input against that schema with `jsonschema` (`strands-rs/strands/src/tools/function_tool.rs:163-200`, `InputValidator`), so an input with a key the schema does not declare is a validation error and the tool does not run.

Evidence: TS declares these inputs with `z.object({...})` (`harness-ts/src/tools/file-tools.ts:94`, `:186`, `:202`; `plugins/todos.ts:25`, `:85`; `tools/web-fetch.ts:220`), which strips unknown keys when it parses; the `additionalProperties: false` in the published schema is only what the JSON Schema export prints. Run on both SDKs with a scripted model issuing the same tool calls through a harness agent (drivers `/tmp/review12_parity/hts/ft2.mts` and `/tmp/review12_parity/dump/src/bin/ft_diff2.rs`):

| call | TS result | Rust result |
|---|---|---|
| `write {path, content, mode: "w"}` | `Wrote 2 lines to <path>.` | `Error: Invalid input: Additional properties are not allowed ('mode' was unexpected)` |
| `read {path, encoding: "utf-8"}` | the numbered lines | `Error: Invalid input: Additional properties are not allowed ('encoding' was unexpected)` |
| `edit {path, old_str, new_str, replace_all: false}` | `Edited <path>.` | `... ('replace_all' was unexpected)` |
| `shell {command: "echo hi", description: "say hi"}` | `{"output":"hi\n","error":"","exit_code":0}` | `... ('description' was unexpected)` |
| `todo_write {todos: [...], merge: true}` | `1 todos remaining ...` | `... ('merge' was unexpected)` |
| `todo_write {todos: [{..., id: "1"}]}` | `1 todos remaining ...` | `Error: Invalid input at /todos/0: Additional properties are not allowed ('id' was unexpected)` |
| `web_fetch {url: "not a url", format: "markdown"}` | the tool's own `Failed to fetch not a url: ...` text | `... ('format' was unexpected)` |

A larger run of 50 `read`/`write`/`edit`/`shell` calls agrees on 43; the other 7 are one more undeclared key and six invalid-type, missing-key or out-of-range inputs, where both SDKs return an error and only its wording differs (zod's issue list against `Invalid input at ...`, which `strands-rs/docs/DIVERGENCES.md:20` lists for the SDK). The difference in outcome is listed nowhere: `harness-rs/docs/DIVERGENCES.md` has no row for tool input validation, and the SDK file mentions zod's strip of undeclared keys only for structured output (`strands-rs/docs/DIVERGENCES.md:37`, implemented by `strip_unknown_properties`, `strands-rs/strands/src/tools/structured_output.rs:59`). Models do add stray arguments; in TS the call goes through, in Rust it costs a failed call and a retry. No ported test covers it.

Fix: drop undeclared keys before validating, as zod does: apply the existing `strip_unknown_properties` (or an equivalent walk over the schema's `properties`, nested objects and array items included) in `InputValidator::parse`, or in the harness tools before `validator.parse`, keeping the published schema unchanged; add a test per tool that an extra key is ignored. If the stricter behaviour is wanted, add a DIVERGENCES.md row saying so.

### parity-14. `web_fetch` with the `direct` transport fetches a URL that carries credentials; TS refuses it

`harness-rs/strands-harness/src/tools/web_fetch.rs` (`fetch_direct`, reached from `WebFetch::fetch`): a URL with userinfo (`http://user:pw@host/path`) passes `validate_url` and is requested with `reqwest`, which sends the userinfo as `Authorization: Basic` (a one-request local server logged `Basic dXNlcjpwdw==` for `http://user:pw@127.0.0.1:18767/x`).

Evidence: the same call through a harness agent with `builtinTools: { web_fetch: { transport: "direct" } }` against a local test server (`/tmp/review12_parity/srv.py` on 127.0.0.1; drivers `hts/wf_direct.mts` and `dump/src/bin/wf_diff.rs`):

- TS: `Failed to fetch http://user:pw@127.0.0.1:18765/plain: Request cannot be constructed from a URL that includes credentials: http://user:pw@127.0.0.1:18765/plain` (Node's `fetch` rejects such a URL, `harness-ts/src/tools/web-fetch.ts`, the `direct` branch).
- Rust: the page text (`hello\nworld\n`).

With the default `curl` transport both SDKs fetch the page, so this is the `direct` transport only. `harness-rs/docs/DIVERGENCES.md:21` covers the wording of `direct` failures, not this difference in outcome. Of the other 26 `direct` calls in the run, 25 agree and one differs only in the documented connection-failure wording.

Fix: in the `direct` path, refuse a URL whose username or password is non-empty with the TS text (`Request cannot be constructed from a URL that includes credentials: <url>`), with a test; or add the DIVERGENCES.md row.

## Checked and found equal (no finding)

These are the checks behind items 2 to 6 of the brief that turned up nothing, with how each was run, so they need not be repeated.

- Titles and rows: `check-test-titles.mjs --parity harness-rs/docs/PARITY.md` exits 0 with no missing case (root 286 matched, 7 mapped, 24 gaps; plugins 19/0/1; tools 145/0/3). Every `*.test.ts` under `/tmp/ts-spec/harness-ts/test/` has a row (20 in scope, 3 in the live checklist), and each row's total equals `count-ts-cases.mjs` for that spec. `harness-ts/` in the worktree is identical to the pinned export (`diff -rq`).
- Tool specs, byte for byte: a scratch binary (`/tmp/review12_parity/dump`, depends on the worktree by path) built a harness with `web_search: Exa` and printed `serde_json::to_string(tool.tool_spec())` for each tool; the TS side built the same eight tools from `harness-ts/src` with `tsx`. `shell`, `read`, `write`, `edit`, `web_fetch`, `web_search`, `programmatic_tool_caller` and `subagent` are equal as compact JSON strings, key order included, and come in the same order. `todo_write` was compared by reading (`plugins/todos.rs:84-110` against the TS spec dump).
- Every string literal and template piece of four or more characters in `harness-ts/src` (TypeScript AST walk) is a substring of a Rust string literal of the crate, except internal identifiers that are not text a user or model sees (`'native'`, `'extended'`, `'userTurn'`, `'abort'`, block type names, import specifiers and the like) and the ones a DIVERGENCES.md row or a declared gap accounts for (module loading, the Google/Ollama/LiteLLM tables, the not-reachable `Tool context is required` and `Unknown built-in ...` errors, the unnamed-MCP-client spelling). `HARNESS_CONTRACT` is identical.
- Model tables: for 75 model strings and the 8 effort values (600 pairs), TS `resolveModel(spec, spec, effort, true, true, false)` and Rust `create_harness` agree on success or failure, on the error text (178 errors), and on the built model's id and `max_tokens`; `supportsThinking`/`supports_thinking` and `supportsWebSearch`/`supports_web_search` agree on all 75. The request fields (`additionalRequestFields`, `params`, `anthropicTools`, `cacheConfig`) were compared by reading `models/build.rs` against `models.ts:128-310` only.
- End to end on identical portable configs: 154 JSON configs (valid, invalid, and edge cases of `builtinTools`, `session.id`, `interventions`, `builtinPlugins`, `mcpServers`, `agentConfig`, `skills`, `effort`, model strings) were run through normalize, options-from-config and `createHarness`/`create_harness` in both SDKs. Compared: success or failure, the full error text, the tool names and their order before and after `initialize()`, the system prompt, the agent name, id and description, and the session id. 146 agree. The 8 that differ are the documented provider list in `Unknown model provider` (3), `ollama/` (2), wrong-typed `agentConfig` values (2, DIVERGENCES.md row 41) and the `backgroundTasks` object (parity-7). Warnings were not captured on the Rust side.
- Tool results: with a scripted model issuing 50 `read`, `write`, `edit` and `shell` calls through a harness agent in both SDKs (offsets and limits including 0, negative and fractional values, missing and relative paths, a directory, CRLF and multi-byte text, a 2,500-line file, a fake `.png` and `.PDF`, a `.csv`, edits with zero, one and several matches and an empty `old_str`, shell exit codes and a timeout), the status and text of the result are identical in the 43 with valid input; see parity-13 for the rest. `web_fetch` agrees on the three URL-validation refusals tried (no fetch was made).
- `web_fetch` against a local HTTP server (plain text, HTML with scripts, styles and entities, a 120,000-character page, a 6 MiB page, Latin-1 and missing charsets, 404, 500, a redirect, JSON, binary and PNG bodies, an empty body, no content type, a repeated URL, prompts that call the summarizer, empty and blank prompts, a refused connection, a URL with a space, an upper-case scheme, `file://`, a fragment): with the default `curl` transport all 27 tool results and all 30 model requests (the summarizer's included) are the same in TS and Rust. With `direct`, 25 of 27 results agree; the two that differ are the documented connection-failure wording and parity-14.
- Pure defaults: `createHarness({ model })`/`create_harness` with only a scripted model given, invoked once with a `write` call in an empty directory: both mint an eight-hex session id, leave the same tree under `./.agent/sessions/<id>/` (and no `./.agent/memory`), and send the same two model requests (system prompt, the eleven tool names, message text).
- More of the same two kinds of run: the model requests in two further directories (a git repository with 30 package folders plus `dist`, `vendor`, `build`, `target` and hidden folders; an empty directory) agree, 20 of 20; and 42 further tool calls agree in 39: `read` on real and mislabelled PNG, JPEG, GIF, WebP, PDF, DOCX, XLSX, DOC and XLS files, an empty `.png`, binary, Latin-1 and NUL-containing text, `todo_write` with an empty list, two in-progress items, all completed, empty strings and multi-line content, `write` line counts, and `shell` with 200,000 to 300,000 characters of output. The three that differ are two invalid `todo_write` inputs (error wording) and `edit` with `$&` in `new_str` (the listed TS bug).
- Defaults: `defaults.rs` against `defaults.ts`; `resolve_background_tasks`, `sanitize_session_id`, the session and stash layout (`save_latest_on: Message`, the stash under `<dir>/<id>`), `check_name_collisions`, `web_search_mode`, `child_options`, `resolve_web_fetch_model`, `resolve_memory`, and `options_from_config` (paths, `~`, MCP `cwd`, `${VAR}` expansion, interventions) were read side by side with `agent.ts`, `builtin-tools.ts`, `models.ts`, `memory.ts` and `config.ts`; no difference beyond the listed divergences.
- Constants: web_fetch (`strands-harness/1.0`, 30 s, 5 MiB, 50,000 characters, 15 minutes), web_search (30 s, 10 results, 500 characters), programmatic tool caller (60 s, 256 MiB, 1,000 suspensions, 10 concurrent calls, 200,000 output characters, 900,000 ms default timeout), `AGENTS_MD_CAP` 16,000, discovery depth 2, subagent max depth 2.
- Declared gaps: all 28 reasons in `test-title-map.json` were read against the Rust types; each holds (typed options that cannot carry the value, module loading #31, providers #60, P-898, P-919, P-1072, P-1073).
- `harness-rs/docs/reviews/live.md` exists and names the 12 tests of `tests/live_e2e.rs` and the 4 of `tests/cross_sdk/main.rs`, which are all the `live_*` tests in the tree (all `#[ignore]`d; not run here).

- Tool specs across configurations: for 17 further configs (subsets and orders of `builtinTools`, per-tool configs for `shell`, `read`, `subagent`, `programmatic_tool_caller`, `web_fetch`, three model strings, each `builtinPlugins` choice, `contextManager` off and agentic, `interventions: "ask"`), the serialized spec of every tool on the built agent is byte-equal between TS and Rust, both before and after `initialize()` (so `todo_write`, `search_memory`, `retrieve_context` and `strands_manage_background_task` are covered as the harness registers them).
- Options from a portable config: for 63 configs aimed at `config/options.rs` (session, memory and skills paths with `..`, `~`, absolute and empty values; `.cedar` interventions; inline and file-backed `mcpServers` with `cwd`, nested `mcpServers`, and `${VAR}`/`${env:VAR}` forms), the options the two SDKs produce agree field by field, and the error texts agree, in 62. The one difference has no effect: `memory.dir: "~/mem/"` keeps its trailing slash in TS (`/home/mem/`) and loses it in Rust.
- Log lines: over the 154 configs, the INFO and WARN messages the harness emitted (a recording `tracing` subscriber in Rust, `configureLogging` in TS) are the same 14 texts, apart from the Ollama ones (TS only) and `Ignoring agentConfig keys that are not supported in Rust` (Rust only, see parity-7). How often each is logged was not compared (see parity-1).
- What the model is sent: with a scripted model on both sides (TS: a `Model` subclass; Rust: `strands::testing::MockMessageModel`), a harness built from the same portable config was invoked twice in a directory with `AGENTS.md`/`README.md` files at several depths and in skipped folders, and in one with a 33,606-character `AGENTS.md` (multi-byte and astral characters). For each `builtinPlugins` choice (both, `environment`, `todos`, none) the 20 model requests carry the same system prompt, tool names and message text (ids aside): the `<environment>` block, the `AGENTS.md` body and its truncation marker at the 16,000 cap, the nearby-file lists and their order, the todo reminder, and the `todo_write` result text.
- `subagent` through the harness: a scripted model made 17 `subagent` calls from a harness agent in both SDKs (presets, `tools` narrowing including an empty and an unknown selection, each `context` mode with and without `last_messages`, an unknown `agent_type`, missing and wrong-typed arguments, and the nested calls that follow from them). The 30 model requests that resulted (parent and children share the model) are the same in both: system prompt, tool names, and message text, including the fork and context preambles and the escaped `<parent_context>` tags; the tool results agree too (`Subagent error: Unknown agent_type "nope"; valid values: generalist.`).
- Sessions on disk and the `ask` preset: a harness with `session: { id: "Sess One", dir }` and a fixed agent id, invoked twice with a `write` call, leaves the same file tree in both SDKs (`sess-one/scopes/agent/<id>/snapshots/snapshot_latest.json`, the stash under `sess-one/context/sess-one/scopes/agent/<id>/`), and the snapshot has the same keys three levels deep. With `interventions: "ask"` both stop with `interrupt`, the same interrupt id, name and reason text, and leave the file unwritten; with `"off"` both write it.

The scratch drivers and their outputs are under `/tmp/review12_parity/` (TS: `hts/*.mts` over a copy of the pinned `harness-ts/src`, with stub packages for `import-meta-resolve` and `cjs-module-lexer`, which the shared `node_modules` lacks; Rust: `dump/src/bin/*.rs`, a crate that depends on the review worktree by path). The runs whose config had `skills: ["https://example.com/SKILL.md"]` requested that URL (it answered 404); nothing else left the machine, and no AWS call was made.

## Not covered

- `programmatic_tool_caller` behaviour against TS: `@pydantic/monty` is not in the shared `node_modules`, so no differential run; `tools/programmatic_tool_caller/*.rs` and its tests were not re-read beyond the limits, the constants and the six MCP cases.
- The matched tests of the modules the 12a review covered were not re-sampled case by case: `models/tests.rs`, `models/tests/providers.rs`, `tools/file_tools/tests.rs`, `tools/web_fetch/tests.rs`, `tools/web_search/tests.rs`, `tools/subagent/tests/*`, `tools/shell/tests.rs`, `plugins/environment/tests.rs`, `plugins/todos/tests.rs`, `interventions/tests.rs`, `telemetry/tests.rs`, `sandbox/tests.rs`, and the `resolveMemory` half of `memory.rs`. For these the evidence is the title check plus the differential runs above.
- `agent/tests/openai_cache.rs`, `agent/tests/delegates.rs`, `agent/tests/config_e2e.rs`: not read in full; `agent/tests/mcp_delegates.rs`: two cases read.
- `web_search` (Exa) was not called; its result formatting was compared only through the literal check.
- Provider request fields (`additionalRequestFields`, `params`, `anthropicTools`, `cacheConfig`) were compared by reading, not by running.
- Not run differentially: memory extraction, the `smart` preset, Cedar and natural-language policies, MCP servers loaded from a portable config, skills loading, `OTEL_TRACES_EXPORTER`.
- The SDK part of the phase diff (`strands-rs/strands/src`: `ToolAgent::call_tool`, `ToolCallOutcome::HookCancelled`, `InvocationState::detached`, `CancelSignal::abort_only`) was exercised only through the harness runs above, not reviewed line by line.
- PowerShell and Windows paths; live tests (not run, as instructed).

## Verdict

The gates pass and the harness matches TS byte for byte in every differential run except where a finding or a listed divergence says otherwise; 14 findings are open: behaviour differences in parity-1, -2, -7, -13 and -14, test and accounting gaps in parity-3, -5, -6, -8, -11 and -12, documentation in parity-4, -9 and -10.
