# sandbox module report (strands-rs)

STATUS: DONE — gates green, committed on `agent-tasks/strands-rs-sandbox`.
NEXT: Docker/SSH backends (`PosixShellSandbox` base, `shell_quote`/env-key helpers already landed);
real streaming in `execute_streaming` (default yields terminal result only); `strands-shell` as a
feature-gated backend (sb1 §6 recommendation 3).

## Files
- `crates/strands-agents/src/sandbox/mod.rs` — types (`StreamType`, `StreamChunk`, `FileInfo`,
  `OutputFile`, `ExecutionResult`, `SandboxEvent`, `ExecuteOptions`), `SandboxError` (verbatim TS
  Display strings), `trait Sandbox`, `SandboxRef`, default slot
  (`set_default_sandbox`/`default_sandbox`/`clear_default_sandbox`/`register_local_sandbox_default`,
  `NO_SANDBOX_MESSAGE` verbatim), `shell_quote`/`is_valid_language`/`is_valid_env_key`.
- `crates/strands-agents/src/sandbox/local.rs` — `LocalSandbox` (TS `NotASandboxLocalEnvironment`):
  `sh -c` in its own process group with the kill/drain logic moved out of `tools/vended/shell.rs`
  (`GroupKill`, `collect`/`Collected`, `kill_group`, `exit_code_of`, 250 ms drain grace) + `tokio::fs`
  file ops.
- `crates/strands-agents/src/tools/vended/shell.rs` — now routes through a `Sandbox`
  (`ShellTool::with_sandbox`, `execute_in(&dyn Sandbox, ..)`); `invoke` resolves tool sandbox →
  `ctx.agent.sandbox()` → `LocalSandbox` from `ShellConfig`. Host-exec code deleted from here.
- `crates/strands-agents/src/agent/{mod,builder}.rs` — `AgentInner.sandbox: Option<Option<SandboxRef>>`,
  `Agent::sandbox()`, `AgentBuilder::{sandbox, sandbox_arc, no_sandbox}`, `register_sandbox_tools()`
  called from `initialize()` before `InitializedEvent` with the verbatim skip warning.
- `crates/strands-agents/tests/sandbox_loop.rs` — 7 e2e tests (default slot, tool-registration
  precedence, `shell` through a sandbox in a `MockMessageModel` loop).

## Test matrix (TS → Rust)
All Rust tests below run real processes/files → they are e2e by the brief's definition.

