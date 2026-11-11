//! Validation helpers (TS `types/validation.ts`).

use crate::error::ModelError;

/// `value`, or an error naming `field_name` when it is missing (TS `ensureDefined`).
///
/// Its callers check provider responses, so a missing field is a [`ModelError`].
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn ensure_defined<T>(value: Option<T>, field_name: &str) -> Result<T, ModelError> {
    value.ok_or_else(|| ModelError::new(format!("Expected {field_name} to be defined, but got undefined")))
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
    fn throws_error_with_field_name() {
        let err = ensure_defined::<&str>(None, "testField").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Expected testField to be defined, but got undefined"
        );
    }
}
