//! Identifier validation (TS `session/validation.ts`).

use std::sync::LazyLock;

use regex::Regex;

use crate::error::{Error, Result};
use crate::types::snapshot::Scope;

static UUID_V7: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$").ok()
});

/// `id` when it matches `^[a-z0-9_-]+$` (TS `validateIdentifier`), so ids are safe path segments.
pub(crate) fn validate_identifier(id: &str) -> Result<&str> {
    let valid = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !valid {
        return Err(Error::other(format!(
            "Identifier '{id}' can only contain lowercase letters, numbers, hyphens, and underscores"
        )));
    }
    Ok(id)
}

/// The scope named `scope` (TS `validateScope`).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TS calls it only from the deprecated FileStorage and S3 storage"
    )
)]
pub(crate) fn validate_scope(scope: &str) -> Result<Scope> {
    match scope {
        "agent" => Ok(Scope::Agent),
        "multiAgent" => Ok(Scope::MultiAgent),
        _ => Err(Error::other(format!(
            "Invalid scope '{scope}': must be 'agent' or 'multiAgent'"
        ))),
    }
}

/// Fails unless `id` is a UUID v7 (TS `validateUuidV7`).
pub(crate) fn validate_uuid_v7(id: &str) -> Result<()> {
    if UUID_V7.as_ref().is_some_and(|pattern| pattern.is_match(id)) {
        return Ok(());
    }
    Err(Error::other(format!("'{id}' is not a valid UUID v7 snapshot ID")))
}

/// The TS string of `scope` (`agent`, `multiAgent`).
pub(crate) fn scope_str(scope: Scope) -> &'static str {
    match scope {
        Scope::MultiAgent => "multiAgent",
        _ => "agent",
    }
}

#[cfg(test)]
mod tests;
