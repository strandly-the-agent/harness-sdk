# shell tool (`tools::vended::ShellTool`) — strands-rs port

Branch `agent-tasks/strands-rs-bash`, worktree `/home/sandbox/wt/bash`.
Files added: `crates/strands-agents/src/tools/vended/mod.rs`, `crates/strands-agents/src/tools/vended/shell.rs`.
Files touched: `crates/strands-agents/src/tools/mod.rs` (+`pub mod vended;`), `crates/strands-agents/src/lib.rs`
(+`pub use tools::vended::ShellTool;`), workspace `Cargo.toml` (tokio `process` feature).

Source of truth: `strands-ts/src/vended-tools/shell/{make-shell.ts,types.ts}`,
`strands-ts/src/sandbox/not-a-sandbox-local-environment.ts` (host executor: `sh -c`, cwd, env prefix),
`strands-ts/src/sandbox/{errors.ts,stream-process.ts}`, tests
`strands-ts/src/vended-tools/shell/__tests__/shell.test.node.ts`, `harness-ts/test/tools/shell.test.ts`.

## Test matrix (14 ported, 6 skipped)

| TS test (file) | Rust test | Status |
|---|---|---|
| `executes command via sandbox` (shell.test.node) | `executes_command_via_sandbox` | pass |
| `reports a non-zero exit code` | `reports_a_non_zero_exit_code` | pass |
| `captures stderr via sandbox` | `captures_stderr_via_sandbox` | pass |
| `does not persist state between calls (stateless)` | `does_not_persist_state_between_calls_stateless` | pass |
| `respects timeout` | `respects_timeout` | pass |
| `throws ShellTimeoutError on timeout` | `throws_shell_timeout_error_on_timeout` | pass |
| `timeout error carries the partial output with the same field names as a success result` | `timeout_error_carries_the_partial_output_with_the_same_field_names_as_a_success_result` | pass |
| `timeout surfaces as a failed tool result that still includes the partial output` | `timeout_surfaces_as_a_failed_tool_result_that_still_includes_the_partial_output` | pass |
| `timeout error still matches the pre-rename BashTimeoutError` | — | skipped: TS class-inheritance compat (`ShellTimeoutError extends BashTimeoutError`); Rust has one `ShellError` enum |
| `ShellExecutionError still matches the pre-rename BashSessionError` | — | skipped: same reason |
| `keeps the pre-rename tool name` (makeBash alias) | — | skipped: deprecated `makeBash` alias (removed in TS v2.0.0) not ported |
| `matches makeShell apart from the name` | — | skipped: same |
| `an explicit name still wins` | — | skipped: same; `ShellConfig` has no name/description override (per task spec) |
| `still exposes every shell name` (pre-rename bash import path) | — | skipped: TS module-barrel re-export test, no Rust analogue |
| `exposes only command and timeout, with no mode discriminator` (harness shell.test.ts) | `exposes_only_command_and_timeout_with_no_mode_discriminator` | pass |
| `runs a command through the agent sandbox` (harness) | covered by `executes_command_via_sandbox` | pass |
| `is stateless: state does not persist across calls` (harness) | covered by `does_not_persist_state_between_calls_stateless` | pass |
| — (new) | `spec_matches_the_ts_name_and_description` | pass |
| — (new) | `defaults_to_a_120_second_timeout` | pass |
| — (new) | `runs_in_the_configured_cwd_with_the_configured_env` | pass |
| — (new) | `cancellation_aborts_the_command` | pass |
| — (new) | `rejects_a_non_positive_timeout` | pass |

## Behaviour / deviations

- **Name/description/schema verbatim.** `shell`, `SANDBOX_SHELL_DESCRIPTION` byte-identical;
  schema `{command: string (required), timeout: number exclusiveMinimum 0}`, `additionalProperties: false`.
- **Result shape.** Success → `ShellOutput { output, error, exit_code }` serialized as a JSON
  tool-result block via `wrap_in_tool_result` (TS returns the same object from the tool callback).
- **Timeout.** Default 120s (`ShellConfig::timeout`), per-call override from `input.timeout`. On
  expiry the child is killed and the error is
  `Execution timed out after <s> seconds\n{"output":…,"error":…,"exit_code":124}` — the TS format
  exactly (partial output JSON-encoded on line 2); `invoke` surfaces it as a `status: error`
  result whose text is `Error: <that message>`.
- **Host execution instead of a sandbox.** No `Sandbox` abstraction exists in strands-rs, so
  `ShellTool` is the host executor: `tokio::process::Command::new("sh").arg("-c")` with
  `stdin` null, piped stdout/stderr, `kill_on_drop`. TS's local sandbox builds
  `sh -c "cd <cwd> && VAR=v … <cmd>"`; the Rust port sets `current_dir`/`envs` on the command
  instead — same observable effect (env vars layered on the inherited environment), no quoting
  hazard. `sh`, not `bash`: the TS shell tool is explicitly shell-agnostic (`sh` locally/Docker).
- **Exit codes.** `status.code()`, else `128 + signal` on Unix, else 1. TS uses a
  `SIGNAL_EXIT_CODES` lookup in `stream-process.ts` with the same values for the signals it maps.
- **No output cap.** TS truncates nothing (`rg 'truncat|maxOutput'` only hits `web-fetch`); the
  port matches — a chatty command returns an unbounded string. Documented on `ShellTool`.
- **No `_background_execution`.** `makeShell` has no such option; backgrounding lives in
  `background-tasks/` and injects itself into any tool's schema. Nothing added here.
- **Cancellation.** `ctx.cancel` is raced against execution (`tokio::select!`, biased); on cancel
  the child future is dropped (`kill_on_drop`) and `Err(Error::Cancelled)` is returned, matching
  the crate convention that `Err` is reserved for control flow.
- **Positive-timeout validation.** Zod's `.positive()` rejects `timeout <= 0` before the callback
  runs; Rust returns a `status: error` result (non-finite values rejected too).
- Partial output on timeout requires incremental capture, so stdout/stderr are drained by
  background tasks into shared buffers rather than `output().await`.

## Nothing stubbed

No dependencies on other in-flight modules. Only new dep surface: tokio's `process` feature.

## Gate output (last line each)

```
$ cargo fmt
(no output)
$ cargo clippy --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized] target(s) in 11.17s
$ cargo test --all-features shell
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 550 filtered out; finished in 15.01s
$ cargo test --all-features
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
  (lib: 564 passed; 0 failed — all suites ok)
```
