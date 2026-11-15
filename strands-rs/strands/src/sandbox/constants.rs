//! Patterns and quoting shared by sandbox backends (TS `sandbox/constants.ts`).

/// The pattern interpreter names must match (TS `LANGUAGE_PATTERN`): letters, digits, `.`, `-`
/// and `_`, which rules out path separators, spaces and shell metacharacters.
pub const LANGUAGE_PATTERN: &str = "^[a-zA-Z0-9._-]+$";

/// Whether `language` matches [`LANGUAGE_PATTERN`].
pub(crate) fn is_valid_language(language: &str) -> bool {
    !language.is_empty()
        && language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether `key` is a POSIX environment variable name (TS `ENV_KEY_PATTERN`,
/// `^[a-zA-Z_][a-zA-Z0-9_]*$`).
pub(crate) fn is_valid_env_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Single-quotes `value` for a POSIX shell (TS `shellQuote`); embedded quotes become `'\''`, so
/// the shell expands nothing inside.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
