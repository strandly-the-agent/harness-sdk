"""Tests for the InvocationContext handle."""

import sys
import threading

import pytest

from strands import InvocationContext
from strands.agent.invocation_context import _create_invocation_context


def test_rejects_direct_construction():
    with pytest.raises(TypeError, match="created by the SDK"):
        InvocationContext()


@pytest.fixture
def frequent_thread_switches():
    switch_interval = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    yield
    sys.setswitchinterval(switch_interval)


@pytest.mark.usefixtures("frequent_thread_switches")
def test_records_every_update_made_from_concurrent_threads():
    request = _create_invocation_context()
    nested = _create_invocation_context({"turns": 100_000}, request)
    usage = {"inputTokens": 1, "outputTokens": 1, "totalTokens": 2, "cacheReadInputTokens": 1}

    def record() -> None:
        for _call_number in range(20_000):
            nested._record_usage(usage)
            nested._record_turn()

    threads = [threading.Thread(target=record) for _thread_number in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert request.usage == {
        "turns_taken": 80_000,
        "inputTokens": 80_000,
        "outputTokens": 80_000,
        "totalTokens": 160_000,
        "cacheReadInputTokens": 80_000,
    }
