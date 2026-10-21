"""Tests for the InvocationContext handle."""

import sys
import threading
import time

import pytest

from strands import InvocationContext
from strands.agent import invocation_context as invocation_context_module
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
            nested._take_turn()

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


def test_lets_concurrent_threads_take_no_more_turns_than_the_limit(monkeypatch):
    check_limits = invocation_context_module._reached_limit

    def check_limits_then_yield_to_other_threads(invocation_context):
        limit_stop_reason = check_limits(invocation_context)
        time.sleep(0.0001)
        return limit_stop_reason

    monkeypatch.setattr(invocation_context_module, "_reached_limit", check_limits_then_yield_to_other_threads)
    request = _create_invocation_context({"turns": 200})
    turns_taken_per_thread = [0] * 4

    def take_turns(thread_number: int) -> None:
        for _attempt in range(200):
            if request._take_turn() is None:
                turns_taken_per_thread[thread_number] += 1

    threads = [threading.Thread(target=take_turns, args=(thread_number,)) for thread_number in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert (sum(turns_taken_per_thread), request.usage["turns_taken"]) == (200, 200)
