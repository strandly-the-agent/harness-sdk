"""Shared error classification for OpenAI-compatible model providers."""

from typing import Literal

OpenAIErrorKind = Literal["context_overflow", "throttling"]

# Union of overflow phrases observed across OpenAI-compatible providers.
_CONTEXT_WINDOW_OVERFLOW_PATTERNS = (
    "maximum context length",
    "context_length_exceeded",
    "too many tokens",
    "context length",
    "input is too long for requested model",
    "input length and `max_tokens` exceed context limit",
    "too many total text bytes",
    "exceed customer model maximum",
)
_RATE_LIMIT_PATTERNS = ("rate_limit_exceeded", "rate limit", "too many requests")


def classify_openai_error(error: BaseException) -> OpenAIErrorKind | None:
    """Classify an OpenAI-compatible error as throttling or context overflow."""
    message = str(error).lower()
    raw_code = getattr(error, "code", None)
    code = raw_code.lower() if isinstance(raw_code, str) else ""
    status = getattr(error, "status_code", getattr(error, "status", None))

    if status == 429 or code == "rate_limit_exceeded" or any(pattern in message for pattern in _RATE_LIMIT_PATTERNS):
        return "throttling"

    if code == "context_length_exceeded" or any(pattern in message for pattern in _CONTEXT_WINDOW_OVERFLOW_PATTERNS):
        return "context_overflow"

    return None
