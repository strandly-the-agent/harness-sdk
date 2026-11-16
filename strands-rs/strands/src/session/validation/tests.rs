//! Ported from strands-ts/src/session/__tests__/validation.test.ts

use super::{validate_identifier, validate_scope, validate_uuid_v7};
use crate::types::snapshot::Scope;

fn message(result: crate::Result<impl Sized>) -> String {
    result.err().unwrap().to_string()
}

#[test]
fn when_identifier_is_valid_returns_the_identifier() {
    assert_eq!(validate_identifier("valid-id").unwrap(), "valid-id");
}

#[test]
fn when_identifier_contains_forward_slash_throws_error() {
    assert_eq!(
        message(validate_identifier("invalid/id")),
        "Identifier 'invalid/id' can only contain lowercase letters, numbers, hyphens, and underscores"
    );
}

#[test]
fn when_identifier_contains_backslash_throws_error() {
    assert_eq!(
        message(validate_identifier("invalid\\id")),
        "Identifier 'invalid\\id' can only contain lowercase letters, numbers, hyphens, and underscores"
    );
}

#[test]
fn when_scope_is_supported_returns_the_scope() {
    assert_eq!(
        [
            validate_scope("agent").unwrap(),
            validate_scope("multiAgent").unwrap()
        ],
        [Scope::Agent, Scope::MultiAgent]
    );
}

#[test]
fn when_scope_contains_path_traversal_throws_error() {
    assert_eq!(
        message(validate_scope("../../outside")),
        "Invalid scope '../../outside': must be 'agent' or 'multiAgent'"
    );
}

#[test]
fn when_id_is_a_valid_uuid_v7_does_not_throw() {
    validate_uuid_v7("01956891-2b4c-7000-8abc-123456789abc").unwrap();
}

#[test]
fn when_id_is_a_uuid_v4_wrong_version_throws_error() {
    assert_eq!(
        message(validate_uuid_v7("550e8400-e29b-41d4-a716-446655440000")),
        "'550e8400-e29b-41d4-a716-446655440000' is not a valid UUID v7 snapshot ID"
    );
}

#[test]
fn when_id_is_a_timestamp_string_throws_error() {
    assert_eq!(
        message(validate_uuid_v7("2025-01-15T10:30:00Z")),
        "'2025-01-15T10:30:00Z' is not a valid UUID v7 snapshot ID"
    );
}

#[test]
fn when_id_contains_path_traversal_throws_error() {
    assert_eq!(
        message(validate_uuid_v7("../evil")),
        "'../evil' is not a valid UUID v7 snapshot ID"
    );
}

#[test]
fn when_id_is_empty_string_throws_error() {
    assert_eq!(
        message(validate_uuid_v7("")),
        "'' is not a valid UUID v7 snapshot ID"
    );
}

#[test]
fn an_empty_identifier_is_rejected() {
    assert!(validate_identifier("").is_err());
    assert!(validate_identifier("Upper").is_err());
}
