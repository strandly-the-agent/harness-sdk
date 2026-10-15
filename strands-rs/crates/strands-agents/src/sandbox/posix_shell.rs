//! POSIX-shell building blocks shared by the shell-backed sandboxes
//! (`strands-ts/src/sandbox/posix-shell.ts`).
//!
//! Rust has no abstract base class, so TS's `PosixShellSandbox` methods are free functions over
//! `&dyn Sandbox`: a backend that can only run shell commands implements [`Sandbox::execute`] and
//! forwards code and file operations here. [`DockerSandbox`](super::DockerSandbox) and
//! [`SshSandbox`](super::SshSandbox) do exactly that, and so can a third-party backend.
//!
//! Two divergences from TS, both because `HashMap` has no insertion order and a shell command
//! line needs a deterministic one: environment variables are emitted sorted by name, and an
//! invalid name is reported in sorted order. A missing path is reported as
//! [`SandboxError::PathNotFound`] for reads and removals as well as listings (TS only does it for
//! listings), so the variant means the same thing on every backend.

use std::collections::HashMap;

use base64::Engine as _;

use super::{ExecuteOptions, ExecutionResult, FileInfo, Sandbox, SandboxError, SandboxResult};

/// Exit code the guard commands use to flag a missing path, chosen to not collide with the real
/// command's own failures (TS uses the same number).
const MISSING_PATH_EXIT_CODE: i32 = 77;

const BASE64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Check every name in `env` is a valid POSIX environment variable name (TS `validateEnvKeys`).
pub fn validate_env_keys(env: &HashMap<String, String>) -> SandboxResult<()> {
    sorted_env(env).map(|_| ())
}

/// Build a `export KEY='value' && ` prefix for a shell command, or `""` when `env` is empty
/// (TS `buildShellEnvPrefix`).
///
/// `export` rather than an `env KEY=VALUE` wrapper so the variables are set in the shell itself
/// and inherited by every stage of a pipeline — [`execute_code`] pipes `base64 -d` into the
/// interpreter, and an `env` wrapper would only bind the left side of the pipe. The trailing
/// `&&` keeps the surrounding `cd … && <prefix><command>` chain fail-fast.
pub fn shell_env_prefix(env: &HashMap<String, String>) -> SandboxResult<String> {
    if env.is_empty() {
        return Ok(String::new());
    }
    let assignments: Vec<String> = sorted_env(env)?
        .into_iter()
        .map(|(key, value)| format!("{key}={}", super::shell_quote(value)))
        .collect();
    Ok(format!("export {} && ", assignments.join(" ")))
}

/// Run `code` through the `language` interpreter by piping it in base64-encoded
/// (TS `PosixShellSandbox.executeCodeStreaming`).
///
/// The code never reaches the shell as syntax, so quoting cannot break on it.
pub async fn execute_code(
    sandbox: &dyn Sandbox,
    code: &str,
    language: &str,
    options: ExecuteOptions,
) -> SandboxResult<ExecutionResult> {
    if !super::is_valid_language(language) {
        return Err(SandboxError::Other(format!(
            "language parameter contains invalid characters: {language}"
        )));
    }
    let eof = eof_marker();
    let encoded = BASE64.encode(code);
    sandbox
        .execute(
            &format!("base64 -d << '{eof}' | {language}\n{encoded}\n{eof}"),
            options,
        )
        .await
}

/// Read a file's raw bytes by base64-encoding it on the target.
pub async fn read_file(sandbox: &dyn Sandbox, path: &str) -> SandboxResult<Vec<u8>> {
    let quoted = super::shell_quote(path);
    let result = sandbox
        .execute(
            &format!("test -e {quoted} || exit {MISSING_PATH_EXIT_CODE}; base64 < {quoted}"),
            ExecuteOptions::default(),
        )
        .await?;
    check(&result, path, "Failed to read file")?;
    BASE64
        .decode(result.stdout.split_whitespace().collect::<String>())
        .map_err(|_| SandboxError::Other(format!("Failed to decode base64 contents of file: {path}")))
}

/// Write `content` to `path`, creating missing parent directories, by piping it in base64-encoded.
pub async fn write_file(sandbox: &dyn Sandbox, path: &str, content: &[u8]) -> SandboxResult<()> {
    let quoted = super::shell_quote(path);
    let eof = eof_marker();
    let encoded = BASE64.encode(content);
    let command =
        format!("mkdir -p \"$(dirname {quoted})\" && base64 -d << '{eof}' > {quoted}\n{encoded}\n{eof}");
    let result = sandbox.execute(&command, ExecuteOptions::default()).await?;
    check(&result, path, "Failed to write file")
}

/// Delete a file with `rm`.
pub async fn remove_file(sandbox: &dyn Sandbox, path: &str) -> SandboxResult<()> {
    let quoted = super::shell_quote(path);
    let result = sandbox
        .execute(
            &format!("test -e {quoted} || exit {MISSING_PATH_EXIT_CODE}; rm {quoted}"),
            ExecuteOptions::default(),
        )
        .await?;
    check(&result, path, "Failed to remove file")
}

/// List a directory, sorted by name, by parsing `ls -1ap`.
///
/// [`FileInfo::size`] is always `None`: `ls -1ap` reports the trailing `/` that marks a directory
/// but no sizes, and a second command per entry is not worth it.
pub async fn list_files(sandbox: &dyn Sandbox, path: &str) -> SandboxResult<Vec<FileInfo>> {
    let quoted = super::shell_quote(path);
    let result = sandbox
        .execute(
            &format!(
                "test -d {quoted} || exit {MISSING_PATH_EXIT_CODE}; env QUOTING_STYLE=literal ls -1ap {quoted}"
            ),
            ExecuteOptions::default(),
        )
        .await?;
    check(&result, path, "Failed to list directory")?;
    Ok(parse_listing(&result.stdout))
}

