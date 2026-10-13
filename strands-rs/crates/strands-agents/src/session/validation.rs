//! Identifier validation for session paths (`strands-ts/src/session/validation.ts`).
//!
//! These are plain errors (`Error::Other`), not `Error::Session`, matching TS.

use super::types::Scope;
use crate::error::{Error, Result};

/// Validates that an identifier contains only `[a-z0-9_-]` and is non-empty.
pub fn validate_identifier(id: &str) -> Result<&str> {
    let valid = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !valid {
        return Err(Error::other(format!(
            "Identifier '{id}' can only contain lowercase letters, numbers, hyphens, and underscores"
        )));
    }
    Ok(id)
}

/// Validates that a snapshot scope name is supported.
pub fn validate_scope(scope: &str) -> Result<Scope> {
    match scope {
        "agent" => Ok(Scope::Agent),
        "multiAgent" => Ok(Scope::MultiAgent),
        other => Err(Error::other(format!(
            "Invalid scope '{other}': must be 'agent' or 'multiAgent'"
        ))),
    }
}

/// Validates that a string is a UUID v7 (version nibble `7`, variant nibble `8`/`9`/`a`/`b`).
pub fn validate_uuid_v7(id: &str) -> Result<()> {
    if !is_uuid_v7(id) {
        return Err(Error::other(format!("'{id}' is not a valid UUID v7 snapshot ID")));
    }
    Ok(())
}

fn is_uuid_v7(id: &str) -> bool {
    let b = id.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let ok = match i {
            8 | 13 | 18 | 23 => c == b'-',
            14 => c == b'7',
            19 => matches!(c.to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b'),
            _ => c.is_ascii_hexdigit(),
        };
        if !ok {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_identifier() {
        assert_eq!(validate_identifier("valid-id").unwrap(), "valid-id");
    }

    #[test]
    fn identifier_with_forward_slash_throws_error() {
        assert_eq!(
            validate_identifier("invalid/id").unwrap_err().to_string(),
            "Identifier 'invalid/id' can only contain lowercase letters, numbers, hyphens, and underscores"
        );
    }

    #[test]
    fn identifier_with_backslash_throws_error() {
        assert_eq!(
            validate_identifier("invalid\\id").unwrap_err().to_string(),
            "Identifier 'invalid\\id' can only contain lowercase letters, numbers, hyphens, and underscores"
        );
    }

    #[test]
    fn rejects_uppercase_and_empty_identifiers() {
        assert!(validate_identifier("Invalid").is_err());
        assert!(validate_identifier("").is_err());
    }

    #[test]
    fn returns_the_scope() {
        assert_eq!(
            [
                validate_scope("agent").unwrap(),
                validate_scope("multiAgent").unwrap()
            ],
            [Scope::Agent, Scope::MultiAgent]
        );
    }

    #[test]
    fn scope_with_path_traversal_throws_error() {
        assert_eq!(
            validate_scope("../../outside").unwrap_err().to_string(),
            "Invalid scope '../../outside': must be 'agent' or 'multiAgent'"
        );
    }

    #[test]
    fn valid_uuid_v7_does_not_throw() {
        assert!(validate_uuid_v7("01956891-2b4c-7000-8abc-123456789abc").is_ok());
    }

    #[test]
    fn uuid_v4_wrong_version_throws_error() {
        assert_eq!(
            validate_uuid_v7("550e8400-e29b-41d4-a716-446655440000")
                .unwrap_err()
                .to_string(),
            "'550e8400-e29b-41d4-a716-446655440000' is not a valid UUID v7 snapshot ID"
        );
    }

    #[test]
    fn timestamp_string_throws_error() {
        assert_eq!(
            validate_uuid_v7("2025-01-15T10:30:00Z").unwrap_err().to_string(),
            "'2025-01-15T10:30:00Z' is not a valid UUID v7 snapshot ID"
        );
    }

    #[test]
    fn uuid_with_path_traversal_throws_error() {
        assert_eq!(
            validate_uuid_v7("../evil").unwrap_err().to_string(),
            "'../evil' is not a valid UUID v7 snapshot ID"
        );
    }

    #[test]
    fn empty_uuid_throws_error() {
        assert_eq!(
            validate_uuid_v7("").unwrap_err().to_string(),
            "'' is not a valid UUID v7 snapshot ID"
        );
    }

    #[test]
    fn uuid_v7_check_is_case_insensitive() {
        assert!(validate_uuid_v7("01956891-2B4C-7000-ABCD-123456789ABC").is_ok());
        assert!(validate_uuid_v7("01956891-2b4c-7000-cabc-123456789abc").is_err());
        assert!(uuid::Uuid::now_v7()
            .to_string()
            .chars()
            .all(|c| !c.is_ascii_uppercase()));
        validate_uuid_v7(&uuid::Uuid::now_v7().to_string()).unwrap();
        validate_identifier(&uuid::Uuid::now_v7().to_string()).unwrap();
    }
}
