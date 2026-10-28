"""What the SDK knows about models: the :class:`ModelInfo` type, and the shared model info data.

Every :class:`ModelInfo` field is optional, and a missing or ``None`` field means unknown. The same
shape is used everywhere: the shared data file, the ``model_info`` config override, and the resolved
:attr:`strands.models.Model.model_info`. A known ``"unsupported"`` disables a feature; unknown leaves
the SDK's existing default for that feature in place.

``model_info.json`` is copied from ``shared/model-info.json`` at the repository root (the source of
truth for every SDK) by the build hook in ``hatch_build.py``; it is not committed.
"""

import json
import re
from dataclasses import dataclass, fields, replace
from importlib import resources
from typing import Literal, TypeVar

PromptCaching = Literal["unsupported", "auto", "cachePoints"]
"""How a model supports prompt caching."""


@dataclass(frozen=True)
class ModelCapabilities:
    """What the model can do.

    A yes/no fact is a ``bool``; a fact with more than two states is a string literal, like
    :data:`PromptCaching` (which mechanism). A fact about two features together gets its own field
    named after the pair. ``None`` is unknown either way.

    Attributes:
        stateful: Whether the model keeps conversation state server-side;
            :attr:`strands.models.Model.stateful` reads it.
        prompt_caching: Whether and how the model caches prompts.
        forced_tool_choice: Whether the model accepts a tool choice that forces a tool call
            (``any`` or a named tool).
        native_structured_output: Whether the provider can make the model's text output conform to a
            JSON schema.
    """

    stateful: bool | None = None
    prompt_caching: PromptCaching | None = None
    forced_tool_choice: bool | None = None
    native_structured_output: bool | None = None


@dataclass(frozen=True)
class ModelLimits:
    """Size limits of the model.

    Attributes:
        context_window: Context window size in tokens.
        max_output_tokens: Largest output token count the model accepts. Providers that need a
            default for it use it.
    """

    context_window: int | None = None
    max_output_tokens: int | None = None


@dataclass(frozen=True)
class ModelInfo:
    """Everything the SDK knows about a model.

    Read it from :attr:`strands.models.Model.model_info`.

    Attributes:
        capabilities: What the model can do, or None when nothing is known.
        limits: Size limits of the model, or None when nothing is known.
    """

    capabilities: ModelCapabilities | None = None
    limits: ModelLimits | None = None


_SectionT = TypeVar("_SectionT", ModelCapabilities, ModelLimits)


def _merge_section(parts: list[_SectionT | None], empty: _SectionT) -> _SectionT | None:
    """Merge one section of the records, later defined fields winning, or None when nothing is known."""
    merged = empty
    for part in parts:
        if part is None:
            continue
        known = {
            field.name: getattr(part, field.name) for field in fields(part) if getattr(part, field.name) is not None
        }
        if known:
            merged = replace(merged, **known)
    return merged if merged != empty else None


def merge_model_info(*parts: ModelInfo | None) -> ModelInfo:
    """Merge partial records, later defined fields winning. None and missing fields inherit.

    Args:
        *parts: Partial records, in order of increasing precedence.

    Returns:
        A new record containing only the fields that are known.
    """
    return ModelInfo(
        capabilities=_merge_section([part.capabilities for part in parts if part], ModelCapabilities()),
        limits=_merge_section([part.limits for part in parts if part], ModelLimits()),
    )


_CAPABILITY_FIELDS = {
    "stateful": "stateful",
    "promptCaching": "prompt_caching",
    "forcedToolChoice": "forced_tool_choice",
    "nativeStructuredOutput": "native_structured_output",
}
_LIMIT_FIELDS = {"contextWindow": "context_window", "maxOutputTokens": "max_output_tokens"}

# Bedrock appends a version to some ids (`claude-3-5-haiku-20241022-v1:0`); the base id carries the facts.
_VERSION_SUFFIX = re.compile(r"-(?:v\d+(?::\d+)?|\d+:\d+)$")
# Dated snapshots (`claude-opus-5-5-20260401`, `claude-haiku-4-5@20251001`) share the undated model's facts.
_DATE_SUFFIX = re.compile(r"[-@]\d{8}$")


def _load_data() -> tuple[int, dict[str, ModelInfo]]:
    """Read the shared model info data, re-casing its camelCase keys onto the record fields."""
    data = json.loads(resources.files("strands.models").joinpath("model_info.json").read_text(encoding="utf-8"))
    models = {
        model_id: ModelInfo(
            capabilities=ModelCapabilities(
                **{_CAPABILITY_FIELDS[key]: value for key, value in entry.get("capabilities", {}).items()}
            )
            if entry.get("capabilities")
            else None,
            limits=ModelLimits(**{_LIMIT_FIELDS[key]: value for key, value in entry.get("limits", {}).items()})
            if entry.get("limits")
            else None,
        )
        for model_id, entry in data["models"].items()
    }
    return data["defaults"]["limits"]["contextWindow"], models


# Context window limit (in tokens) assumed for a model whose window is unknown, and the known models.
DEFAULT_CONTEXT_WINDOW_LIMIT, _MODELS = _load_data()


def lookup_model(model_id: str) -> ModelInfo | None:
    """Find the entry for a model id.

    Tries the id as given, then without a trailing Bedrock version suffix, then without a trailing
    date, then with each leading dot-separated segment removed in turn
    (``us.anthropic.claude-x`` to ``anthropic.claude-x`` to ``claude-x``). One entry therefore serves
    a model across providers and snapshots: ``claude-opus-5-5`` answers for
    ``anthropic.claude-opus-5-5-20260401-v1:0`` too.

    Args:
        model_id: The model id to look up.

    Returns:
        The entry, or None when the id is not in the data.
    """
    candidate = model_id
    while True:
        unversioned = _VERSION_SUFFIX.sub("", candidate)
        for key in (candidate, unversioned, _DATE_SUFFIX.sub("", unversioned)):
            if key in _MODELS:
                return _MODELS[key]
        dot_index = candidate.find(".")
        if dot_index == -1:
            return None
        candidate = candidate[dot_index + 1 :]


def lookup_model_info(model_id: str) -> ModelInfo:
    """Report what the shared model info data knows about a model.

    Args:
        model_id: The model id to describe.

    Returns:
        What the data knows about the model; empty when it knows nothing.
    """
    return merge_model_info(lookup_model(model_id))


def get_context_window_limit(model_id: str) -> int | None:
    """Look up the context window limit for a model id.

    Args:
        model_id: The model id to look up.

    Returns:
        The context window limit in tokens, or None if not known.
    """
    limits = lookup_model_info(model_id).limits
    return limits.context_window if limits else None
