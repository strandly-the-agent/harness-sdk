import pytest

from strands.models._openai_errors import classify_openai_error


class StubOpenAIError(Exception):
    def __init__(self, message: str, *, code: object = None, status_code: int | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.status_code = status_code


@pytest.mark.parametrize(
    "message",
    [
        "maximum context length",
        "context_length_exceeded",
        "too many tokens",
        "context length",
        "Input is too long for requested model",
        "input length and `max_tokens` exceed context limit",
        "too many total text bytes",
        "prompt tokens (320666) exceed customer model maximum (278528) for model-id",
    ],
)
def test_classify_openai_error_context_overflow_messages(message):
    assert classify_openai_error(StubOpenAIError(message)) == "context_overflow"


@pytest.mark.parametrize("code", ["context_length_exceeded", "Context_Length_Exceeded"])
def test_classify_openai_error_context_overflow_codes_are_case_insensitive(code):
    assert classify_openai_error(StubOpenAIError("request rejected", code=code)) == "context_overflow"


@pytest.mark.parametrize("message", ["rate_limit_exceeded", "Rate limit reached", "Too Many Requests"])
def test_classify_openai_error_throttling_messages(message):
    assert classify_openai_error(StubOpenAIError(message)) == "throttling"


@pytest.mark.parametrize("code", ["rate_limit_exceeded", "Rate_Limit_Exceeded"])
def test_classify_openai_error_throttling_codes_are_case_insensitive(code):
    assert classify_openai_error(StubOpenAIError("request rejected", code=code)) == "throttling"


def test_classify_openai_error_throttling_status_code():
    assert classify_openai_error(StubOpenAIError("request rejected", status_code=429)) == "throttling"


def test_classify_openai_error_throttling_status_when_status_code_is_none():
    error = StubOpenAIError("request rejected")
    error.status = 429
    assert classify_openai_error(error) == "throttling"


def test_classify_openai_error_ignores_non_string_code():
    assert classify_openai_error(StubOpenAIError("request rejected", code=400)) is None
