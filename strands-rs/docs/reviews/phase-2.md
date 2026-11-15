# Phase 2 review: model abstraction, streaming and retry

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed `StateStore`, the cancel primitives, `AgentMetadata`, `ModelInfo` and the embedded model-info table, the `Model` trait with stream aggregation, token counting, summarize and cache configuration, the model defaults, the strict-schema and native-output helpers, the retry strategies and backoffs, warn-once, and `strands::testing::MockModel`. The correctness and API reviews ran on a frozen checkout of `cb5a8e49d`, the parity review on `8cc7d1a10` (after the correctness fixes). After all fixes, the full gates (fmt, clippy with all features and with no default features, tests both ways, rustdoc with `-D warnings`, the 1.96.0 MSRV check and `cargo deny check`) pass at `0724c715f` with 749 tests (this count includes the first tools-layer commits that landed meanwhile).

## Correctness

| Finding | Disposition | Commit |
|---|---|---|
| The strict schema inlined `$ref` against the original schema and in map order, so tool schema bytes and recursive inlining differed from TS | Fixed: TS keyword order, refs resolved against the schema being rewritten; expected bytes from the TS function; new fixture `schemas/strict_json_schemas.json` | `16c6219b0`, `5bf5a86ee` |
| The default `model_info` ignored an overridden `model_id()` | Fixed | `fe1593cba` |
| Rust `trim` differs from JavaScript `String.prototype.trim` (U+FEFF, U+0085) | Fixed: a crate-internal ECMAScript trim used where TS trims | `ffe49aec1` |
| Object keys that look like array indexes are ordered first by JavaScript but kept in insertion order by Rust | Fixed: the crate's JSON writer and `StateStore` order keys as JavaScript does | `5bf5a86ee` |

The reviewer also confirmed the retry decision logic and defaults, backoff overflow handling, stream aggregation edge cases, the embedded model-info table and the summarize prompt text.

## API bar-raiser

| Finding | Disposition | Commit |
|---|---|---|
| `CacheTtl` did not keep the TS acronym casing | Fixed: renamed to `CacheTTL` | `2876f100d` |
| No conversion from `Arc<ConcreteModel>` into `ModelSource` | Fixed: `From<Arc<M>>` | `999c5595f` |
| Third-party providers could not merge `ModelInfo` records | Fixed: public `ModelInfo::merge` | `814054a64` |
| `CacheConfig::cache_key` did not take a string | Fixed: `From<&str>`/`From<String>` and builder `into` | `325294718` |
| Extension traits lacked examples and the `async_trait` requirement was undocumented | Fixed: doctests and the `strands::async_trait` re-export | `8cadacbf3` |
| `BackoffContext` promised additive fields but derived `Copy` and `Eq` | Fixed: derives `Clone` and `PartialEq` only | `35885b45a` |
| Invented names missing from the name map | Fixed | `a16a675e6` |

## Parity validator

| Finding | Disposition | Commit |
|---|---|---|
| Native-output tests did not assert the warnings TS asserts | Fixed | `5f774ef1b` |
| The strict-schema recursive-ref test did not assert the warning | Fixed | `0d0565a6b` |
| The summarize test checked the warning text but not that it was logged | Fixed: a scoped log recorder | `af1664531` |
| The retry debug log text differed from TS | Fixed | `16b444fe9` |
| Three parity totals missed parameterised cases | Fixed | `913a2f3bb` |
| Two test names did not follow the TS titles | Fixed | `4eb700330` |
