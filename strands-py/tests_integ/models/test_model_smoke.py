"""Per-model smoke coverage: the context window resolves from the model table and a tool-using turn completes.

One row per model; each row carries its provider's skip mark, so a missing key skips only that provider's rows.
"""

from typing import Any, NamedTuple

import pytest

import strands
from strands import Agent
from tests_integ.models import providers


class SmokeModel(NamedTuple):
    provider: providers.ProviderInfo
    model_id: str
    expected_window: int
    model_kwargs: dict[str, Any] = {}
    tool_use_marks: list[pytest.MarkDecorator] = []


LARGE_4_TOOL_USE_XFAIL = pytest.mark.xfail(
    strict=True,
    raises=TypeError,
    reason="Large 4 returns list content (ThinkChunk/TextChunk) that MistralModel passes through as text: #5032",
)

_US_EAST_1 = {"region_name": "us-east-1"}

SMOKE_MODELS = [
    SmokeModel(providers.gemini, "gemini-4-argon", 1_048_576),
    SmokeModel(providers.gemini, "gemini-3.8-flash", 1_048_576),
    SmokeModel(providers.bedrock, "us.xai.grok-4.7", 524_288, _US_EAST_1),
    SmokeModel(providers.bedrock, "mistral.mistral-large-3-675b-instruct", 256_000, _US_EAST_1),
    SmokeModel(providers.bedrock, "openai.gpt-oss-120b-1:0", 128_000, _US_EAST_1),
    SmokeModel(providers.mantle, "google.gemma-4-31b", 256_000),
    SmokeModel(providers.mantle, "openai.gpt-oss-120b", 128_000),
    SmokeModel(providers.mantle_responses, "openai.gpt-oss-120b", 128_000),
    SmokeModel(providers.openai, "chat-latest", 272_000),
    SmokeModel(providers.mistral, "mistral-medium-3-5", 262_144),
    SmokeModel(providers.mistral, "mistral-large-4-0", 1_048_576, tool_use_marks=[LARGE_4_TOOL_USE_XFAIL]),
    SmokeModel(providers.mistral, "ministral-14b-2512", 262_144),
    SmokeModel(providers.mistral, "mistral-small-2603", 262_144),
    SmokeModel(providers.mistral, "codestral-2508", 256_000),
]


def _rows(*, tool_use: bool) -> list[Any]:
    return [
        pytest.param(
            row,
            id=f"{row.provider.id}-{row.model_id}",
            marks=[row.provider.mark, *(row.tool_use_marks if tool_use else [])],
        )
        for row in SMOKE_MODELS
        if row.provider is not None
    ]


@strands.tool
def echo(value: str) -> str:
    """Echo the given value back."""
    return value


@pytest.mark.parametrize("row", _rows(tool_use=False))
def test_smoke_context_window_limit_resolves(row: SmokeModel):
    model = row.provider.create_model(model_id=row.model_id, **row.model_kwargs)

    assert model.get_config()["context_window_limit"] == row.expected_window


@pytest.mark.parametrize("row", _rows(tool_use=True))
def test_smoke_tool_use_turn(row: SmokeModel):
    model = row.provider.create_model(model_id=row.model_id, **row.model_kwargs)
    agent = Agent(
        model=model,
        tools=[echo],
        system_prompt="You are a helpful assistant.",
        load_tools_from_directory=False,
        callback_handler=None,
    )

    result = agent("Use the echo tool to echo the value 'pineapple'.")

    assert result.stop_reason == "end_turn"
    assert any(block.get("toolUse") for message in agent.messages for block in message["content"])
