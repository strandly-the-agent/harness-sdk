# Testing guidelines - Rust SDK

The TypeScript test suites are the behavioural spec: `strands-ts/src/**/__tests__/` for the SDK and `harness-ts/test/` for the harness. Every applicable TypeScript `it(...)` case in an in-scope spec file has a Rust test with the same intent. [PARITY.md](PARITY.md) (and [harness-rs/docs/PARITY.md](../../harness-rs/docs/PARITY.md)) track the count per spec file.

## Layout

- Unit tests live beside the code in `#[cfg(test)] mod tests`, or in a sibling `tests.rs` when the module is large. This mirrors the TypeScript `__tests__/` folders next to each module.
- Cross-module behaviour (agent loop with tools, hooks, sessions) goes in `strands-rs/strands/tests/`.
- Macro tests, including trybuild compile-fail cases, live in `strands-rs/strands-macros/tests/`.
- Harness tests live in `harness-rs/strands-harness/` under the same rules.
- Test names are the snake_case form of the TypeScript `it(...)` title. The test module's doc comment names the source spec file:

  ```rust
  //! Ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts
  ```

- A test file that needs a cargo feature starts with `#![cfg(feature = "...")]`, so `--no-default-features` runs stay green.

## Fixtures

| Fixture | Use |
|---|---|
| `strands::testing::MockModel` | Agent-loop tests. Script the turns (text, tool use, reasoning, errors including throttles, delays through the injected sleeper); it records every `ModelRequest` it receives. |
| Scripted `ModelStreamEvent` provider (`strands::testing`) | Model-layer tests that need exact control over the stream event sequence and errors. |
| `strands::testing::HookRecorder` | Registers for every hook event and records the order they fired. |
| `strands::testing::test_agent(..)` | Agents with a mock model and empty registries for plugin and hook tests. |
| `strands-rs/conformance/fixtures/` | JSON produced by the TypeScript SDK. Wire tests deserialize, re-serialize and compare against it. |
| `tempfile::tempdir()` | Storage, session and sandbox tests. |

`strands::testing` is a public module, so harness tests and downstream crates can use the same fixtures.

Conformance fixtures are regenerated from the repository root with `npm ci && npm run build`, then `npx tsx strands-rs/conformance/generate.ts`. Never edit them by hand; CI fails if the regenerated output differs from what is committed.

## Rules

- Async tests use `#[tokio::test]`. Run at least one agent-loop test with tools on `#[tokio::test(flavor = "current_thread")]`. Never `block_on` inside a tokio test.
- Never sleep for real. Inject a sleeper and a clock, or use `tokio::time::pause()`. Seed any RNG, or assert ranges where jitter is involved.
- Assert wire shapes with `serde_json::json!` literals, never with `Debug` output.
- Assert the exact TypeScript error text wherever the TypeScript test asserts it or another SDK reads it (`assert_eq!(err.to_string(), "...")`).
- Unit tests make no network calls. Test providers with constructed SDK types, a local test server or a transport seam.
- Never call `std::env::set_current_dir` in tests: it is process-global and races parallel tests. Code that resolves relative paths takes a crate-internal base-directory seam, and tests go through it.
- Never change process environment variables in tests. Code that reads the environment takes a crate-internal env-lookup seam; spawned children get their environment through the sandbox's `env` option.
- Compile-time `Send` checks: `Agent`, `AgentResult`, `ToolAgent`, `ToolContext`, `Error`, every event type and `AgentStream<'_>` are `Send` (and `Sync` where shared), and the futures from `agent.invoke(..)` and `create_harness(..)` are `Send`. Assert it with a `fn assert_send<T: Send>(_: &T)` helper.
- Canonicalize paths before comparing them (macOS resolves `/var` to `/private/var`).
- A TypeScript case that isn't ported gets a reason in `PARITY.md`: out of scope, TypeScript-only mechanism, or deprecated.

## Running

```bash
cargo test --workspace --all-features                  # everything
cargo test --workspace --no-default-features           # without the default bedrock feature
cargo test -p strands --all-features sliding_window    # filter by name
cargo test -p strands --doc --all-features             # doctests only
```

Redirect long output to a file and read the summary with `grep '^test result:'`.

## Live tests

Live tests call real model providers. They port the TypeScript integration suites (`strands-ts/test/integ/` and `harness-ts/test/integration/`) and are tracked in the live checklist tables of the parity files.

- Mark each live test `#[ignore = "live <provider>"]` and name it `live_*`. It reads credentials from the environment and returns early with a message when they are missing, so it never fails for lack of credentials.
- Run them explicitly:

  ```bash
  cargo test -p strands --all-features -- --ignored live_
  cargo test -p strands-harness --all-features -- --ignored live_
  ```

- Bedrock and Bedrock Mantle use the default AWS credential chain. Anthropic and OpenAI read `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`. Use the regions and model ids from `strands-ts/test/integ/__fixtures__/model-providers.ts`.
- Assert on the trace and side effects, not on wording: tool attempted or succeeded, tool name, parsed input, tool-result content, files written, stop reason, `Usage` and cache counters. Never assert only that the model's text mentions something.
- Never print keys, and never `Debug`-format a config that holds one. Live prompts never ask the model to print the environment.
- Record results in `strands-rs/docs/reviews/live.md` (harness results in `harness-rs/docs/reviews/live.md`): test names, pass or fail, model ids, regions, and the exact error text on failure. No secrets, account ids or full response bodies.
