# h12ga divergence rows (for `harness-rs/docs/DIVERGENCES.md`)

Replaces the row whose first column is `` `effort` outside the enum `` (api-3, parity-3):

| `effort` outside the enum | `resolveModel` throws `Effort "x" is not valid. Supported: ...` | `Effort` is an enum, so `create_harness` cannot be given such a value. A name is parsed with `"x".parse::<Effort>()` (`FromStr`), which returns `HarnessError::InvalidEffort` with the same text; a config's `effort` is reported as `effort must be one of: ...`, as the TS config schema does | typed API (P-833) |
