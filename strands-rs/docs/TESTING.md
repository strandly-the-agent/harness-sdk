# Testing guidelines - Rust SDK

The TypeScript test suite is the behavioural spec. Every applicable `it(...)` case in an in-scope TS spec file has a Rust test, and [PARITY.md](PARITY.md) records the mapping.

## Layout

- Unit tests live beside the code in `#[cfg(test)] mod tests` (or `src/**/tests.rs`).
- Cross-module behaviour goes in `strands-rs/strands/tests/`.
- Macro tests live in `strands-rs/strands-macros/tests/` and use `trybuild` for pass and compile-fail cases.
- `strands::testing` is a public module with test doubles: `MockModel` (scripted turns: text, tool use, reasoning, errors including throttles, delays through an injected sleeper; records every `ModelRequest`), a lower-level scripted `ModelStreamEvent` provider, `HookRecorder` (records the order events fired), and `test_agent(..)` helpers.

## Naming

- A ported test is named with the snake_case form of the TS `it(...)` title.
- The snake_case form: drop `it.each` placeholders (`%s`, `$name`) and apostrophes inside words, split camelCase words (`structuredOutput` is `structured_output`, `toJSON` is `to_json`), lowercase the title, turn every run of characters outside `a-z0-9` into one `_`, trim `_` from both ends, and prefix `_` when the result starts with a digit. `it("doesn't retry 4xx errors")` is `doesnt_retry_4xx_errors`. Names aren't truncated. An `it.each` case is one test named after its template text.
- When a title repeats in a spec under different `describe` blocks, each `describe` becomes a `mod` and the tests keep the exact name.
- A case covered by a differently named test, by a test that merges several cases, or not ported at all is declared in [test-title-map.json](test-title-map.json) (`{ "<spec path>": { "<ts title>": "<rust test name>" | { "gap": "<reason>" } } }`).
- `node strands-rs/docs/check-test-titles.mjs [--report] [filter...]` checks every in-scope [PARITY.md](PARITY.md) row against these rules and fails on a missing case or a wrong `ported` count. It needs the `typescript` package (run it from a checkout with `node_modules`, or set `NODE_PATH`).
- A Rust-only test carries a `// No TS case:` comment with the reason.
- The test module's doc comment names the source spec file, for example `//! Ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts`.
- Tests written during development carry no issue references.

## Fixtures

`strands-rs/conformance/fixtures/` holds JSON written by strands-ts. A wire test parses the fixture, serializes it again and compares the bytes with the TS-written JSON. Regenerate with the command in [the conformance README](../conformance/README.md); CI regenerates and fails on any diff.

## Rules

- Never sleep for real. Inject a sleeper and a clock, or use `tokio::time::pause()`; seed any RNG.
- Assert wire shapes with `serde_json::json!` literals, never with `Debug` output.
- Assert the exact TS error text wherever the TS test asserts it or cross-SDK tests check it.
- Unit tests make no network calls. Test providers with constructed SDK types, a local test server, or a transport seam.
- Never call `std::env::set_current_dir` or mutate process environment in tests. Code that reads the working directory or environment has a crate-internal seam; test through it. Spawned children get environment through the sandbox's `env` option.
- Compile-time `Send` checks cover `Agent`, `AgentResult`, `ToolAgent`, `ToolContext`, `Error`, every event type, `AgentStream<'_>`, and the futures returned by `agent.invoke(..)`.
- At least one agent-loop test with tools runs on `#[tokio::test(flavor = "current_thread")]`.
- Test files that need a cargo feature start with `#![cfg(feature = "...")]`.
- Clippy's `allow-*-in-tests` settings only reach `#[test]` functions and `#[cfg(test)]` items. Helpers in `tests/` files carry `#[cfg(test)]`, or the file starts with `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]`.
- Test helpers shared across unit-test modules are `pub(crate)`, never `pub`.
- An example's `main` returns a `Result` and uses `?`, never `unwrap` or `expect`.
- Canonicalize paths before comparing them (macOS `/var` and `/private/var`).

## Running

```bash
cargo test --workspace --all-features
cargo test --workspace --no-default-features
cargo test -p strands --all-features <filter>
```

Doctests run as part of `cargo test`.

## Live tests

Live tests call real model providers. The live checklist in [PARITY.md](PARITY.md#live-checklist) lists the TS integration specs they cover. They are `#[ignore = "live <provider>"]`, named `live_*`, and skip with a message when credentials are missing.

```bash
cargo test -p strands --all-features -- --ignored live_
```

- Credentials come from the environment only: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and the default AWS credential chain for Bedrock and Bedrock Mantle. Tests never print them.
- Model ids and regions follow the TS integration fixtures (`strands-ts/test/integ/__fixtures__/model-providers.ts`).
- Assertions check the trace and side effects (tool attempted or succeeded, tool name, parsed input, tool-result content, files written, stop reason, usage and cache counters), never only the model's wording.
- Results are recorded in `docs/reviews/live.md`: test names, pass or fail, model ids, regions, and exact error text on failure.
