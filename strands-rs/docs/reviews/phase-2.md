# Phase 2 review: model abstraction, streaming and retry

Three independent reviewers (correctness, API, parity) reviewed the diff from the end of Phase 1 (`705c52190`) to the end of Phase 2. Every finding below was checked against the code and the TS spec before it was fixed. None was disputed. Each fix that changes behaviour comes with a test. After the fixes, the full gates pass: fmt, clippy with all features and with no default features, tests both ways, docs, MSRV 1.96.0 and `cargo deny`. Each provider feature also passes clippy on its own, for both `strands` and `strands-harness`.

The parity reviewer's gate run could not check MSRV because the disk filled up (`No space left on device`). MSRV was re-run on a cleaned target after the fixes and passes.

Adding the missing wire fields to the stream fixture (P5) found a byte difference that no reviewer reported: Rust wrote `outputRedaction` as `redactedContent` then `replaceContent`, while TS writes `replaceContent` first (`bedrock.ts` builds the object with `replaceContent` and adds `redactedContent` after). The `RedactOutputContent` fields are now declared in the TS order, and the fixture round-trips byte for byte (`6c2ca6dec`).

Ids: C = correctness, A = API, P = parity.

| finding | disposition | commit |
|---|---|---|
| C1. Whitespace-only checks in aggregation and `summarize` use Rust `str::trim`, which trims U+0085 and keeps U+FEFF; JS `trim` does the opposite | Fixed. A crate-internal `js_trim` matches JS `String.prototype.trim` and is used in both places. Tests: `drops_text_blocks_that_javascript_trims_to_empty`, `trims_the_reply_as_javascript_does`, `trims_like_javascript` | `66b8ddaad` |
| C2. The default `Model::model_info` looks up `config.model_id` instead of the overridable `model_id()` (TS uses the `modelId` getter) | Fixed. The lookup goes through `self.model_id()`. Test: `looks_up_model_info_by_the_model_id_method` | `66b8ddaad` |
| C3. `MockModel::with_delay` panics outside a tokio runtime, and the doc example drives the mock with `futures::executor::block_on` | Fixed. With a delay set and no tokio runtime, the turn fails with a `ModelError` instead of panicking. Test: `a_delay_outside_a_tokio_runtime_is_a_model_error` | `abcdc1913` |
| C4. `StateStore` keys and the JSON writer keep insertion order; JS lists array-index keys first, in ascending order | Fixed. `StateStore::keys`, `get_all` and the persisted-data writer `types::json::to_vec` order objects as JS does. Tests: `lists_array_index_keys_first_as_js_objects_do`, `writes_objects_in_javascript_key_order` | `66b8ddaad` |
| C5. Tool input that `JSON.parse` accepts is rejected: numbers beyond the `f64` range and nesting deeper than 128 levels | Numbers fixed: such literals read as `null`, which is what TS persists for `Infinity`. Tests: `reads_out_of_range_numbers_as_null`, `reads_out_of_range_tool_input_numbers_as_null`. Nesting kept as a recorded divergence: lifting `serde_json`'s recursion limit lets a hostile model reply overflow the stack, which aborts the process instead of returning an error. Both rows are in `DIVERGENCES.md` | `66b8ddaad`, `a76adf36c` |
| C6. The `Model` doc example's `update_config` resets every field the update leaves unset; the correct `BaseModelConfig::merge` is `pub(crate)` | Fixed. `merge` is public and the doc example uses it; the doctest asserts that an unset field survives an update | `66b8ddaad` |
| A1. `ConstantBackoff::default()` waits 0 s; TS waits 1 s | Fixed. `Default` goes through `ConstantBackoffOptions::default()`. Test: `defaults_delay_ms_to_1000` asserts `ConstantBackoff::default()` | `ee125ebef` |
| A2. The `bon` builders of the four retry option structs require every field, so the TS defaults never apply through the builder | Fixed. Each field has `#[builder(default = ..)]` from the same consts as the hand-written `Default`. Tests: `builders_without_fields_match_the_defaults` and the builder case in the default retry strategy tests | `ee125ebef` |
| A3. `BaseModelConfig::merge` is private, so third-party providers can't shallow-merge, and the doc example merges wrongly | Fixed with C6 | `66b8ddaad` |
| A4. `ModelCapabilities`, `ModelLimits` and `ModelInfo` derive `Copy`, a semver hazard when TS adds a non-`Copy` field | Fixed. `Copy` is dropped from the three structs; they keep `Clone` | `66b8ddaad` |
| A5. `BackoffContext` is exhaustive with public fields although the SDK builds it and hands it to user code | Fixed. `#[non_exhaustive]`, built with `new(attempt)`, `with_elapsed` and `with_last_delay`; the crate's own code and tests use the constructors | `ee125ebef` |
| A6. `MockModel::with_delay(&self) -> &Self` breaks the crate's consuming `with_*` convention | Fixed. Renamed to `set_delay(&self, Duration) -> &Self`, which chains like `add_turn` | `abcdc1913` |
| A7. The port's name map wasn't extended for Phase 2, and some renamed or invented names aren't logged | Fixed. The name map now has a Phase 2 block (the `*Ms` to `Duration` renames, `JitterKind`, `RetryDecision`, the stream event names, the new constructors and setters, `BaseModelConfig::merge`, `BackoffContext` constructors, the `MockModel` methods). The published API is unchanged by this finding | `acb2ceb00` |
| A8. No doctest on `ModelRetryStrategy`, `BackoffStrategy`, `ModelStreamEvent` or `TestModelProvider` | Fixed. Compiled examples of a custom `ModelRetryStrategy` (with `#[async_trait]`), a custom `BackoffStrategy`, a `ModelStreamEvent` and `TestModelProvider` driving `stream_aggregated` | `ee125ebef`, `abcdc1913` |
| P1. `ConstantBackoff::default()` waits 0 ms; TS 1000 ms | Fixed with A1 | `ee125ebef` |
| P2. The retry option builders make every option required | Fixed with A2 | `ee125ebef` |
| P3. `PARITY.md` counts the native-output spec as `10/11`; it is `14/15` | Fixed | `a76adf36c` |
| P4. The `models/model/tests.rs` module doc comment doesn't name its TS spec | Fixed. The `Ported from strands-ts/src/models/__tests__/model.test.ts` line is in the `//!` header | `66b8ddaad` |
| P5. The model stream fixture leaves out `outputRedaction`, the cache usage fields, `timeToFirstByteMs` and a `toolUseStart` without `reasoningSignature` | Fixed. `generate.ts` writes those events, the fixture is regenerated and the round-trip test counts 18 events. This found the `outputRedaction` key-order difference described above | `a0d2b3cd6`, `6c2ca6dec` |

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
tests all-features: passed=529 failed=0 ignored=0
```
