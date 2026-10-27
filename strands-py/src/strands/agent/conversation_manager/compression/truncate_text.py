"""Last-resort text truncation for when summarization cannot reduce the context."""

from collections.abc import Iterator

from ....types.content import ContentBlock, Message, Messages
from ....types.tools import ToolResultContent
from .pin_message import is_pinned

_TRUNCATION_MARKER = "... [truncated:"
_PRESERVE_CHARS = 2_000


def _text_blocks(message: Message) -> Iterator[ContentBlock | ToolResultContent]:
    """Yield every dict carrying a ``text`` key: content blocks and items inside toolResult content."""
    for content in message.get("content", []):
        if "text" in content:
            yield content
        for item in content.get("toolResult", {}).get("content", []):
            if "text" in item:
                yield item


def truncate_largest_text_block(messages: Messages, preserve_chars: int = _PRESERVE_CHARS) -> bool:
    """Replace the largest untruncated text block in a non-pinned message with a head+tail preview.

    Each call strictly shrinks the history and never touches an already truncated block, so a caller that
    retries until this returns False is bounded.

    Args:
        messages: The conversation history, modified in place.
        preserve_chars: Characters kept at each end of the truncated text.

    Returns:
        True if a block was truncated, False if nothing is left to truncate.
    """
    candidates = [
        block
        for index, message in enumerate(messages)
        if not is_pinned(messages, index)
        for block in _text_blocks(message)
        if _TRUNCATION_MARKER not in block["text"]
    ]
    if not candidates:
        return False

    largest = max(candidates, key=lambda block: len(block["text"]))
    text = largest["text"]
    removed = len(text) - 2 * preserve_chars
    marker = f"...\n\n{_TRUNCATION_MARKER} {removed} chars removed] ...\n\n..."
    if removed <= len(marker):
        return False

    largest["text"] = f"{text[:preserve_chars]}{marker}{text[-preserve_chars:]}"
    return True
