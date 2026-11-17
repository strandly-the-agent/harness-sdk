# Follow-ups

Written from the run's logs, the decision log (`.rust-port/DECISIONS-pending.md`), the review dispositions and the live result files of the branch.

## Site docs

- [ ] Open the docs-site PR that adds the Rust tabs (decision #50). Nothing under `site/` was changed on this branch.

## Parity automation

- [ ] Start the parity automation workstream (decision #16), after SDK v0.1.0.
- [ ] Decide whether that workstream takes over the two checks this run wrote: `strands-rs/docs/count-ts-cases.mjs` (TS case totals) and `strands-rs/docs/check-test-titles.mjs` (TS titles against Rust test names, for the SDK and, with `--parity harness-rs/docs/PARITY.md`, the harness).
- [ ] Keep `strands-rs/conformance/` (the fixture generator and the generated fixtures) in step with the TS SDK when the pin moves.

## Later v0.x items

From PROMPT.md section 5.2. None of these was built.

- [ ] Graph, Swarm and A2A, with the vended tools `graph`, `swarm` and `a2a_client` (TS `multiagent/` and `a2a/`). The multi-agent cases of `session-manager.test.ts` and `queue.test.ts` come with them.
- [ ] Bidirectional streaming.
- [ ] The `ContextOffloader` plugin.
- [ ] GoalLoop.
- [ ] Steering.
- [ ] Checkpointing, with the `checkpointing` key of `AgentConfig` and the `agent.checkpoint` tests.
- [ ] Decision models and the decision-model classifier of `HumanInTheLoop`; `ClassifierStrategy` then routes through the decision-model layer (P-456).
- [ ] Docker, SSH and OS-level sandboxes.
- [ ] S3 storage, vector search, embeddings and the Bedrock Knowledge Base memory store.
- [ ] MCP tasks, with `TasksConfig` and its request timeouts (P-237).
- [ ] Google, Vercel, Ollama and LiteLLM providers, and the harness model strings for them.
- [ ] Module references in portable harness config (they produce an "unsupported" error today).
- [ ] `MessageUpdatedEvent`, when TS adds it.
- [ ] A Rust CLI (`strands-cli` stays TypeScript).

## CI items that could not be validated locally

The workflows are staged in `.rust-port/ci-workflows/` because the token that persists the branch cannot push under `.github/workflows/` (P-13). They were checked with PyYAML and actionlint and have never run.

- [ ] Move the six files from `.rust-port/ci-workflows/` into `.github/workflows/`; `ci.yml` replaces the existing file and adds the `strands-rs` and `harness-rs` areas.
- [ ] Run "Rust: Test and Lint" (`strands-rs-test-lint.yml`) once and confirm its jobs pass on the hosted runners.
- [ ] Run "Harness Rust: Test and Lint" (`harness-rs-test-lint.yml`) once and confirm its lint, test, docs, features and MSRV jobs pass.
- [ ] Confirm in that run that the test job's Node setup and `npm ci` steps let the six `programmatic_tool_caller over MCP` cases run: no `SKIPPED programmatic_tool_caller over MCP` line in the test output (P-1198). Run actionlint on that file as well; the steps were added after the first check (`039033db0`).
- [ ] Run "Rust: Security Audit" (`rust-security-audit.yml`, `cargo deny` through the pinned action) once.
- [ ] Confirm on a pull request that `detect-changes` in `ci.yml` routes changes under `strands-rs/`, `harness-rs/` and the root Cargo files to the `rust` and `harness-rust` jobs, and that `ci-gate` waits for both.
- [ ] Decide whether `pr-metrics` scores Rust; no complexity adapter was written (P-12).
- [ ] Review the `deny.toml` exceptions that come from `monty`: the BlueOak-1.0.0 licence allowance and the ignored advisory RUSTSEC-2023-0089 (P-4).

## Open owner decisions

- [ ] P-1023 (with P-815): make the `#[doc(hidden)]` file modules `pub(crate)` before a published 0.1.0, and decide whether the families with two documented paths keep both.
- [ ] P-26 and P-646: keep Cedar non-optional with the exact `cedar-policy = "=4.14.0"` requirement, or put Cedar or its schema generation behind a cargo feature (this changes the five-feature list of PROMPT.md section 8.1).
- [ ] P-832: add an SDK accessor so the harness `supports_media` can tell an `OpenAIModel` on Bedrock Mantle from a `BedrockModel`, or keep judging by model id.
- [ ] P-873 and P-991: add a marker or downcast on the SDK `Tool` so programmatic tool callers are recognised by identity, or keep recognition by name.
- [ ] P-913: confirm that `EnvironmentContext` probes the platform through the sandbox as TS does, against PROMPT.md section 11 item 40.
- [ ] P-1120 (replaces P-938): decide whether an `agentConfig` key that neither SDK reads is an error or stays a warning, and whether the six keys the top-level config always overrides (`model`, `effort`, `contextManager`, `session`, `skills`, `memory`) should warn. Keys TS acts on and Rust cannot apply are errors already.
- [ ] P-958 and P-1142: accept that a consumer plugin stands in for a built-in one by its name or by the tool specs it vends, or add a typed marker to the SDK `Plugin` trait (for example a `kind()` string defaulting to `name()`); decision #15 keeps `as_any` out. What remains today is in the "recognising a consumer plugin" row of `harness-rs/docs/DIVERGENCES.md`.
- [ ] P-2: decide when the MSRV moves from 1.96; `monty` is held at 1.5 until then.
- [ ] Review the rest of `.rust-port/DECISIONS-pending.md`; the report lists the entries that change behaviour or public API the most.
- [ ] Decide which of the 42 entries in `.rust-port/ts-bugs.md` become TypeScript issues. None was filed, and no TypeScript file was changed (decision #25); eight entries say Rust keeps the TS behaviour so the SDKs act alike.

Opened by the Phase 12 review gate (`harness-rs/docs/reviews/phase-12.md`):

- [ ] P-1140: confirm that a callback the consumer set with `McpClient::set_on_tools_changed` stays in place when an agent initializes on the client (TS replaces it).
- [ ] P-1141 (finding correctness-5): decide the `wait_for_completion: false` case of a background task resumed after the model ended its turn. The model is still called with a history that ends in an assistant message, as in TS. The entry's options: the agent loop skips the model call of a pass that added no message (both SDKs), the scheduler returns the interrupt on the caller's next invocation, or the behaviour is kept and documented.
- [ ] P-1168 (finding api-12): make the SDK's `SnapshotField::ALL` and `SnapshotPreset::ALL` slices (`&'static [Self]`) before the first release, as the harness list constants are, or keep the arrays.
- [ ] P-1169 (finding api-13): keep `strands-harness` out of `[workspace.dependencies]`, or restore the entry that PROMPT.md section 8.1 names and accept cargo's unused-dependency warning until a member depends on the harness.
- [ ] P-1172 (finding api-18): decide whether `HarnessError::ModelFeatureDisabled` says `requires the strands-harness feature` and is folded into `FeatureDisabled`. The text is fixed by PROMPT.md section 10, so only the rustdoc was changed; the "Disabled provider" row of `harness-rs/docs/DIVERGENCES.md` changes with it.
- [ ] P-1180: decide whether `NotASandboxLocalEnvironment` gets a public working-directory option (TS has none). The live environment-context test defines its own sandbox for lack of one.

## Live tests that need credentials

- [ ] Run `anthropic_live.rs` (5 tests) with `ANTHROPIC_API_KEY`; no Anthropic request was sent in this run.
- [ ] Run `openai_live.rs` (8 tests) with `OPENAI_API_KEY`; no OpenAI request was sent in this run.
- [ ] Run the Anthropic and OpenAI cases of the summarize-tool-calls spec, which have no Rust live test yet.
- [ ] Run the `anthropic` and `openai` examples once with their keys; they are compile-checked only (P-525).
- [ ] Run `live_anthropic_tool_call_mcp_tool_interrupt_resume_and_session_resume` and `live_openai_tool_call_mcp_tool_interrupt_resume_and_session_resume` (`harness-rs/strands-harness/tests/live_e2e.rs`) with `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`. Both are written and skipped in every sweep; no request was sent to either provider.
- [ ] Test Bedrock API-key (bearer) authentication live; no Bedrock API key was available.
- [ ] Test Bedrock guardrails live; that needs an existing guardrail resource, and creating one was not allowed in this run.

- [ ] When the harness live sweep is run, set `STRANDS_TEST_NODE`, `STRANDS_TEST_NODE_MODULES` and `STRANDS_CROSS_SDK_HARNESS_TS` as `harness-rs/docs/reviews/live.md` shows; without them the MCP flow and the four cross-SDK tests skip with a message.
- [ ] Rerun the harness live sweep on the last commit. The four sweeps ran at `7273873c6` and `87521c86a`, before the Phase 12 review fixes; after them only the environment-context test ran, once.

## Deferred in the logs

SDK live coverage listed as not covered in `strands-rs/docs/reviews/live.md`:

- [ ] A live test for `models/openai/mantle-routing.test.node.ts` (P-560; the base-path resolver and the token minting are crate-private).
- [ ] Live tests for the Bedrock string model id, the region from `AWS_REGION` and explicit region precedence.
- [ ] Live tests for the OpenAI session cache key and the stateful-model case of the session manager spec.
- [ ] Live cases of `agent.test.ts` for PDF and document input, video, citations and `Message[]` input; audio input on Bedrock.
- [ ] Live cases of `agent.cancel.test.ts` for the timer and timeout forms.
- [ ] Live MCP tests over streamable HTTP (stdio only today), and the filtering and prefixing cases against the v2 server.
- [ ] The remaining `telemetry.test.node.ts` live cases: span events and timing attributes, tool-error and mid-loop error spans, `stream()`, concurrent agents.
- [ ] The interventions live cases beyond the ported ones, and providers other than Bedrock for them.
- [ ] Make `live_summarizes_the_tool_call_in_a_single_summarizer_request` independent of the model's wording; its last assertion failed in 2 of 35 runs.

Harness live coverage listed as not covered in `harness-rs/docs/reviews/live.md`:

- [ ] Enable the environment plugin in the six-tool and caching live tests, as TS does (P-1094). It reads the working directory of the agent's sandbox, so these tests need a sandbox with its own working directory, as the environment-context test has (P-1180).
- [ ] Extend the cross-SDK session check to the context manager's stash files and to memory files; it covers a completed turn with a tool call and a pending tool-approval interrupt. Multi-agent sessions come with the multi-agent work.
- [ ] Run the Bedrock Mantle flow at effort `off` again once `openai.gpt-oss-120b` accepts `reasoning.effort: "none"` or the harnesses stop sending it; the test runs at `low` (P-1092, and the TS bug list).
- [ ] Run `live_exa_web_search_returns_parsed_results` with `EXA_API_KEY` set; the recorded runs were keyless, and a keyless run skips when the rate limit is hit.

Other items:

- [ ] Call `shutdown_telemetry().await` on the exit path of every binary built on the harness (a later Rust CLI, examples), so the batch exporter flushes. The harness exports and documents the function and does not call it on its own, and Rust has no exit hook (P-1130, which closes P-923; Phase 10 finding correctness-5).
- [ ] Push the tag `strands-rs-v0.1.0-preview.1` (annotated, on `fa45de4be`) or recreate it where the branch lands; it is local only.
- [ ] Run the `streaming`, `structured_output`, `bedrock` and `mcp` examples once against Bedrock; they are compile-checked only, and `basic` was not rerun after its printer change (P-525).
- [ ] Remove `.rust-port/` before any PR (PROMPT.md section 9.7), after the staged workflows have moved and the report, the pending decisions and the TS bug list have been read.

Left by the Phase 12 review fixes (`harness-rs/docs/reviews/phase-12.md`), beyond the owner decisions above:

- [ ] Extend the strip of undeclared keys in typed tool input to union branches (`anyOf`, `oneOf`), record values and recursive `$ref` types, where zod drops the key and Rust still fails validation (finding parity-13, P-1195; a row of `strands-rs/docs/DIVERGENCES.md`).
- [ ] Give the SDK's `StdioServerParameters` (`strands-rs/strands/src/mcp/transport.rs`) a `Debug` that does not print its `env` values, as `McpServerConfig` has (finding correctness-4, P-1199).
- [ ] Bring two texts in line with the public `builders` module: the crate rustdoc in `harness-rs/strands-harness/src/lib.rs` says the modules are private, and `harness-rs/AGENTS.md` does not name the module (finding api-2, P-1154).
- [ ] Add a test for the subscriber order when `OTEL_TRACES_EXPORTER` is set (the harness installs the SDK's span-only global subscriber when none is set, and a later one is rejected). The README and crate docs state it from reading the SDK source; a test needs the process environment changed (finding api-14).
- [ ] Add the checks the early review's dispositions name as missing: a serialized-string test for the `description`-first order of `web_fetch`'s `prompt` property (parity-a2), the `await echo(x=a)` form of the deeply nested guest value (correctness-a4), and the huge-timeout case through the public options path (correctness-a5).

Left by the Phase 12 test tracks (`.rust-port/handoff/h12a-handoff.md`, `h12b-handoff.md`):

- [ ] Run the `bedrock-mantle/` rows of the two `openai-cache.test.ts` cases. The Mantle provider needs an endpoint override and a token provider reachable from the harness environment seam; `BedrockMantleConfig` has `region`, `credentials` and `expires_in_seconds` only (P-1074).
- [ ] Assert the per-turn dates in the durable-checkpoint case as TS does; it counts `<environment>` blocks, because `EnvironmentContext` reads the clock directly. A crate-private clock on the plugin would allow it.
- [ ] Decide whether the SDK gets an accessor for a provider model's config. The caching and effort cases of `agent.test.ts` and one line of the durable-checkpoint case assert what `create_harness` asks `resolve_model` for, where TS reads `agent.model.getConfig()` (P-1051).
- [ ] Assert identity directly in the three `createHarness` memory instance cases if the SDK gains `MemoryManager` store access or pointer equality; they assert by behaviour (P-1071).
- [ ] Cover in the end-to-end portable config tests (`agent/tests/config_e2e.rs`, P-1075): `~` expansion, the `smart` and natural-language intervention presets, `contextManager: "auto"`, the per-tool `web_fetch`, `subagent` and `programmatic_tool_caller` configs, and a Bedrock or Anthropic model string (only `openai/` can be pointed at a localhost server through the environment seam).
- [ ] The delegate cases whose TS parent takes `mcpServers: { srv: { command } }` pass a named client in `tools`; `keeps missing directories when multiple are given` shows that the absent directory does not fail the load, because the plugin's source list has no accessor.

Harness parity gaps: no case is open. The 29 declared gaps of `harness-rs/docs/PARITY.md` are values a typed option cannot hold, providers and module loading outside v0.1, states that cannot be built in Rust, and the unconditional Cedar dependency; the 10 that come back with later work are under "Later v0.x items" (the six Google and LiteLLM cases of `models.test.ts` and the four module-loading cases of `config.test.ts`).
