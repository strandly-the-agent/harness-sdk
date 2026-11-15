//! Error classification shared by both OpenAI APIs (TS `models/openai/errors.ts`).

/// Error message phrases (lowercase) that mean the request overflowed the context window.
const CONTEXT_WINDOW_OVERFLOW_PATTERNS: [&str; 8] = [
    "maximum context length",
    "context_length_exceeded",
    "too many tokens",
    "context length",
    "input is too long for requested model",
    "input length and `max_tokens` exceed context limit",
    "too many total text bytes",
    "exceed customer model maximum",
];

/// Error message phrases (lowercase) that mean the request was rate limited.
const RATE_LIMIT_PATTERNS: [&str; 3] = ["rate_limit_exceeded", "rate limit", "too many requests"];

/// What an OpenAI error means to the SDK (TS `OpenAIErrorKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAIErrorKind {
    ContextOverflow,
    Throttling,
}

/// Classifies an error by its status, string `code` and message (TS `classifyOpenAIError`).
pub(crate) fn classify_openai_error(
    status: Option<u16>,
    code: Option<&str>,
    message: &str,
) -> Option<OpenAIErrorKind> {
    let message = message.to_lowercase();
    let code = code.map(str::to_lowercase).unwrap_or_default();
    if status == Some(429)
        || code == "rate_limit_exceeded"
        || RATE_LIMIT_PATTERNS
            .iter()
            .any(|pattern| message.contains(pattern))
    {
        return Some(OpenAIErrorKind::Throttling);
    }
    if code == "context_length_exceeded"
        || CONTEXT_WINDOW_OVERFLOW_PATTERNS
            .iter()
            .any(|pattern| message.contains(pattern))
    {
        return Some(OpenAIErrorKind::ContextOverflow);
    }
    None
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/models/openai/__tests__/errors.test.ts. TS `code` values that
    //! are not strings (numbers, `null`) never reach the classifier: the client keeps only string codes.

    use super::*;

    fn classify(message: &str) -> Option<OpenAIErrorKind> {
        classify_openai_error(None, None, message)
    }

    #[test]
    fn classifies_overflow_phrase_p_as_context_overflow() {
        for phrase in [
            "maximum context length exceeded",
            "context_length_exceeded",
            "too many tokens",
            "context length",
            "Input is too long for requested model",
            "input length and `max_tokens` exceed context limit",
            "too many total text bytes",
            "prompt tokens (320666) exceed customer model maximum (278528) for model-id",
        ] {
            assert_eq!(
                classify(phrase),
                Some(OpenAIErrorKind::ContextOverflow),
                "{phrase}"
            );
        }
    }

    #[test]
    fn matches_overflow_phrases_case_insensitively() {
        assert_eq!(
            classify("MAXIMUM CONTEXT LENGTH EXCEEDED"),
            Some(OpenAIErrorKind::ContextOverflow)
        );
    }

    #[test]
    fn classifies_a_structured_context_length_exceeded_code_as_context_overflow() {
        assert_eq!(
            classify_openai_error(None, Some("context_length_exceeded"), "something opaque"),
            Some(OpenAIErrorKind::ContextOverflow)
        );
    }

    #[test]
    fn matches_structured_codes_case_insensitively() {
        assert_eq!(
            classify_openai_error(None, Some("Context_Length_Exceeded"), "something opaque"),
            Some(OpenAIErrorKind::ContextOverflow)
        );
    }

    #[test]
    fn returns_undefined_for_unrelated_errors() {
        assert_eq!(classify("a totally unrelated failure"), None);
    }

    #[test]
    fn handles_numeric_error_codes_without_throwing() {
        let error = super::super::client::ApiError::from_status(
            400,
            r#"{"error":{"message":"some error","code":400}}"#,
        );
        assert_eq!(error.code, None);
        assert_eq!(
            classify_openai_error(None, error.code.as_deref(), "some error"),
            None
        );
    }

    #[test]
    fn handles_null_error_code_without_throwing() {
        let error = super::super::client::ApiError::from_status(
            400,
            r#"{"error":{"message":"some error","code":null}}"#,
        );
        assert_eq!(error.code, None);
        assert_eq!(
            classify_openai_error(None, error.code.as_deref(), "some error"),
            None
        );
    }

    #[test]
    fn classifies_429_rate_limit_codes_and_phrases_as_throttling() {
        assert_eq!(
            classify_openai_error(Some(429), None, "x"),
            Some(OpenAIErrorKind::Throttling)
        );
        assert_eq!(
            classify_openai_error(None, Some("RATE_LIMIT_EXCEEDED"), "x"),
            Some(OpenAIErrorKind::Throttling)
        );
        for phrase in ["Rate limit reached", "Too Many Requests", "rate_limit_exceeded"] {
            assert_eq!(classify(phrase), Some(OpenAIErrorKind::Throttling), "{phrase}");
        }
    }
}
