"""Last-resort text truncation for when summarization cannot reduce the context."""

import re
from collections.abc import Iterator

from ....types.content import ContentBlock, Message, Messages
from ....types.tools import ToolResultContent
from .pin_message import is_pinned

_TRUNCATION_MARKER = re.compile(r"\.\.\. \[truncated: \d+ chars removed\] \.\.\.")
_PRESERVE_CHARS = 2_000


def _text_blocks(message: Message) -> Iterator[ContentBlock | ToolResultContent]:
    """Yield every dict carrying a ``text`` key: content blocks and items inside toolResult content."""
    for content in message.get("content", []):
        if not isinstance(content, dict):
            continue  # type: ignore[unreachable]
        if "text" in content:
            yield content
        for item in content.get("toolResult", {}).get("content", []):
            if isinstance(item, dict) and "text" in item:
                yield item


def _truncate_largest_text_blocks(messages: Messages, preserve_chars: int = _PRESERVE_CHARS) -> int:
    """Replace the largest untruncated text blocks in non-pinned messages with head+tail previews.

    Each call truncates as many blocks as the history already carries truncated, at least one, largest
    first (ties go to the oldest). The first overflow therefore costs exactly one block, and the caller
    retrying the model and calling again while it still overflows converges in O(log n) rounds instead
    of one per block. A block qualifies when truncating it saves more than ``preserve_chars`` characters,
    so each call either shrinks the history by a meaningful amount or reports that nothing is left.

    Args:
        messages: The conversation history, modified in place.
        preserve_chars: Characters kept at each end of a truncated text.

    Returns:
        The number of blocks truncated.
    """
    if preserve_chars <= 0:
        return 0

    budget = max(1, sum(1 for message in messages for block in _text_blocks(message) if _is_truncated(block)))
    return sum(_truncate_largest(messages, preserve_chars) for _ in range(budget))


def _is_truncated(block: ContentBlock | ToolResultContent) -> bool:
    return _TRUNCATION_MARKER.search(block["text"]) is not None


def _truncate_largest(messages: Messages, preserve_chars: int) -> bool:
    candidates = (
        block
        for index, message in enumerate(messages)
        if not is_pinned(messages, index)
        for block in _text_blocks(message)
        if not _is_truncated(block)
    )
    largest = max(candidates, key=lambda block: len(block["text"]), default=None)
    if largest is None:
        return False

    text = largest["text"]
    removed = len(text) - 2 * preserve_chars
    if removed <= preserve_chars:
        return False
    marker = f"...\n\n... [truncated: {removed} chars removed] ...\n\n..."
    largest["text"] = f"{text[:preserve_chars]}{marker}{text[-preserve_chars:]}"
    return True
