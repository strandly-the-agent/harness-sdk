//! Validation helpers (TS `types/validation.ts`).

use crate::error::{DataError, Result};

/// Returns the value, or the TS `ensureDefined` error naming the missing field.
#[cfg_attr(not(test), expect(dead_code, reason = "used by the Bedrock provider"))]
pub(crate) fn ensure_defined<T>(value: Option<T>, field_name: &str) -> Result<T> {
    value.ok_or_else(|| {
        DataError::new(format!("Expected {field_name} to be defined, but got undefined")).into()
    })
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
        assert_eq!(error.name(), "Error");
        assert_eq!(
            error.to_string(),
            "Expected testField to be defined, but got undefined"
        );
    }
}