/// Environment entries sorted by name, with every name validated.
pub(super) fn sorted_env(env: &HashMap<String, String>) -> SandboxResult<Vec<(&str, &str)>> {
    let mut entries: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    entries.sort_unstable();
    for (key, _) in &entries {
        if !super::is_valid_env_key(key) {
            return Err(SandboxError::Other(format!(
                "Invalid environment variable name: {key}"
            )));
        }
    }
    Ok(entries)
}

/// `extra` layered over `base`, so a per-call variable wins over a sandbox-wide one.
pub(super) fn merge_env(
    base: Option<&HashMap<String, String>>,
    extra: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let mut merged = base.cloned().unwrap_or_default();
    merged.extend(extra.into_iter().flatten().map(|(k, v)| (k.clone(), v.clone())));
    merged
}

/// A heredoc delimiter that cannot collide with the payload (TS `STRANDS_EOF_<uuid>`).
fn eof_marker() -> String {
    format!("STRANDS_EOF_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

fn check(result: &ExecutionResult, path: &str, what: &str) -> SandboxResult<()> {
    match result.exit_code {
        0 => Ok(()),
        MISSING_PATH_EXIT_CODE => Err(SandboxError::PathNotFound(path.to_owned())),
        _ if result.stderr.is_empty() => Err(SandboxError::Other(format!("{what}: {path}"))),
        _ => Err(SandboxError::Other(result.stderr.clone())),
    }
}

fn parse_listing(stdout: &str) -> Vec<FileInfo> {
    let mut entries: Vec<FileInfo> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line == "./" || line == "../" {
                return None;
            }
            let (name, is_dir) = match line.strip_suffix('/') {
                Some(name) => (name, true),
                None => (line, false),
            };
            (!name.is_empty()).then(|| FileInfo {
                name: name.to_owned(),
                is_dir: Some(is_dir),
                size: None,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn returns_an_empty_string_when_there_are_no_env_vars() {
        assert_eq!(shell_env_prefix(&HashMap::new()).unwrap(), "");
    }

    #[test]
    fn uses_export_so_vars_are_inherited_across_a_pipeline_with_a_fail_fast_separator() {
        assert_eq!(
            shell_env_prefix(&env(&[("FOO", "bar"), ("BAZ", "qux")])).unwrap(),
            "export BAZ='qux' FOO='bar' && "
        );
    }

    #[test]
    fn shell_quotes_values_so_they_are_not_evaluated() {
        assert_eq!(
            shell_env_prefix(&env(&[("FOO", "$(whoami)")])).unwrap(),
            "export FOO='$(whoami)' && "
        );
    }

    #[test]
    fn errors_on_invalid_env_var_names() {
        let error = shell_env_prefix(&env(&[("BAD-KEY", "v")])).unwrap_err();
        assert_eq!(error.to_string(), "Invalid environment variable name: BAD-KEY");
        assert!(validate_env_keys(&env(&[("GOOD", "v")])).is_ok());
    }

    #[test]
    fn layers_per_call_env_over_the_sandbox_env() {
        let base = env(&[("KEEP", "base"), ("OVERRIDE", "base")]);
        let extra = env(&[("OVERRIDE", "call")]);
        let merged = merge_env(Some(&base), Some(&extra));
        assert_eq!(merged["KEEP"], "base");
        assert_eq!(merged["OVERRIDE"], "call");
        assert!(merge_env(None, None).is_empty());
    }

    #[test]
    fn parses_an_ls_listing_excluding_dot_entries_and_marking_directories() {
        let entries = parse_listing("./\n../\nb.txt\nsubdir/\na.txt\r\n\n");
        assert_eq!(
            entries,
            vec![
                FileInfo {
                    name: "a.txt".to_owned(),
                    is_dir: Some(false),
                    size: None
                },
                FileInfo {
                    name: "b.txt".to_owned(),
                    is_dir: Some(false),
                    size: None
                },
                FileInfo {
                    name: "subdir".to_owned(),
                    is_dir: Some(true),
                    size: None
                },
            ]
        );
    }

    #[test]
    fn maps_the_missing_path_exit_code_onto_path_not_found() {
        let missing = ExecutionResult {
            exit_code: MISSING_PATH_EXIT_CODE,
            ..ExecutionResult::default()
        };
        assert!(matches!(
            check(&missing, "/nope", "Failed to read file"),
            Err(SandboxError::PathNotFound(path)) if path == "/nope"
        ));

        let failed = ExecutionResult {
            exit_code: 1,
            stderr: "rm: permission denied\n".to_owned(),
            ..ExecutionResult::default()
        };
        assert_eq!(
            check(&failed, "/x", "Failed to remove file")
                .unwrap_err()
                .to_string(),
            "rm: permission denied\n"
        );

        let silent = ExecutionResult {
            exit_code: 1,
            ..ExecutionResult::default()
        };
        assert_eq!(
            check(&silent, "/x", "Failed to remove file")
                .unwrap_err()
                .to_string(),
            "Failed to remove file: /x"
        );
    }

    #[test]
    fn eof_markers_are_unique() {
        assert_ne!(eof_marker(), eof_marker());
        assert!(eof_marker().starts_with("STRANDS_EOF_"));
        assert_eq!(eof_marker().len(), "STRANDS_EOF_".len() + 16);
    }
}
