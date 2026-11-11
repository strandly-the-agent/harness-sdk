# Phase 1 review: types, wire and errors

Three independent reviewers (correctness, API, parity) reviewed the diff from the end of Phase 0 (`7f48a8bd7`) to the end of Phase 1. Every finding below was checked against the code and the TS spec before it was fixed or disputed. Each fix that changes behaviour comes with a test, and each of those tests fails when its fix is reverted. After the fixes, the full gates pass: fmt, clippy with all features and with no default features, tests both ways, docs, MSRV 1.96.0 and `cargo deny`. Each provider feature also passes clippy on its own, for both `strands` and `strands-harness`.

The gate run also found a warning that none of the reviewers reported: on Rust 1.96.0 the `dead_code` expectation on the crate-internal `InterruptState` is unfulfilled, which fails the CI MSRV job because it builds with `-D warnings`. It is fixed in `92b3ffe46`, and `cargo +1.96.0 check --workspace --all-features --locked` with `-D warnings` is now clean.

Ids: C = correctness, A = API, P = parity.

| finding | disposition | commit |
|---|---|---|
| C1. `tests/wire_constructors.rs` builds as a second integration-test binary besides `tests/main.rs`, which declares it as a module, so its tests run twice and the crate links twice | Fixed. Reproduced: `cargo test` listed `Running tests/wire_constructors.rs` next to `Running tests/main.rs`. The `strands` manifest sets `autotests = false` and declares one `[[test]] name = "main"`. `cargo test` now runs one integration binary. `docs/TESTING.md` describes the setup | `411b439a9` |
| C2. An interrupt with `"source": null` fails to load, and with it the whole interrupt state; TS reads it as `hook` (`interrupt.ts`, `data.source ?? 'hook'`) | Fixed. Reproduced. `source` now reads `null` and a missing field the same way, as `InterruptSource::Hook`. Test `reads_a_null_source_as_hook` | `c427ad268` |
| C3. Index-keyed byte objects with gaps, repeated keys or out-of-range indices (`{"0":1,"5":2}`) load as packed bytes without an error | Fixed. Reproduced. `JSON.stringify(Uint8Array)` writes every index from `0` to `n-1`, so anything else now fails with `invalid byte index`. Test `byte_objects_with_missing_or_repeated_indices_fail_to_load` | `db6f05c78` |
| A1. The struct variants of `CitationLocation` and `ToolChoice::Tool` have no `#[non_exhaustive]`, so adding a field later breaks callers that build them with struct literals | Fixed. Each struct variant is `#[non_exhaustive]`. `CitationLocation` gains `document_char`, `document_page`, `document_chunk`, `search_result` and `web` constructors, next to the existing `ToolChoice::tool`. The integration test builds every location through them and matches a variant from outside the crate | `37a5f197c` |
| A2. `ToolChoice::Auto {}` and `ToolChoice::Any {}` are awkward to write; make them unit variants with hand-written serde | Disputed. The TS wire shape is `{"auto":{}}` and `{"any":{}}` (`tools/types.ts`, `Record<string, never>`). Empty struct variants produce it from the serde derive. Unit variants would serialize as bare strings and need a hand-written `Serialize`/`Deserialize`, which is more code to keep in sync with the wire format. The braces are the cost of that choice, and the `tool-choice` fixture round-trip pins the shape | none |
| A3. `Usage::accumulate` and `Usage::total_prompt_tokens` are public, but TS keeps `accumulateUsage` out of `index.ts` and marks `totalPromptTokens` `@internal` | Fixed. Both are `pub(crate)`, with a `dead_code` expectation outside tests until the agent loop and telemetry call them | `37a5f197c` |
| A4. `SystemContentBlock::text(..)` duplicates `From<&str>`/`From<String>`, and `GuardContentBlock::text()` is a getter with the same name | Disputed. The constructor is a deliberate shorthand for building a block list in place, like `ToolChoice::tool(..)`, and is listed with the names Rust adds. In a mixed list such as `vec![SystemContentBlock::text(".."), CachePointBlock::new().into()]`, a bare `".."` has no type to convert into, so the shorthand is what callers reach for. The two `text` methods are on different types and their signatures differ (an associated constructor taking text, and a `&self` getter), so a call site can't confuse them | none |
| A5. `pub mod errors` makes callers write `strands::sandbox::errors::SandboxTimeoutError`, unlike the flat re-exports of the other modules; TS exports these errors from its package root | Fixed. `sandbox::errors` is private and the five errors are re-exported as `strands::sandbox::{SandboxAbortError, ...}` | `37a5f197c` |
| A6. Only four doctests; none for `ContentBlock`, `InterruptResponse`, `InterruptParams::builder()`, the strict parsers, `ToolResultBlock`, `Snapshot` or the crate root | Fixed. Doctests added for the crate root, `ContentBlock` (lenient versus strict parsing), `Message::from_message_data`, `InterruptParams` (builder and `From<&str>`), `InterruptResponse`, `ToolResultBlock` and `Snapshot`. `cargo test --doc` runs 11 | `d2d84a5ec` |
| P1. Restoring interrupt state fails with a JSON error when `pendingToolExecution` lacks a field, where TS gives a `SessionError` with the field name, and fails even when the state isn't activated, where TS ignores the pending data (`interrupt.ts`, `fromJSON` and `validatePendingToolExecution`) | Fixed. Reproduced. The pending data is read as raw JSON, kept only when the state is activated and the value is truthy (as TS's `if`), and missing fields reach the validator as `null`, which reports them with the TS texts. Tests: two new rows in the `has_description` table (no assistant message, no completed tool results) and `ignores_a_malformed_pending_tool_execution_when_the_interrupt_state_is_not_activated` | `c427ad268` |
| P2. The `types/__tests__/agent.test.ts` gap says all 30 cases arrive with `AgentResult`, but the 4 `checkpoint` cases test experimental checkpointing, which is out of v0.1 | Fixed. The gap reason lists the 4 `checkpoint` cases as out of scope and the other 26 as arriving with `AgentResult` | `4f9f847cb` |
| P3. Five test names don't follow the snake_case-of-the-TS-title rule: four mime tests and the interrupt `it.each` table test | Fixed. The mime tests sit in `to_mime_type` and `to_media_format` modules (`converts_s_to_s`, `is_case_insensitive`). The interrupt test is `throws_session_error_when_the_restored_pending_tool_execution_has_description` | `b98a9c4bf`, `c427ad268` |

Gates after the fixes (fmt, clippy, tests, docs, MSRV and `cargo deny`; 0 means passed):

```
fmt: 0
clippy: 0
clippy_nodef: 0
test_all: 0
test_nodef: 0
doc: 0
msrv: 0
deny: 0
tests all-features: passed=269 failed=0 ignored=0
```
