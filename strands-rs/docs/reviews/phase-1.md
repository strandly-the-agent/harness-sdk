# Phase 1 review: types, wire and errors

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed the message and content-block types, media, citations, tool types, stop reasons, usage and metrics, stream event records, interrupt and snapshot types, `strands::Error`, and the conformance round-trip tests at `8711e79cd`. Because the citation fixes were substantial, a second correctness pass reviewed the fix diff at `d996727c5`, and its findings were fixed too. After all fixes, the full gates (fmt, clippy with all features and with no default features, tests both ways, rustdoc with `-D warnings`, the 1.96.0 MSRV check and `cargo deny check`) pass at `6b89c5dd6` with 573 tests.

## Correctness

| Finding | Disposition | Commit |
|---|---|---|
| Citations from the TS Bedrock adapter (two key orders) were not written back in their loaded key order | Fixed: citations keep their loaded key order; new fixture `types/bedrock_citations.json` from the real TS Bedrock adapter | `1a5032b13`..`888f0aab0` |
| A known location `type` with misfit fields landed in `CitationLocation::Other`, contrary to the docs | Kept, matching TS (TS writes such locations back unchanged); docs corrected | `1a5032b13`..`888f0aab0` |
| `UnknownBlock::new` accepted a known key, so the block changed type on a round trip | Fixed: returns `None` for data with a known block or tool-result-content key | `1a5032b13`..`888f0aab0` |
| A present `redactedContent: null` failed the whole block | Fixed: read as absent and dropped on write, as TS does | `1a5032b13`..`888f0aab0` |
| `interrupt.source: null` failed to load | Fixed: loads as `hook`; `InterruptStateData.context: null` loads as empty | `1a5032b13`..`888f0aab0` |

## Correctness re-review of the fixes

| Finding | Disposition | Commit |
|---|---|---|
| A known location type with an extra key, or `"domain": null`, lost data on save | Fixed: such locations load as `Other` and are written back as read | `489f35949` |
| Unknown keys on a citation and its content items were dropped; `null` or missing fields failed the message | Fixed: unknown keys kept in place; `null` and missing fields kept as written | `489f35949` |
| Remaining citation differences were not in `DIVERGENCES.md` | Fixed: a citations row | `6b89c5dd6` |

## API bar-raiser

| Finding | Disposition | Commit |
|---|---|---|
| Struct-like variants of `CitationLocation` and `ToolChoice` could be built and matched field by field, so new fields would break callers | Fixed: variants are `#[non_exhaustive]`; `CitationLocation` constructors added | `d996727c5` |
| `DataError` had no public constructor | Fixed: `DataError::new` is public | `d996727c5` |

## Parity validator

| Finding | Disposition | Commit |
|---|---|---|
| `messages.test.ts` count should include the 20-entry round-trip table | Fixed: 105/105 | `1a5032b13`..`888f0aab0` |
| `mime.test.ts` count should include both mapping tables | Fixed: 52/52 | `1a5032b13`..`888f0aab0` |
| `interrupt.test.ts` row lacked gap reasons for the `InterruptError` cases | Fixed: listed as a TS-only mechanism (interrupts are not errors) | `1a5032b13`..`888f0aab0` |
| `schemas/tool_specs.json` was deferred although its type exists | Fixed: covered by `tool_specs_round_trip` | `1a5032b13`..`888f0aab0` |
| `saved_data_round_trip.json` compared values, not bytes | Fixed: compares bytes | `1a5032b13`..`888f0aab0` |
