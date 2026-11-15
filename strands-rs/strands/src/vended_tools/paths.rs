//! Path rules shared by the sandbox-routed file tools (TS `vended-tools/paths.ts`).

use crate::tools::ToolError;

/// Rejects a path that is not absolute (POSIX `/`, a drive letter, or a UNC share) or that has a
/// `..` segment (TS `validateAbsolutePath`).
pub(crate) fn validate_absolute_path(path: &str) -> Result<(), ToolError> {
    if !(path.starts_with('/') || is_windows_absolute(path)) {
        return Err(ToolError::msg(format!(
            "The path {path} is not absolute; it should start with '/' (or a drive letter or UNC share on Windows)."
        )));
    }
    if path.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(ToolError::msg("Invalid path: path traversal is not allowed."));
    }
    Ok(())
}

/// A drive-letter (`C:\`, `C:/`) or UNC (`\\server\share`) root.
fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = matches!(bytes, [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic());
    drive || path.starts_with("\\\\")
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/vended-tools/__tests__/paths.test.ts

    use super::validate_absolute_path;

    fn error(path: &str) -> String {
        validate_absolute_path(path).unwrap_err().to_string()
    }

    #[test]
    fn accepts_a_posix_absolute_path() {
        assert!(validate_absolute_path("/tmp/file.txt").is_ok());
    }

    #[test]
    fn accepts_a_windows_drive_letter_absolute_path() {
        assert!(validate_absolute_path("C:\\project\\file.txt").is_ok());
    }

    #[test]
    fn accepts_a_unc_share() {
        assert!(validate_absolute_path("\\\\server\\share\\file.txt").is_ok());
    }

    #[test]
    fn rejects_a_relative_path() {
        assert_eq!(
            error("relative.txt"),
            "The path relative.txt is not absolute; it should start with '/' (or a drive letter or UNC share on Windows)."
        );
    }

    #[test]
    fn rejects_posix_parent_traversal() {
        assert_eq!(
            error("/tmp/../etc/passwd"),
            "Invalid path: path traversal is not allowed."
        );
    }

    #[test]
    fn rejects_windows_parent_traversal() {
        assert!(error("C:\\project\\..\\etc\\hosts").contains("path traversal is not allowed"));
    }

    #[test]
    fn accepts_a_double_dot_substring_inside_a_segment() {
        assert!(validate_absolute_path("/tmp/file..name.txt").is_ok());
    }
}
