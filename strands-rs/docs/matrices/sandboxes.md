# sandboxes (Docker + SSH) — port matrix (strands-rs)

STATUS: DONE — `cargo check`/`clippy --all-features --tests` clean, `cargo test --lib sandbox::`
87 passed, `cargo test --test sandbox_loop` 8 passed, `cargo test --test integ_sandboxes -- --ignored`
4 passed (all skipped-with-message: no docker/sshd in the sandbox).

Follows `docs/matrices/sandbox.md`, which landed the `Sandbox` trait + `LocalSandbox` and listed the
Docker/SSH backends and the `PosixShellSandbox` base as not ported. This is that work.

## Files
- `src/sandbox/posix_shell.rs` — the TS `PosixShellSandbox` base class as **free functions** over
  `&dyn Sandbox`: `validate_env_keys`, `shell_env_prefix` (TS `buildShellEnvPrefix`), `execute_code`,
  `read_file`, `write_file`, `remove_file`, `list_files`, plus `pub(super)` `sorted_env`/`merge_env`.
- `src/sandbox/stream_process.rs` — TS `streamProcess`: `Spawn` builder + `run`/`stream` (process-group
  kill, UTF-8 chunk-boundary decoding, 128+signal exit codes, ENOENT message injection).
- `src/sandbox/docker.rs` — `DockerSandbox` (`docker exec`).
- `src/sandbox/ssh.rs` — `SshSandbox` (`ssh`), incl. the `ALLOWED_SSH_OPTIONS` allowlist.
- `tests/integ_sandboxes.rs` — live tests, `#[ignore]`, env-gated.

## TS API → Rust API

| TS (`sandbox/docker.ts`, `sandbox/ssh.ts`) | Rust |
| --- | --- |
| `new DockerSandbox({ container, workingDir, user, env })` | `DockerSandbox::new(container)` + `.with_working_dir/.with_user/.with_env` |
| — (not in TS) | `DockerSandbox::with_docker_binary` — test seam / `podman` |
| `sandbox.container`, `sandbox.workingDir` | `container()`, `working_dir() -> Option<&str>` |
| `new SshSandbox({ host, workingDir })` | `SshSandbox::new(host, working_dir)` — both required, as in TS |
| `{ port, identityFile, allowUnknownHosts, sshOptions, allowUnsafeSshOptions }` | `.with_port/.with_identity_file/.with_allow_unknown_hosts/.with_ssh_options/.with_allow_unsafe_ssh_options` |
| — (not in TS) | `SshSandbox::with_env` (sibling of `DockerSandbox::with_env`), `with_ssh_binary` |
| `sandbox.host`, `sandbox.workingDir` | `host()`, `working_dir() -> &str` |
| `ALLOWED_SSH_OPTIONS` (module-private) | `ssh.rs` `ALLOWED_SSH_OPTIONS` (module-private, same 34 names) |
| `executeStreaming` | `Sandbox::execute` (primitive) + `execute_streaming` |
| `PosixShellSandbox.{executeCodeStreaming,readFile,writeFile,removeFile,listFiles}` | `sandbox::posix_shell::{execute_code,read_file,write_file,remove_file,list_files}` |
| `buildShellEnvPrefix`, `validateEnvKeys` | `posix_shell::{shell_env_prefix,validate_env_keys}` |
| `getTools()` → `[sandbox_file_editor, sandbox_shell]` | `tools()` → `[ShellTool]` only (see divergences) |

## TS test → Rust test

`ssh.test.node.ts` (20 `it`s) → `src/sandbox/ssh.rs` `mod tests`:

