//! Validation helpers (TS `types/validation.ts`).

use std::fmt;

/// The error a strict wire-data parser or a validation helper reports, carrying the TS message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataError(String);

impl DataError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "read when `strands::Error` wraps it"))]
    pub(crate) fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DataError {}

/// Returns the value, or the TS `ensureDefined` error naming the missing field.
#[cfg_attr(not(test), expect(dead_code, reason = "used by the Bedrock provider"))]
pub(crate) fn ensure_defined<T>(value: Option<T>, field_name: &str) -> Result<T, DataError> {
    value.ok_or_else(|| DataError::new(format!("Expected {field_name} to be defined, but got undefined")))
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/types/__tests__/validation.test.ts

    use super::*;

    #[test]
    fn returns_the_value() {
        assert_eq!(ensure_defined(Some("test"), "testField").unwrap(), "test");
    }

    #[test]
    fn returns_zero() {
        assert_eq!(ensure_defined(Some(0), "numberField").unwrap(), 0);
    }

    #[test]
    fn returns_empty_string() {
        assert_eq!(ensure_defined(Some(""), "stringField").unwrap(), "");
    }

    #[test]
    fn when_value_is_undefined_throws_error_with_field_name() {
        let error = ensure_defined(None::<&str>, "testField").unwrap_err();
        assert_eq!(
            error.message(),
            "Expected testField to be defined, but got undefined"
        );
    }
}
