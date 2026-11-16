# Follow-ups

Markers to fill:

- TODO(lead): owner decisions opened by the Phase 12 review gate
- TODO(lead): harness live tests that could not run, from `harness-rs/docs/reviews/live.md`
- TODO(lead): items deferred by the Phase 12 tracks (harness parity gaps left open, review findings not fixed)

Drafted on `wip/p13` at `ba501ced3` from the run's logs. A `TODO(lead)` line marks a group that depends on work still in progress when this was written.

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
- [ ] P-938: decide whether an unsupported `agentConfig` key in portable config is an error or stays a warning.
- [ ] P-958: add `as_any` to `Plugin` so built-in plugins are recognised by type, or keep recognition by name.
- [ ] P-2: decide when the MSRV moves from 1.96; `monty` is held at 1.5 until then.
- [ ] Review the rest of `.rust-port/DECISIONS-pending.md`; the report lists the entries that change behaviour or public API the most.
- [ ] Decide which of the 39 entries in `.rust-port/ts-bugs.md` become TypeScript issues. None was filed, and no TypeScript file was changed (decision #25); seven entries say Rust keeps the TS behaviour so the SDKs act alike.

TODO(lead): owner decisions opened by the Phase 12 review gate

## Live tests that need credentials

- [ ] Run `anthropic_live.rs` (5 tests) with `ANTHROPIC_API_KEY`; no Anthropic request was sent in this run.
- [ ] Run `openai_live.rs` (8 tests) with `OPENAI_API_KEY`; no OpenAI request was sent in this run.
- [ ] Run the Anthropic and OpenAI cases of the summarize-tool-calls spec, which have no Rust live test yet.
- [ ] Run the `anthropic` and `openai` examples once with their keys; they are compile-checked only (P-525).
- [ ] Run the harness end-to-end tests for `anthropic/` and `openai/` model strings with their keys.
- [ ] Test Bedrock API-key (bearer) authentication live; no Bedrock API key was available.
- [ ] Test Bedrock guardrails live; that needs an existing guardrail resource, and creating one was not allowed in this run.

TODO(lead): harness live tests that could not run, from `harness-rs/docs/reviews/live.md`

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

Other items:

- [ ] Call the harness `shutdown_telemetry` on the exit path so the batch exporter flushes (P-923; Phase 10 finding correctness-5 left this to the harness). At `ba501ced3` nothing calls it.
- [ ] Push the tag `strands-rs-v0.1.0-preview.1` (annotated, on `fa45de4be`) or recreate it where the branch lands; it is local only.
- [ ] Run the `streaming`, `structured_output`, `bedrock` and `mcp` examples once against Bedrock; they are compile-checked only, and `basic` was not rerun after its printer change (P-525).
- [ ] Remove `.rust-port/` before any PR (PROMPT.md section 9.7), after the staged workflows have moved and the report, the pending decisions and the TS bug list have been read.

TODO(lead): items deferred by the Phase 12 tracks (harness parity gaps left open, review findings not fixed)
