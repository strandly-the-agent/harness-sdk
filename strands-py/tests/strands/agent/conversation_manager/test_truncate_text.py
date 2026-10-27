"""Tests for last-resort text truncation."""

from strands.agent.conversation_manager.compression.pin_message import pin_message
from strands.agent.conversation_manager.compression.truncate_text import truncate_largest_text_block


def _text_message(text, role="user"):
    return {"role": role, "content": [{"text": text}]}


def _tool_result_message(text, tool_use_id="t1"):
    return {
        "role": "user",
        "content": [{"toolResult": {"toolUseId": tool_use_id, "status": "success", "content": [{"text": text}]}}],
    }


class TestTruncateLargestTextBlock:
    def test_truncates_largest_text_block_to_head_and_tail(self):
        messages = [_text_message("small"), _text_message("h" * 100 + "m" * 100 + "t" * 100)]

        assert truncate_largest_text_block(messages, preserve_chars=100) is True

        assert messages[0]["content"][0]["text"] == "small"
        truncated = messages[1]["content"][0]["text"]
        assert truncated == "h" * 100 + "...\n\n... [truncated: 100 chars removed] ...\n\n..." + "t" * 100

    def test_truncates_text_inside_tool_results(self):
        messages = [_tool_result_message("x" * 500)]

        assert truncate_largest_text_block(messages, preserve_chars=10) is True

        text = messages[0]["content"][0]["toolResult"]["content"][0]["text"]
        assert text.startswith("x" * 10 + "...")
        assert "[truncated: 480 chars removed]" in text

    def test_returns_false_when_nothing_is_large_enough(self):
        messages = [_text_message("short"), _tool_result_message("also short")]

        assert truncate_largest_text_block(messages, preserve_chars=100) is False
        assert messages[0]["content"][0]["text"] == "short"

    def test_does_not_retruncate_and_is_bounded(self):
        messages = [_text_message("a" * 1_000), _text_message("b" * 800)]

        assert truncate_largest_text_block(messages, preserve_chars=10) is True
        assert truncate_largest_text_block(messages, preserve_chars=10) is True
        assert truncate_largest_text_block(messages, preserve_chars=10) is False

        assert all("[truncated:" in message["content"][0]["text"] for message in messages)

    def test_skips_pinned_messages(self):
        messages = [_text_message("p" * 1_000), _text_message("q" * 500)]
        pin_message(messages, 0)

        assert truncate_largest_text_block(messages, preserve_chars=10) is True

        assert messages[0]["content"][0]["text"] == "p" * 1_000
        assert "[truncated:" in messages[1]["content"][0]["text"]

    def test_requires_strict_shrink(self):
        # 2 * preserve + 1 chars: a marker would make the text longer, so it is left alone.
        messages = [_text_message("z" * 201)]

        assert truncate_largest_text_block(messages, preserve_chars=100) is False
