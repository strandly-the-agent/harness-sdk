# Testing Guidelines - Strands Rust SDK

The TS test suite (`strands-ts/src/**/__tests__`) is the behavioural spec. Every TS `it(...)` in
scope has a Rust test with the same intent; [PORT_REPORT.md](PORT_REPORT.md) holds the matrix.

## Layout

- Unit tests live beside the code in `#[cfg(test)] mod tests` (mirrors `__tests__/` next to the
  module). Name tests after the TS `it(...)` title in snake_case.
- Cross-module behaviour (agent loop + tools + hooks) goes in `crates/strands-agents/tests/`.
- Shared fixtures live in `src/test_support.rs` (compiled under `cfg(test)` and the
  `test-support` feature so integration tests and downstream crates can use them).
- Live Bedrock tests live in `tests/integ_bedrock.rs` (port of `strands-ts/test/integ`), are
  `#[ignore]`d, and run explicitly with AWS credentials:
  `AWS_REGION=us-west-2 cargo test --all-features --test integ_bedrock -- --ignored`.
  The three guardrail tests additionally need `STRANDS_TEST_GUARDRAIL_ID` (a word-filter guardrail
  blocking `CACTUS`, see the file header) and skip with a message when it is unset.

## Fixtures

| Fixture | Use |
|---|---|
| `MockMessageModel` | Agent-loop tests: give it the assistant messages (content blocks + stop reason) to return in order; it generates the stream events. |
| `TestModelProvider` | Model-layer tests: exact control over the `ModelStreamEvent` sequence (and errors). |
| `HookRecorder` | Registers a callback for every event type and records the order they fired. |
| `test_agent(..)` helpers | `Agent` handles with a mock model and empty registries for plugin/hook tests. |
| `tempfile::tempdir()` | Session storage tests. |

## Rules

- Async tests use `#[tokio::test]`. Never `block_on` inside a tokio test.
- Never sleep for real: retry/backoff takes an injectable sleeper.
- Assert on **wire shapes** with `serde_json::json!` literals, not on `Debug` output.
- Assert exact error messages where TS tests do (`assert_eq!(err.to_string(), "...")`).
- Unit tests make no network calls; Bedrock unit tests exercise request formatting and response
  parsing with constructed SDK types. Live behaviour is covered by `tests/integ_bedrock.rs`.
- Keep tests deterministic: seed `rand` where jitter is involved or assert ranges.

## Running

```bash
cargo test --all-features                 # everything
cargo test --all-features sliding_window  # by name filter
cargo test --no-default-features          # without the bedrock feature
```
