"""Table-wide drift check: every direct-API key in the context-window table matches the provider's reported limit.

Keys the provider no longer serves skip (not fail), so the skip list doubles as a report of stale rows.
Metadata calls only; no model turns.
"""

import pytest

from strands.models._defaults import _CONTEXT_WINDOW_LIMITS
from tests_integ.models.context_window_lookups import CONTEXT_WINDOW_LOOKUPS, ModelNotServed, provider_for

DIRECT_API_MODELS = [
    pytest.param(provider, model_id, table_window, id=model_id, marks=provider.mark)
    for model_id, table_window in _CONTEXT_WINDOW_LIMITS.items()
    if (provider := provider_for(model_id)) is not None
]


@pytest.mark.parametrize(("provider", "model_id", "table_window"), DIRECT_API_MODELS)
def test_context_window_matches_provider_api(provider, model_id, table_window):
    try:
        api_window = CONTEXT_WINDOW_LOOKUPS[provider](model_id)
    except ModelNotServed:
        pytest.skip(f"{model_id}: not served by {provider.id} API: retired?")

    assert table_window == api_window, (
        f"{model_id}: table says {table_window}, provider API reports {api_window}; update strands/models/_defaults.py"
    )
