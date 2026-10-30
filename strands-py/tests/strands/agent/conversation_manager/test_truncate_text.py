"""Tests for last-resort text truncation."""

from strands.agent.conversation_manager.compression.pin_message import pin_message
from strands.agent.conversation_manager.compression.truncate_text import _truncate_largest_text_blocks


def _text_message(text, role="user"):
    return {"role": role, "content": [{"text": text}]}


def _tool_result_message(text, tool_use_id="t1"):
    return {
        "role": "user",
        "content": [{"toolResult": {"toolUseId": tool_use_id, "status": "success", "content": [{"text": text}]}}],
    }


class TestTruncateLargestTextBlocks:
    def test_truncates_to_head_and_tail(self):
        messages = [_text_message("small"), _text_message("h" * 100 + "m" * 200 + "t" * 100)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=100) == 1

        assert messages[0]["content"][0]["text"] == "small"
        truncated = messages[1]["content"][0]["text"]
        assert truncated == "h" * 100 + "...\n\n... [truncated: 200 chars removed] ...\n\n..." + "t" * 100

    def test_truncates_only_the_largest_block_per_call(self):
        messages = [_text_message("a" * 1_000), _tool_result_message("b" * 3_000), _text_message("c" * 2_000)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1

        assert messages[0]["content"][0]["text"] == "a" * 1_000
        assert "[truncated: 2980 chars removed]" in messages[1]["content"][0]["toolResult"]["content"][0]["text"]
        assert messages[2]["content"][0]["text"] == "c" * 2_000

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1

        assert messages[0]["content"][0]["text"] == "a" * 1_000
        assert "[truncated: 1980 chars removed]" in messages[2]["content"][0]["text"]

    def test_truncates_as_many_blocks_as_already_carry_a_marker(self):
        # Rounds go 1, 1, 2, 4...: a history needing many blocks gone converges in O(log n) retries.
        messages = [_text_message(str(i) * 1_000) for i in range(8)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1
        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1
        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 2
        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 4
        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 0

        assert all("[truncated:" in message["content"][0]["text"] for message in messages)

    def test_ties_go_to_the_oldest_block(self):
        messages = [_text_message("a" * 1_000), _text_message("b" * 1_000)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1

        assert "[truncated:" in messages[0]["content"][0]["text"]
        assert messages[1]["content"][0]["text"] == "b" * 1_000

    def test_requires_a_meaningful_saving(self):
        # Saving must exceed preserve_chars: 3 * preserve is the floor.
        messages = [_text_message("z" * 300), _text_message("y" * 301)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=100) == 1
        assert "[truncated: 101 chars removed]" in messages[1]["content"][0]["text"]

        assert _truncate_largest_text_blocks(messages, preserve_chars=100) == 0
        assert messages[0]["content"][0]["text"] == "z" * 300

    def test_returns_false_when_nothing_qualifies(self):
        messages = [_text_message("short"), _tool_result_message("also short"), {"role": "user", "content": ["raw"]}]

        assert _truncate_largest_text_blocks(messages, preserve_chars=100) == 0

    def test_does_not_retruncate(self):
        messages = [_text_message("a" * 1_000)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1
        once = messages[0]["content"][0]["text"]
        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 0
        assert messages[0]["content"][0]["text"] == once

    def test_user_text_mentioning_truncated_is_still_truncated(self):
        messages = [_text_message("... [truncated: lol] " + "a" * 1_000)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1

    def test_skips_pinned_messages(self):
        messages = [_text_message("p" * 1_000), _text_message("q" * 500)]
        pin_message(messages, 0)

        assert _truncate_largest_text_blocks(messages, preserve_chars=10) == 1

        assert messages[0]["content"][0]["text"] == "p" * 1_000
        assert "[truncated:" in messages[1]["content"][0]["text"]

    def test_non_positive_preserve_chars_is_a_no_op(self):
        messages = [_text_message("a" * 1_000)]

        assert _truncate_largest_text_blocks(messages, preserve_chars=0) == 0
        assert messages[0]["content"][0]["text"] == "a" * 1_000
