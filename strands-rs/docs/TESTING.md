# Testing guidelines - Strands Rust SDK

The TS test suite (`strands-ts/src/**/__tests__`) is the behavioural spec. Every in-scope TS `it(...)` case has a Rust test with the same intent, and [PARITY.md](PARITY.md) maps each TS spec file to its Rust tests.

## Layout

- Unit tests live beside the code in `#[cfg(test)] mod tests` (or `src/**/tests.rs`), mirroring `__tests__/` next to the TS module.
- Each test is named with the snake_case form of the TS `it(...)` title. The test module's doc comment names its source spec, for example `//! Ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts`.
- Cross-module behaviour (agent loop with tools, hooks and plugins) goes in `strands/tests/`.
- Test files that need a cargo feature start with `#![cfg(feature = "...")]`, so `--no-default-features` runs stay green.
- Live tests are `#[ignore = "live <provider>"]`, named `live_*`, and skip with a message when their credentials are missing.

## Fixtures

| Fixture | Use |
|---|---|
| `strands::testing::MockModel` | Agent-loop tests: scripted turns (text, tool use, reasoning, errors including throttles, delays through the injected sleeper); records every `ModelRequest`. |
| Scripted `ModelStreamEvent` provider in `strands::testing` | Model-layer tests that need exact control over the event sequence. |
| `strands::testing::HookRecorder` | Records the order in which hook events fired. |
| `strands::testing::test_agent(..)` | Agents with a mock model for plugin and hook tests. |
| `conformance/fixtures/*.json` | TS-written JSON. Rust parses each file, serializes it again and compares bytes. |
| `tempfile::tempdir()` | Storage and session tests. |

### Conformance fixtures

`conformance/generate.ts` runs under `tsx` against the built TS SDK and writes the fixtures deterministically: it patches the clock, `performance.now`, `crypto.randomUUID` and `crypto.getRandomValues` before importing the SDK. Regenerate from the repository root:

```bash
npm ci && npm run build
npm run generate --prefix strands-rs/conformance
git diff --exit-code strands-rs/conformance/fixtures
```

CI runs the same commands and fails on a diff. When a TS wire change lands, commit the regenerated fixtures and the Rust change together.

## Rules

- Async tests use `#[tokio::test]`. Run at least one agent-loop test with tools on `#[tokio::test(flavor = "current_thread")]`.
- Never sleep for real. Retry, backoff and timeouts take an injected sleeper and clock, or the test uses `tokio::time::pause()`. Seed any RNG.
- Assert wire shapes with `serde_json::json!` literals, never with `Debug` output.
- Assert the exact TS error text where TS tests assert it or cross-SDK tests check it (`assert_eq!(err.to_string(), "...")`).
- Unit tests make no network calls. Provider tests use constructed SDK types, a local HTTP test server, or a transport seam.
- Never mutate process state: no `std::env::set_var`/`remove_var` (they are `unsafe` in edition 2024) and no `std::env::set_current_dir`. Code that reads env or the working directory has a crate-internal seam that tests use instead.
- Compare paths after canonicalizing them (macOS `/var` is `/private/var`).
- Public types and futures that must be `Send` get compile-time checks (`fn assert_send<T: Send>(_: &T)`).

## Running

```bash
cargo test --workspace --all-features                 # everything
cargo test --workspace --no-default-features          # without providers
cargo test -p strands --all-features sliding_window   # filter by name
```

### Live tests

Live tests read credentials from the environment only: AWS through the default credential chain (Bedrock and Bedrock Mantle), `ANTHROPIC_API_KEY`, and `OPENAI_API_KEY`. They use the regions and model ids of the TS integration tests (`strands-ts/test/integ/__fixtures__/model-providers.ts`) and assert on traces and side effects (tool attempted, tool input, tool result, stop reason, usage), not on model wording.

```bash
cargo test -p strands --all-features -- --ignored live_
cargo test -p strands-harness --all-features -- --ignored live_
```

Results are recorded in `docs/reviews/live.md`.
