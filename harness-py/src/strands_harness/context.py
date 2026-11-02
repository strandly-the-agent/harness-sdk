"""Harness context management: the default (``"auto"``) reduction pipeline.

The pipeline mirrors the SDK's ``"auto"`` preset (tool-result truncation, then LLM summarization at
high utilization) with one harness-owned step ahead of the summarizer: once utilization crosses the
threshold, everything older than the last :data:`KEEP_RECENT_TURNS` turns collapses into a single
deterministic summary note instead of being dropped or sent to the model for summarization.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from strands.experimental.context_manager import ContextManager, ContextState, Offload
from strands.types.content import ContentBlock, Message

if TYPE_CHECKING:
    from strands.types.content import Messages

KEEP_RECENT_TURNS = 10
"""Turns (assistant message plus the tool results / input that follow it) kept verbatim."""

SUMMARIZE_UTILIZATION = 0.85
"""Context utilization at which older turns are summarized; matches the SDK's ``"auto"`` threshold."""

# The SDK's "auto" preset settings; private there, so repeated here for the harness pipeline.
_TRUNCATE_THRESHOLD = 1_500
_TRUNCATE_PREVIEW_TOKENS = 750

_SUMMARY_HEADER = (
    "<system-reminder>\nAutomatically generated summary of earlier conversation (not written by the user)."
)


def build_auto_context_manager() -> ContextManager:
    """The harness's ``"auto"`` context manager."""
    return ContextManager(
        strategies=[
            Offload.truncate("tool_results", {"preview_tokens": _TRUNCATE_PREVIEW_TOKENS}).when(
                threshold=_TRUNCATE_THRESHOLD
            ),
            SlidingWindowSummarizeStrategy(),
            Offload.summarize("*").when(utilization=SUMMARIZE_UTILIZATION, preserve_recent=4),
        ]
    )


class SlidingWindowSummarizeStrategy:
    """Collapse turns older than the last :data:`KEEP_RECENT_TURNS` into one templated summary note.

    The first message (the task, with the harness's environment notes) is kept, the summary is folded
    into it as a ``<system-reminder>`` block (a standalone user message would break role alternation),
    and the kept window starts at an assistant message so no tool use/result pair is split. A history
    shorter than the window is left alone, so the SDK's existing strategies handle it as before.
    """

    def __init__(self) -> None:
        self._replaced_messages = 0
        self._replaced_turns = 0
        self._tool_names: dict[str, None] = {}

    @property
    def name(self) -> str:
        """Strategy name."""
        return "harness:summarize-window"

    async def apply(self, context: ContextState) -> bool:
        """Summarize when over the threshold (or recovering from overflow). Returns True if it acted."""
        if not context.overflow and context.utilization < SUMMARIZE_UTILIZATION:
            return False
        messages = context.messages
        cut = _window_start(messages, KEEP_RECENT_TURNS)
        if cut <= 1 or messages[0]["role"] != "user":
            return False

        # Experimental alternate context-management strategy: summarize-then-keep-window instead of drop-oldest.
        replaced = messages[1:cut]
        self._replaced_messages += len(replaced)
        self._replaced_turns += sum(1 for message in replaced if message["role"] == "assistant")
        for message in replaced:
            for block in message["content"]:
                if "toolUse" in block:
                    self._tool_names.setdefault(block["toolUse"]["name"], None)

        head = messages[0]
        kept = [block for block in head["content"] if not _is_summary_block(block)]
        summary = ContentBlock(text=self._render())
        messages[0] = Message({**head, "content": [*kept, summary]})  # type: ignore[typeddict-item]
        del messages[1:cut]
        return True

    def _render(self) -> str:
        turns = "turn" if self._replaced_turns == 1 else "turns"
        if self._tool_names:
            tools = (
                "Tools called during the replaced turns, in order of first use: " + ", ".join(self._tool_names) + "."
            )
        else:
            tools = "No tools were called during the replaced turns."
        return (
            f"{_SUMMARY_HEADER} To free context, {self._replaced_messages} earlier messages "
            f"({self._replaced_turns} {turns}) between the task above and the conversation below were replaced "
            f"by this note; the most recent {KEEP_RECENT_TURNS} turns are kept verbatim. {tools}\n</system-reminder>"
        )


def _window_start(messages: Messages, turns: int) -> int:
    """Index of the assistant message that opens the last ``turns`` turns, or 0 if there are fewer."""
    remaining = turns
    for index in range(len(messages) - 1, 0, -1):
        if messages[index]["role"] == "assistant":
            remaining -= 1
            if remaining == 0:
                return index
    return 0


def _is_summary_block(block: ContentBlock) -> bool:
    return "text" in block and block["text"].startswith(_SUMMARY_HEADER)
