# Phase 1 review: types, wire and errors

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed `d2cce53e2..0e4b20905`. All gates passed at review time.

| Finding | Disposition | Commit |
|---|---|---|
| Correctness 1: JS number formatting rounds exact ties half up where `JSON.stringify` rounds half to even | Fixed: `ryu-js` formatting, tie tests | `1c28bb4c6` |
| Correctness 2: data deeper than serde_json's 128-level limit can't be read back, including snapshots Rust wrote itself | Fixed: the saved-data reader has no depth limit (`serde_stacker`); `deep_nesting.json`, 200-level snapshot and 1000-level tests | `1c28bb4c6` |
| Correctness 3: `Error::Other` drops the source chain | Fixed: `#[error(transparent)]`, test | `059d74dd5` |
| Correctness 4: a present `null` in metrics, interrupt `source` or interrupt-state `context` fails the whole load | Fixed: treated as missing, as TS does; tests | `6f2593445` |
| Correctness 5: a known citation location type with a missing field fails the whole message | Fixed: falls back to `Other` with the raw object | `1352e7fc3` |
| Correctness 6: `UnknownBlock::new` can hold a known block that reloads as that block | Fixed: constructor is crate-internal; TS constructor hole recorded | `d5edc8d1c` |
| API 1: `generate_tracking_id` public while TS marks it internal | Fixed: `pub(crate)` | `9bd205060` |
| API 2: `Usage::total_prompt_tokens` public while TS marks it internal | Fixed: `pub(crate)` | `9bd205060` |
| API 3: `mime` module exposes TS-internal helpers | Fixed: module is crate-internal; the four format enums (and their unknown-value payload types) stay public through `types::media` | `9bd205060` |
| API 4: struct-shaped enum variants with public fields lock their field sets | Fixed: variants are `#[non_exhaustive]` with constructors | `5daa10b76` |
| API 5: `Error::Other` cuts the source chain | Fixed (same as correctness 3) | `059d74dd5` |
| API 6: `InterruptParams` has no builder | Fixed: `bon` builder | `309167929` |
| API 7: no doctests | Fixed: crate root, `StopReason`, `Error` and `ToolResultBlock` examples | `ec04f0ba4` |
| API 8: media source variant names missing from the name map | Fixed: logged with their TS discriminants | `9adffca7b` |
| Parity 1: `deep_nesting.json` unread and unloadable | Fixed (same as correctness 2) | `1c28bb4c6` |
| Parity 2: `JsonBlock` row missing from the round-trip table test | Fixed | `52f23487e` |
| Parity 3: `agent.test.ts` gap reason wrong for the checkpoint cases | Fixed | `fb17f1bc4` |
| Parity 4: the conformance gate fails without a clear message when Node dependencies are missing | Fixed: explicit precheck | `9adffca7b` |
