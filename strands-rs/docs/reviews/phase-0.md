# Phase 0 review: setup and guardrails

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed `7296caba7..d2cce53e2`. All gates passed at review time; the findings below are about configuration, tooling, documentation and CI.

| Finding | Disposition | Commit |
|---|---|---|
| Correctness 1: `serde_json` without `float_roundtrip` changes some floats on a parse and re-serialize | Fixed: feature enabled, round-trip test added | `6756aa2e5`, `64159c70f` |
| Correctness 2: the secret scan's self-pattern exclusion hides keys on matching lines and shifts line numbers | Fixed: exclusion removed; patterns written so they can't match their own text | `454e94bb9` |
| Correctness 3: binary and `-diff` files are not scanned | Fixed: `--text --no-textconv --no-ext-diff` and `grep -a` | `454e94bb9` |
| Correctness 4: keyword patterns are case-sensitive | Fixed: case-insensitive second pass | `454e94bb9` |
| Correctness 5: gates run without `--locked` | Fixed in the gate script and the staged workflows | `454e94bb9`, `a2fc21afa` |
| Correctness 6: the full gate skips per-feature checks and the fixture regeneration diff; fixed output paths collide between concurrent runs | Fixed: per-feature checks, conformance diff, `GATE_OUT` output directory | `454e94bb9` |
| Correctness 7: lints don't catch string slicing, indexing, wrapping casts or unchecked counter arithmetic | Fixed: `string_slice`, `indexing_slicing`, `cast_possible_wrap`, `cast_precision_loss`, and `arithmetic_side_effects` denied in the counter module; existing code adjusted | `6756aa2e5`, `64159c70f` |
| Correctness 8: test-helper and example lint rules undocumented | Fixed in `AGENTS.md` and `TESTING.md` | `8d08f0aa3`, `46aa38235` |
| Correctness 9: no fixture exceeds serde_json's recursion limit | Fixed: `edge_cases/deep_nesting.json` from a real `ToolUseBlock` | `e57f78f05` |
| Correctness 10: format and stop-reason lists hand-transcribed | Fixed: read from TS source with the compiler API | `e57f78f05` |
| Correctness 11: CI filter misses dependency-driven fixture changes | Fixed: npm manifests, lockfile and `.node-version` added | `a2fc21afa` |
| API 1: no lint enforces "no `Any`, `TypeId` or downcasting" | Fixed: `disallowed-types` plus `disallowed_types = "deny"` | `6756aa2e5` |
| API 2: nothing enforces the `non_exhaustive` policy | Fixed: `exhaustive_enums` and `exhaustive_structs` denied; config structs carry a scoped `#[expect]` | `6756aa2e5` |
| API 3: two no-op lint lines; lint docs inaccurate | Fixed: lines removed, lint list in `AGENTS.md` matches `Cargo.toml` | `6756aa2e5`, `8d08f0aa3` |
| API 4: public files cite internal decision numbers | Fixed: removed | `95c800b0e` |
| API 5: public docs refer to "the pin" | Fixed: reworded | `95c800b0e`, `e57f78f05` |
| API 6: Rust forms of cross-SDK rules missing | Fixed in `strands-rs/AGENTS.md`. The root `AGENTS.md` is not changed beyond its layout section, which is the only part of it this port edits | `8d08f0aa3` |
| API 7: `strands::testing` names differ from the TS helpers without a name map | Deferred to Phase 2, where `strands::testing` is built; its name map is logged there | - |
| API 8: harness doesn't enable `otel` by default, while the TS harness always sets up telemetry | Fixed: `otel` in the harness default features | `6756aa2e5` |
| API 9: `cedar-policy` is non-optional, a semver hazard | Not applied: the design makes `cedar-policy` a non-optional dependency; recorded for the owner's decision | - |
| API 10: docs.rs metadata lacks `--cfg docsrs` | Fixed | `6756aa2e5` |
| API 11: CI area and push paths miss fixture-affecting changes | Fixed | `a2fc21afa` |
| API 12: harness-rs CI wiring doesn't mirror harness-ts | Fixed | `a2fc21afa` |
| API 13: SDK CI tests the whole workspace, duplicating harness CI | Fixed: `-p strands -p strands-macros` | `a2fc21afa` |
| API 14: security audit blocks on advisories, unlike the other audits | Fixed: bans, licenses and sources block; advisories are informational | `a2fc21afa` |
| Parity 1: checkpoint, Google and Vercel specs listed as in scope | Fixed: moved to out of scope | `95c800b0e` |
| Parity 2: case totals undercount parameterized and conditional cases | Fixed: AST-based counter committed as `docs/count-ts-cases.mjs` | `95c800b0e` |
| Parity 3: internal decision numbers in `PARITY.md` | Fixed (same as API 4) | `95c800b0e` |
| Parity 4: no live checklist for `strands-ts/test/integ` | Fixed | `95c800b0e` |
| Parity 5: run-state base commits wrong | Fixed | `650b99fdb` |
| Parity 6: browser-only specs not mentioned | Fixed: one line in both headers | `95c800b0e` |

The staged CI workflows were validated with a YAML parser and actionlint but not run.