| TS `it(...)` | Rust test |
| --- | --- |
| stores host and workingDir | `stores_host_and_working_dir` |
| permits known-safe options | `permits_known_safe_options` |
| rejects ProxyCommand | `rejects_proxy_command` |
| rejects Include | `rejects_include` |
| rejects Match | `rejects_match` |
| rejects LocalCommand | `rejects_local_command` |
| rejects forwarding directives | `rejects_forwarding_directives` |
| is case-insensitive | `option_allowlist_is_case_insensitive` |
| allowUnsafeSshOptions=true bypasses validation | `allow_unsafe_ssh_options_bypasses_validation` |
| builds correct SSH args with defaults | `builds_ssh_args_with_defaults` |
| uses StrictHostKeyChecking=no when allowUnknownHosts is true | `uses_strict_host_key_checking_no_when_allow_unknown_hosts_is_set` |
| includes identity file when provided | `includes_identity_file_when_provided` |
| uses custom port | `uses_custom_port` |
| appends user sshOptions as -o flags | `permits_known_safe_options` (asserts the `-o` run) |
| quotes cwd with single quotes | `quotes_cwd_with_single_quotes` |
| uses cwd option when provided | `uses_cwd_option_when_provided` |
| forwards timeout and signal to streamProcess | `times_out_a_slow_ssh_and_keeps_the_partial_output` (no mock: a real slow fake binary) |
| prefixes command with env vars when provided | `prefixes_command_with_env_vars_when_provided` |
| rejects invalid env var names | `rejects_invalid_env_var_names`, `surfaces_an_invalid_env_var_name_from_execute_and_execute_streaming` |
| vends the sandbox-routed fileEditor and bash tools / vends bash with the sandbox description | `vends_a_shell_tool` (partial — no `FileEditorTool` in Rust yet) |

Rust-only, no TS counterpart: `terminates_flags_with_dashdash_so_a_dash_prefixed_host_is_not_parsed_as_a_flag`
(TS asserts this only via the argv snapshots), `layers_a_per_call_env_var_over_the_sandbox_env`
(`with_env` has no TS equivalent), `reports_a_missing_ssh_cli_as_exit_127`,
`runs_the_constructed_argv_through_the_ssh_binary`,
`streams_stdout_chunks_that_add_up_to_the_terminal_result`,
`round_trips_files_and_code_through_the_shared_posix_shell_layer`.

`docker.test.node.ts` (11 `it`s) → `src/sandbox/docker.rs` `mod tests`; `posix-shell.test.node.ts`
rows that `sandbox.md` skipped ("`PosixShellSandbox` not ported") are now covered by
`posix_shell.rs`/`stream_process.rs` unit tests and the `local_exec` fake-binary round-trips.

## Divergences
- **No abstract base class.** TS `PosixShellSandbox` → free functions in `sandbox::posix_shell`
  taking `&dyn Sandbox`; each backend's trait impl forwards to them. Rust has no inherited
  behaviour, and the alternative (a default-method trait) would put shell-specific internals on the
  public `Sandbox` trait that `LocalSandbox` does not use.
- **Env vars are emitted sorted by name** (and invalid names reported sorted): `HashMap` has no
  insertion order and the command line must be deterministic. So TS's
  `export FOO='bar' BAZ='qux'` becomes `export BAZ='qux' FOO='bar'`.
- **SSH option allowlist is enforced when the argv is built**, not in the constructor: Rust
  builders do not fail. No connection is made with an unvetted option either way, only the timing
  of the error differs. Message: `SSH option "X" is not allowed. Set allow_unsafe_ssh_options to bypass.`
- **`SshSandbox::with_env` / `DockerSandbox::with_env` have no TS equivalent** (TS takes env
  per call only); a per-call `ExecuteOptions::env` entry wins over an instance one.
- **`tools()` vends only `ShellTool`**; TS also vends `makeFileEditor` as `sandbox_file_editor`.
  **Follow-up:** there is no Rust `FileEditorTool` yet. When one lands, add it to `DockerSandbox`,
  `SshSandbox` (and decide whether `LocalSandbox`, which vends nothing, should too) and update
  `vends_a_shell_tool` + the two TS `getTools` rows above.
- `SandboxError::PathNotFound` also covers reads/removals (TS raises it only for listings).
- No ssh/docker *library* divergence: TS and Python also shell out to the `ssh`/`docker` CLIs.
