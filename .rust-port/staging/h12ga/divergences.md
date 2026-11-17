# h12ga divergence rows (for `harness-rs/docs/DIVERGENCES.md`)

Replaces the row whose first column is `` `effort` outside the enum `` (api-3, parity-3):

| `effort` outside the enum | `resolveModel` throws `Effort "x" is not valid. Supported: ...` | `Effort` is an enum, so `create_harness` cannot be given such a value. A name is parsed with `"x".parse::<Effort>()` (`FromStr`), which returns `HarnessError::InvalidEffort` with the same text; a config's `effort` is reported as `effort must be one of: ...`, as the TS config schema does | typed API (P-833) |

New row (api-20, P-1153):

| `builtinTools.programmatic_tool_caller.allowedTools: null` in a normalized config | `normalizeHarnessAgentConfig` keeps the explicit `null` in its output | `ProgrammaticToolCallerConfig::allowed_tools` is `Option<Vec<String>>`: `null` reads as `None` and is not written back. `timeout: null` is kept (`Some(None)`, no bound). A whole number of seconds is written as an integer, a fractional one as a float | `null` and absent both mean every tool in both SDKs; the Rust type does not carry the JSON encoding (section 7.3, P-1153) |
