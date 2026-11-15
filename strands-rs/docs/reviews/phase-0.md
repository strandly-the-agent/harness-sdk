# Phase 0 review: setup and guardrails

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed the workspace skeleton, manifests, lint and `cargo deny` configuration, CI workflows, conformance generator and fixtures, and the AGENTS, TESTING, PARITY and DIVERGENCES documents. The correctness review ran on a frozen checkout of `56f05de24`; the other two on the same tree. After the fixes, the full gates (fmt, clippy with all features and with no default features, tests both ways, rustdoc with `-D warnings`, the 1.96.0 MSRV check and `cargo deny check`) pass at `0371b1577` with 256 tests.

## Correctness

| Finding | Disposition | Commit |
|---|---|---|
| `clippy.toml` exempts only `#[test]` functions and `#[cfg(test)]` modules, so plain helpers under `tests/` can't use `unwrap`, but the docs said tests may use it everywhere | Fixed: AGENTS.md and TESTING.md now state the scope and the crate-level `#![expect(..)]` for integration-test crates | `0371b1577` |
| A zod (or other npm) version bump in `package-lock.json` would not rerun the conformance fixture diff | Fixed: `package-lock.json` and `strands-ts/package.json` added to the `strands-rs` CI filter and the push paths | `0371b1577` |
| A hand edit to the generated `fixtures/INDEX.md` would not run the fixture diff (excluded by the `!(*.md)` filter) | Fixed: `strands-rs/conformance/fixtures/**` added to the `strands-rs` filter | `0371b1577` |

Checked with no finding: each cargo feature alone and no default features build; one copy each of hyper, rustls, h2, reqwest, OpenTelemetry, cedar-policy-core and tokio, and no legacy hyper 0.14 stack; `preserve_order` on `serde_json` and `schemars`; every lint deny in AGENTS.md fires in a scratch crate; `cargo deny` rules enforce; MSRV check passes; the generator's determinism patches load before strands-ts and repeated runs are byte-identical; CI steps and action SHA pins are valid.

## API bar-raiser

| Finding | Disposition | Commit |
|---|---|---|
| Public PARITY files and the fixture index named internal plan phases | Fixed: phase lines and the index column removed | `607053007`, `c8e237569` |
| `strands-rs/AGENTS.md` lacked most behaviour contracts and the architecture rules | Fixed: a behaviour contracts section | `607053007` |
| `harness-rs/AGENTS.md` lacked the harness error texts and byte-observable list | Fixed | `607053007` |
| Naming table row misaligned for the TypeScript `experimental` entry point | Fixed (and the row below it) | `607053007` |
| AGENTS.md overstated the scope of the root cross-SDK rules | Fixed: reworded | `607053007` |
| Lint table omitted the print and `dbg!` test allowances | Fixed | `607053007`, `0371b1577` |
| TESTING.md dropped "`Sync` where shared" | Fixed | `607053007` |
| Harness crate docs lacked the cargo features and panics sections | Fixed | `607053007` |
| Harness README example needed a non-default feature | Fixed: uses a Bedrock model id | `607053007` |
| Acronym rule and Phase 0 names not logged | Fixed: logged in the run's decision log | `049002279` |
| Toolchain action pin comment wording | Fixed: reworded | `049002279` |

## Parity validator

| Finding | Disposition | Commit |
|---|---|---|
| Two harness integration totals counted comment prose | Fixed: recounted every row with a comment-aware counter; only these two changed | `607053007` |
| No live checklist for `strands-ts/test/integ/`, and `docs/reviews/live.md` missing | Fixed: 58-row checklist with out-of-scope reasons; `live.md` files created | `607053007` |
| Out-of-scope cases inside in-scope spec files not recorded as gaps | Fixed: recorded, plus further cases found by a wider grep | `607053007` |
| Telemetry fixtures had no memory spans | Fixed: `telemetry/memory_run.default.json` | `c8e237569` |
| Base64 fixture couldn't distinguish the lenient-decoding rules; a decision cited a wrong file | Fixed: four more inputs; the duplicate decision withdrawn | `c8e237569`, `049002279` |
| Hand-written usage and metrics values not marked validity only | Fixed: built through TS where a code path exists; the rest moved to a validity-only fixture | `c8e237569` |