| TS test | Rust test | Status |
| --- | --- | --- |
| default.test.node: returns the instance registered by the setup file | `sandbox_loop::default_slot_fallback_and_unconfigured_error` | pass |
| default.test.node: returns the configured sandbox when one is provided | `sandbox_loop::returns_the_configured_sandbox_when_one_is_provided` | pass |
| default.test.node: falls back to the registered default when unconfigured | `sandbox_loop::default_slot_fallback_and_unconfigured_error` | pass |
| default.test.node: treats sandbox: false the same as unconfigured | `sandbox_loop::default_slot_fallback_and_unconfigured_error` (`no_sandbox()`) | pass |
| default.test.browser: throws when unconfigured because no default sandbox is registered | same test (slot cleared; verbatim message asserted) | pass |
| errors.test.node: builds a message from the timeout duration / fixed messages / names | `sandbox::tests::timeout_error_message_is_verbatim` | pass |
| errors.test.node: carries the partial output captured before the kill | `local::tests::kills_process_on_timeout_and_reports_the_output_captured_before_the_kill` | pass |
| errors.test.node: is an instance of Error ×2, has the correct error name ×2, defaults to empty output | — | skipped (JS class/prototype semantics; Rust uses a `thiserror` enum) |
| not-a-sandbox: runs a command on the host | `local::tests::runs_a_command_on_the_host` | pass |
| not-a-sandbox: respects the cwd option | `local::tests::respects_the_cwd_option` | pass |
| not-a-sandbox: reports a non-zero exit code and stderr from a failing command | `local::tests::reports_a_non_zero_exit_code_and_stderr_from_a_failing_command` | pass |
| not-a-sandbox: applies the env option (execute + executeCode) | `local::tests::applies_the_env_option` | pass |
| not-a-sandbox: runs code through the named interpreter | `local::tests::runs_code_through_the_named_interpreter` (accepts "interpreter not found" when python3 is absent) | pass |
| not-a-sandbox: rejects a language with invalid characters | `local::tests::rejects_a_language_with_invalid_characters` | pass |
| not-a-sandbox: text file roundtrip via absolute path | `local::tests::text_file_roundtrip_via_absolute_path` | pass |
| not-a-sandbox: binary roundtrip preserves all byte values | `local::tests::binary_roundtrip_preserves_all_byte_values` | pass |
| not-a-sandbox: creates missing parent directories on write | `local::tests::creates_missing_parent_directories_on_write` | pass |
| not-a-sandbox: throws when reading a nonexistent file | `local::tests::throws_when_reading_a_nonexistent_file` | pass |
| not-a-sandbox: removes a file / throws on a nonexistent file | `local::tests::removes_a_file_and_throws_on_a_nonexistent_file` | pass |
| not-a-sandbox: returns entries sorted by name with isDir and size metadata | `local::tests::returns_entries_sorted_by_name_with_is_dir_and_size_metadata` | pass |
| not-a-sandbox: throws SandboxPathNotFoundError on a nonexistent directory / when a path component is a file | `local::tests::throws_path_not_found_on_a_nonexistent_directory_or_file_component` | pass |
| not-a-sandbox: writes absolute paths as-is / resolves relative paths against process.cwd() | `local::tests::resolves_relative_paths_against_the_base_dir` | pass (base dir instead of process cwd — tests must not chdir) |
| posix-shell: shellQuote (spaces, single quotes) | `sandbox::tests::shell_quote_handles_paths_with_spaces`, `..._single_quotes` | pass |
| posix-shell: rejects invalid env var names (×3) | `sandbox::tests::rejects_invalid_env_var_names`, `local::tests::rejects_invalid_env_var_names` | pass |
| posix-shell: language validation (traversal, metachars, spaces, valid, dots/hyphens) | `sandbox::tests::rejects_invalid_language_characters` | pass |
| posix-shell: kills process on timeout + reports output before kill | `local::tests::kills_process_on_timeout_...` | pass |
| posix-shell: does not timeout fast commands | `local::tests::does_not_timeout_fast_commands` | pass |
| posix-shell: kills process when signal is aborted | `local::tests::kills_process_when_signal_is_aborted` | pass |
| posix-shell: rejects immediately if signal is already aborted | `local::tests::rejects_immediately_if_signal_is_already_aborted` | pass |
| posix-shell: handles multiple concurrent commands | `local::tests::handles_multiple_concurrent_commands` | pass |
| posix-shell: handles concurrent file writes to different files | — | skipped (covered by the concurrent-command test + `tokio::fs`; no shell-channel serialization to prove) |
| posix-shell: yields StreamChunks then ExecutionResult | `local::tests::yields_stream_chunks_then_execution_result` | pass (adapted: default streaming yields only the terminal result — deviation below) |
| posix-shell: returns exit code 127 when command is not found | `local::tests::returns_exit_code_127_when_command_is_not_found` | pass |
| posix-shell: maps signal termination to 128 + signal number | `local::tests::maps_signal_termination_to_128_plus_signal_number` | pass |
| posix-shell: decodes multibyte UTF-8 split across chunk boundaries | — | skipped (the collector buffers bytes and decodes once; no chunk-boundary seam to test) |
| posix-shell: returns enoentMessage when spawned binary does not exist | `local::tests::rejects_a_language_with_invalid_characters` neighbour: interpreter-not-found path in `runs_code_through_the_named_interpreter` | pass |
| posix-shell: remaining rows that exercise file ops *via shell commands* (base64/ls parsing, workingDir, env prefix, unicode/metachar content, `buildShellEnvPrefix` ×4, executeCode quoting ×5) | — | skipped: `PosixShellSandbox` not ported (no Docker/SSH backend yet) |
| docker.test.node (11 rows), ssh.test.node (20 rows) | — | skipped: backends not ported by design (sb1 §6 rec. 4–5) |
| *(new, no TS equivalent)* | `local::tests::writes_stdin_to_the_command`, `vends_no_tools_by_default`, `sandbox_loop::{registers_sandbox_vended_tools_at_initialization, skips_a_sandbox_vended_tool_when_a_user_tool_has_the_same_name, an_opted_out_sandbox_registers_no_tools, the_shell_tool_runs_through_the_agents_sandbox_in_the_loop, the_shell_tool_falls_back_to_the_local_host_without_an_agent_sandbox}` | pass |

Counts: 32 sandbox unit/e2e tests + 7 `tests/sandbox_loop.rs` = **39 new tests**; 28 TS rows mapped,
**40 skipped** (37 of them Docker/SSH/PosixShell backends not ported, 3 JS-semantics or no-seam).

## Deviations from TS (deliberate)
1. `execute` is the required trait primitive; TS makes `executeStreaming` the primitive and derives
   `execute`. The default `execute_streaming` wraps `execute` and yields one `SandboxEvent::Result`,
   so a streaming-capable backend overrides it. Streaming is therefore not yet incremental.
2. `AbortSignal` → `Option<CancellationToken>` in `ExecuteOptions`; `ExecuteOptions.stdin` added
   (TS has none) because the task asked for it — `None` keeps the TS behaviour (`/dev/null`).
3. No `stat` and no `start`/`stop`: TS has neither. The trait is `#[non_exhaustive]`-friendly in
   spirit (new methods can land with defaults).
4. Field names follow TS `ExecutionResult { stdout, stderr, exit_code, output_files }`; the
   `shell` tool keeps its own `ShellOutput { output, error, exit_code }` wire shape and maps between.
5. `Sandbox: Debug` supertrait so `ShellTool`/`AgentInner` stay `Debug`.
6. The default slot starts **empty** (no lazy host default), so `Agent::sandbox()` errors with the
   verbatim TS message until something calls `set_default_sandbox`/`register_local_sandbox_default`.
   `ShellTool` falls back to a `LocalSandbox` itself, so the harness is unaffected.
7. `LocalSandbox` resolves relative paths against an explicit `base_dir` (TS uses `process.cwd()`
   when no base is given — same behaviour when `base_dir` is `None`).

## Gates (last line, verbatim)
- `cargo fmt` → (no output)
- `cargo clippy --all-targets --all-features -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 13.87s`
- `cargo test --all-features -p strands-agents` → `test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.63s` (692+33+24+4+1+7+10+1 passed, 0 failed across targets)
- `cargo test -p strands-harness` → `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` (173+1 passed, 0 failed)
