"""Per-model smoke coverage: the context window resolves from the model table and a tool-using turn completes.

One row per model; each row carries its provider's skip mark, so a missing key skips only that provider's rows.
Where the provider's API reports the input limit, the table value is also cross-checked against it.
"""

import os

import pytest
from anthropic import Anthropic
from google import genai
from mistralai.client import Mistral

import strands
from strands import Agent
from tests_integ.models import providers

SMOKE_MODELS = [
    pytest.param(provider, model_id, expected_window, id=f"{provider.id}-{model_id}", marks=provider.mark)
    for provider, model_id, expected_window in [
        (providers.anthropic, "claude-opus-5-5", 1_000_000),
        (providers.anthropic, "claude-sonnet-5-5", 1_000_000),
        (providers.anthropic, "claude-haiku-5-5", 1_000_000),
        (providers.gemini, "gemini-4-argon", 1_048_576),
        (providers.gemini, "gemini-3.8-flash", 1_048_576),
        (providers.bedrock, "us.xai.grok-4.7", 524_288),
        (providers.bedrock, "mistral.mistral-large-3-675b-instruct", 256_000),
        (providers.bedrock, "openai.gpt-oss-120b-1:0", 128_000),
        (providers.mantle, "google.gemma-4-31b", 256_000),
        (providers.mantle, "openai.gpt-oss-120b", 128_000),
        (providers.mantle_responses, "openai.gpt-oss-120b", 128_000),
        (providers.openai, "chat-latest", 272_000),
        (providers.mistral, "mistral-medium-3-5", 262_144),
        (providers.mistral, "mistral-large-4-0", 1_048_576),
        (providers.mistral, "ministral-14b-2512", 262_144),
        (providers.mistral, "mistral-small-2603", 262_144),
        (providers.mistral, "codestral-2508", 256_000),
    ]
    if provider is not None
]

# Providers whose API reports the input limit (same env key the provider itself uses).
CONTEXT_WINDOW_LOOKUPS = {
    providers.anthropic: lambda model_id: (
        Anthropic(api_key=os.getenv("ANTHROPIC_API_KEY")).models.retrieve(model_id).max_input_tokens
    ),
    providers.gemini: lambda model_id: (
        genai.Client(api_key=os.getenv("GOOGLE_API_KEY")).models.get(model=model_id).input_token_limit
    ),
    providers.mistral: lambda model_id: (
        Mistral(api_key=os.getenv("MISTRAL_API_KEY")).models.retrieve(model_id=model_id).max_context_length
    ),
}
API_REPORTED_MODELS = [row for row in SMOKE_MODELS if row.values[0] in CONTEXT_WINDOW_LOOKUPS]

TOOL_USE_MARKS = {
    "mistral-large-4-0": pytest.mark.xfail(
        strict=True,
        raises=TypeError,
        reason="Large 4 returns list content (ThinkChunk/TextChunk) that MistralModel passes through as text: #5032",
    ),
}


@strands.tool
def echo(value: str) -> str:
    """Echo the given value back."""
    return value


@pytest.mark.parametrize(("provider", "model_id", "expected_window"), SMOKE_MODELS)
def test_smoke_context_window_limit_resolves(provider, model_id, expected_window):
    model = provider.create_model()
    model.update_config(model_id=model_id)

    assert model.get_config()["context_window_limit"] == expected_window


@pytest.mark.parametrize(("provider", "model_id", "expected_window"), API_REPORTED_MODELS)
def test_smoke_context_window_matches_provider_api(provider, model_id, expected_window):
    model = provider.create_model()
    model.update_config(model_id=model_id)

    table_window = model.get_config()["context_window_limit"]
    api_window = CONTEXT_WINDOW_LOOKUPS[provider](model_id)

    assert table_window == api_window, (
        f"{model_id}: table says {table_window}, provider API reports {api_window}; update strands/models/_defaults.py"
    )


@pytest.mark.parametrize(("provider", "model_id", "expected_window"), SMOKE_MODELS)
def test_smoke_tool_use_turn(request, provider, model_id, expected_window):
    if model_id in TOOL_USE_MARKS:
        request.applymarker(TOOL_USE_MARKS[model_id])
    model = provider.create_model()
    model.update_config(model_id=model_id)
    agent = Agent(model=model, tools=[echo], load_tools_from_directory=False, callback_handler=None)

    result = agent("Use the echo tool to echo the value 'pineapple'.")

    assert result.stop_reason == "end_turn"
    assert any(block.get("toolUse") for message in agent.messages for block in message["content"])
