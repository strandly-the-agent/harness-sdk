# Workspace lints

Every crate in the Cargo workspace inherits one lint set from `[workspace.lints]` in the root `Cargo.toml` (`[lints] workspace = true` in each crate manifest). CI builds with `-D warnings`, so anything at `warn` fails CI too. Lints at `deny` also fail local builds.

## Rust lints

| Lint | Level | Why |
|---|---|---|
| `unsafe_code` | forbid | The SDK has no reason to use `unsafe`, and `forbid` can't be overridden further down. |
| `missing_docs` | warn | Every public item carries rustdoc. Kept at `warn` so a half-written item still builds locally; CI denies it. |

## Clippy lints

`clippy::all` (the default correctness, suspicious, style, complexity and perf groups) is on at `warn`. On top of it, these are denied:

| Lint | Why |
|---|---|
| `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable` | Library code returns errors instead of panicking. |
| `exit` | A library never ends the host process. |
| `dbg_macro`, `print_stdout`, `print_stderr` | Output goes through `tracing` or through the agent's printer, never straight to the terminal. The printer and the examples opt out with `#[expect(clippy::print_stdout, reason = "...")]`. |
| `allow_attributes_without_reason` | Every lint exception says why. Prefer `#[expect(..., reason = "...")]`, which also fails once the exception is no longer needed. |
| `await_holding_lock`, `await_holding_refcell_ref`, `await_holding_invalid_type` | No lock or `RefCell` borrow is held across `.await`. |
| `needless_pass_by_value` | Take a reference when the callee doesn't need ownership, so callers don't clone. |
| `redundant_clone` | Catch clones whose source is never used again. |

## Tests

`clippy.toml` sets `allow-unwrap-in-tests`, `allow-expect-in-tests` and `allow-panic-in-tests`, so `unwrap`, `expect` and `panic!` are fine inside `#[test]` functions and `#[cfg(test)]` modules. Helper functions in integration test files (`tests/*.rs`) are outside both, so they either return `Result` or carry a scoped `#[expect(clippy::unwrap_used, reason = "...")]`.

## Not enabled

The rest of `clippy::pedantic`, `clippy::nursery` and `clippy::restriction` stays off. Lints that flag unused crate dependencies (`unused_crate_dependencies`) are also off: the manifests declare dependencies ahead of the modules that use them, and feature-gated modules make the lint noisy.
