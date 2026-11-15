# Testing guidelines - Strands Rust SDK

The TypeScript test suite (`strands-ts/src/**/__tests__`) is the behavioural spec for `strands`, and `harness-ts/test/` is the spec for `strands-harness`. Every applicable TypeScript `it(...)` case has a Rust test with the same intent. [PARITY.md](PARITY.md) and [harness-rs/docs/PARITY.md](../../harness-rs/docs/PARITY.md) track the ported cases per spec file, and every case that isn't ported is listed there with a reason (out of scope, a TypeScript-only mechanism, or deprecated).

## Layout

- Unit tests live beside the code in `#[cfg(test)] mod tests` (or `src/**/tests.rs`), mirroring `__tests__/` next to the TypeScript module.
- Cross-module behaviour (agent loop, tools and hooks together) goes in `strands-rs/strands/tests/`.
- Macro tests, including compile-fail cases, live in `strands-rs/strands-macros/tests/` and use `trybuild`.
- Each Rust test is named with the snake_case form of the TypeScript `it(...)` title, and the test module's doc comment names the source spec file:

  ```rust
  //! Ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts
  ```

- A test file that needs a cargo feature starts with `#![cfg(feature = "...")]`, so `cargo test --no-default-features` builds and passes.

## Fixtures

### Conformance fixtures

`strands-rs/conformance/fixtures/` holds JSON written by the TypeScript SDK: messages and every content block, stream events, hook events, snapshots, session manifests and immutable history, interrupt state, stash records, and metric and span names, including edge cases (nulls, integer formatting, surrogates, non-canonical base64). Rust tests parse each fixture, serialize it back, and compare against the TypeScript-written bytes. Most fixtures are pretty-printed `toJSON()` output (compare after re-serializing pretty); `persisted/**` holds the exact compact bytes TypeScript handed to storage. `fixtures/INDEX.md` lists every fixture and the TypeScript API that produced it, and [conformance/README.md](../conformance/README.md) describes the byte conventions in full. The tests that consume a fixture arrive with the module that implements its types.

Regenerate the fixtures from the repo root:

```bash
npm ci && npm run build
npx tsx strands-rs/conformance/generate.ts
```

`npm run generate` from `strands-rs/conformance/` runs the same command. The generator is deterministic and rewrites `fixtures/` from scratch, so an unchanged TypeScript SDK produces identical files. CI regenerates the fixtures and fails if the result differs from the committed files, so commit regenerated fixtures together with the change that caused them. Never edit a fixture by hand.

### `strands::testing` helpers

`strands::testing` is a public module, so integration tests and downstream crates can use it.

| Helper | Use |
|---|---|
| `MockModel` | Agent-loop tests: scripted turns (text, tool use, reasoning, errors including throttling, delays through the injected sleeper). Records every `ModelRequest` it receives. |
| Scripted `ModelStreamEvent` provider | Model-layer tests that need exact control over the stream event sequence and errors. |
| `HookRecorder` | Registers a callback for every hook event and records the order they fired. |
| `test_agent(..)` helpers | Agents with a mock model and empty registries for plugin and hook tests. |
| `tempfile::tempdir()` | Storage, session and sandbox tests. |

## Rules

- Async tests use `#[tokio::test]`. Never `block_on` inside a test. Run at least one agent-loop test with tools on `#[tokio::test(flavor = "current_thread")]`.
- Never sleep for real. Inject a sleeper and a clock, or use `tokio::time::pause()`. Seed any RNG (for example retry jitter).
- Assert wire shapes with `serde_json::json!` literals, never with `Debug` output.
- Assert the exact TypeScript error text wherever the TypeScript test asserts it or a cross-SDK test checks it (`assert_eq!(err.to_string(), "...")`).
- Unit tests make no network calls. Test providers with constructed SDK types, a local HTTP test server, or a transport seam.
- Never mutate process env (`std::env::set_var` is `unsafe` in edition 2024, and `unsafe_code` is forbidden). Every env-reading path takes a crate-internal env-lookup seam; spawned children get env through the sandbox's `env` option.
- Never call `std::env::set_current_dir`; it is process-global and races parallel tests. Code that resolves relative paths takes a crate-internal base-directory seam.
- Check `Send`, and `Sync` where shared, at compile time with `fn assert_send<T: Send>(_: &T)` and `fn assert_sync<T: Sync>(_: &T)` helpers: `Agent`, `AgentResult`, `ToolAgent`, `ToolContext`, `Error`, every event type and `AgentStream<'_>` are `Send` (and `Sync` where shared), and the futures from `agent.invoke(..)` and `create_harness(..)` are `Send`.
- Canonicalize paths before comparing them (macOS `/var` versus `/private/var`).
- `#[test]` functions and `#[cfg(test)]` modules may use `unwrap` and `expect`; integration-test crates under `tests/` with helper functions add a crate-level `#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test code")]`. Tests carry no issue references.

## Running

From the repo root:

```bash
cargo test --workspace --all-features                  # everything, including doctests
cargo test --workspace --no-default-features           # without the bedrock feature
cargo test -p strands --all-features sliding_window    # by name filter
cargo test -p strands-harness --all-features           # the harness only
```

## Live tests

Live tests call real models. They are `#[ignore = "live <provider>"]`, named `live_*`, and run only when you ask for them:

```bash
cargo test -p strands --all-features -- --ignored live_
cargo test -p strands-harness --all-features -- --ignored live_
```

- Credentials come from the environment only: `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` for the direct providers, and the default AWS credential chain (with `AWS_REGION`) for Bedrock and Bedrock Mantle. A test whose credentials are missing skips with a message.
- Never print, log or commit a key, and never dump the environment. Configs that hold keys redact them in `Debug`. Model-run tools such as `shell` inherit the process environment, so live prompts never ask the model to print it, and you check output for secrets before reading it.
- Use the regions and model ids the TypeScript integration tests use (`strands-ts/test/integ/__fixtures__/model-providers.ts`), preferring small models unless a test needs a specific capability.
- Assert on the trace and side effects, not on the model's wording: tool attempted or succeeded, tool name, parsed input, tool-result content, files written, stop reason, `Usage` and cache counters. Group features into as few calls as practical.
- The TypeScript integration suites (`strands-ts/test/integ/`, `harness-ts/test/integration/`) are the live checklist. Record results in `docs/reviews/live.md` (see [reviews/README.md](reviews/README.md)).
