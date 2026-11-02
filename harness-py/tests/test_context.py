"""Tests for the harness's sliding-window summarization strategy."""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

import pytest
from strands.experimental.context_manager import ContextState
from strands.types.content import Message

from strands_harness import context as ctx
from strands_harness.context import KEEP_RECENT_TURNS, SlidingWindowSummarizeStrategy, build_auto_context_manager


def _turn(index: int, tool: str | None) -> list[Message]:
    """One turn: an assistant message and the user message answering it."""
    if tool is None:
        return [
            Message(role="assistant", content=[{"text": f"reply {index}"}]),
            Message(role="user", content=[{"text": f"follow-up {index}"}]),
        ]
    tool_use_id = f"t{index}"
    return [
        Message(role="assistant", content=[{"toolUse": {"toolUseId": tool_use_id, "name": tool, "input": {}}}]),
        Message(
            role="user",
            content=[{"toolResult": {"toolUseId": tool_use_id, "status": "success", "content": [{"text": "ok"}]}}],
        ),
    ]


def _history(turns: int, tools: Sequence[str | None] | None = None) -> list[Message]:
    tools = tools or [None]
    messages = [
        Message(role="user", content=[{"text": "the task"}, {"text": "<system-reminder>\nenv\n</system-reminder>"}])
    ]
    for index in range(turns):
        messages.extend(_turn(index, tools[index % len(tools)]))
    return messages


def _state(messages: list[Message], utilization: float = 1.0, overflow: bool = False) -> ContextState:
    return ContextState(messages=messages, agent=object(), utilization=utilization, overflow=overflow)  # type: ignore[arg-type]


def _assistant_count(messages: list[Message]) -> int:
    return sum(1 for m in messages if m["role"] == "assistant")


def _alternates(messages: list[Message]) -> bool:
    return all(a["role"] != b["role"] for a, b in zip(messages, messages[1:], strict=False))


async def test_keeps_last_n_turns_verbatim_and_summarizes_the_rest():
    messages = _history(15, ["shell", "read", "shell", None, "edit"])
    original = [dict(m) for m in messages]

    assert await SlidingWindowSummarizeStrategy().apply(_state(messages)) is True

    assert _assistant_count(messages) == KEEP_RECENT_TURNS
    assert messages[1:] == original[-2 * KEEP_RECENT_TURNS :]
    assert messages[1]["role"] == "assistant"
    assert _alternates(messages)

    head = messages[0]
    assert head["role"] == "user"
    assert head["content"][:2] == original[0]["content"]
    summary = head["content"][2]["text"]
    assert summary.startswith("<system-reminder>\nAutomatically generated summary of earlier conversation")
    assert summary.endswith("</system-reminder>")
    assert "10 earlier messages (5 turns)" in summary
    assert "in order of first use: shell, read, edit." in summary


async def test_summary_without_tool_calls():
    messages = _history(KEEP_RECENT_TURNS + 1)
    assert await SlidingWindowSummarizeStrategy().apply(_state(messages)) is True
    assert "2 earlier messages (1 turn)" in messages[0]["content"][-1]["text"]
    assert "No tools were called" in messages[0]["content"][-1]["text"]


async def test_repeated_passes_keep_one_cumulative_summary_block():
    strategy = SlidingWindowSummarizeStrategy()
    messages = _history(KEEP_RECENT_TURNS + 1, ["shell", "read"] + ["shell"] * (KEEP_RECENT_TURNS - 1))
    assert await strategy.apply(_state(messages)) is True
    messages.extend(_turn(99, "shell"))
    assert await strategy.apply(_state(messages)) is True

    assert _assistant_count(messages) == KEEP_RECENT_TURNS
    summaries = [b for b in messages[0]["content"] if b.get("text", "").startswith("<system-reminder>\nAutomatically")]
    assert len(summaries) == 1
    assert "4 earlier messages (2 turns)" in summaries[0]["text"]
    assert "shell, read." in summaries[0]["text"]


@pytest.mark.parametrize("turns", [0, 1, KEEP_RECENT_TURNS])
async def test_short_history_is_left_to_the_existing_strategies(turns: int):
    messages = _history(turns)
    before = list(messages)
    assert await SlidingWindowSummarizeStrategy().apply(_state(messages)) is False
    assert messages == before


async def test_below_threshold_does_nothing_unless_overflowing():
    messages = _history(15)
    assert await SlidingWindowSummarizeStrategy().apply(_state(messages, utilization=0.5)) is False
    assert _assistant_count(messages) == 15
    assert await SlidingWindowSummarizeStrategy().apply(_state(messages, utilization=0.5, overflow=True)) is True
    assert _assistant_count(messages) == KEEP_RECENT_TURNS


async def test_history_not_opened_by_a_user_message_is_left_alone():
    messages = _history(15)[1:]
    assert await SlidingWindowSummarizeStrategy().apply(_state(messages)) is False


def test_auto_pipeline_runs_the_window_summary_before_the_sdk_summarizer():
    names: list[Any] = [s.name for s in build_auto_context_manager()._strategies]
    assert names[: names.index("offload:summarize")] == ["offload:truncate", ctx.SlidingWindowSummarizeStrategy().name]
    assert names[-1] == "offload:emergency-truncate"
