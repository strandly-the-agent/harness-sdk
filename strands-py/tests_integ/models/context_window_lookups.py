"""Live context-window lookups for providers whose API reports the model's input limit.

Each lookup uses the provider's own SDK client and the same env key the provider fixture uses.
"""

import os

import anthropic
from google import genai
from google.genai import errors as genai_errors
from mistralai.client import Mistral
from mistralai.client.errors import SDKError as MistralSDKError

from tests_integ.models import providers


class ModelNotServed(Exception):
    """The provider API returned not-found for the model id (retired, or never served)."""


def _anthropic(model_id: str) -> int | None:
    try:
        return anthropic.Anthropic(api_key=os.getenv("ANTHROPIC_API_KEY")).models.retrieve(model_id).max_input_tokens
    except anthropic.NotFoundError as e:
        raise ModelNotServed(model_id) from e


def _gemini(model_id: str) -> int | None:
    try:
        return genai.Client(api_key=os.getenv("GOOGLE_API_KEY")).models.get(model=model_id).input_token_limit
    except genai_errors.ClientError as e:
        if e.code == 404:
            raise ModelNotServed(model_id) from e
        raise


def _mistral(model_id: str) -> int | None:
    try:
        return Mistral(api_key=os.getenv("MISTRAL_API_KEY")).models.retrieve(model_id=model_id).max_context_length
    except MistralSDKError as e:
        if e.status_code == 404:
            raise ModelNotServed(model_id) from e
        raise


CONTEXT_WINDOW_LOOKUPS = {
    providers.anthropic: _anthropic,
    providers.gemini: _gemini,
    providers.mistral: _mistral,
}

# Direct-API key shapes in the context-window table; Bedrock-style `vendor.` keys match none of these.
MODEL_ID_PREFIXES = {
    "claude-": providers.anthropic,
    "gemini-": providers.gemini,
    "mistral-": providers.mistral,
    "ministral-": providers.mistral,
    "codestral-": providers.mistral,
    "magistral-": providers.mistral,
}


def provider_for(model_id: str) -> providers.ProviderInfo | None:
    """Return the direct-API provider a table key belongs to, or None for keys that aren't direct-API."""
    return next((provider for prefix, provider in MODEL_ID_PREFIXES.items() if model_id.startswith(prefix)), None)
