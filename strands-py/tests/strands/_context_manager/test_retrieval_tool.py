"""Tests for the retrieve_context tool."""

import json

import pytest

from strands._context_manager.retrieval_tool import (
    RETRIEVAL_TOOL_NAME,
    _create_retrieval_tool,
    _extract_text,
    _render_shell_result,
    _track_retrieval_tool_use_ids,
)
from strands._context_manager.stash import Stash
from strands.storage.in_memory_storage import InMemoryStorage
from strands.types.content import ContentBlock, Message
from strands.types.tools import ToolUse


@pytest.fixture
def stash():
    return Stash(InMemoryStorage(), "test-session", "test-agent")


async def _store_text(stash, text):
    data = json.dumps({"text": text}).encode("utf-8")
    return await stash.store("tool-1", 0, data)


class TestRetrievalTool:
    """Tests for the retrieve_context tool."""

    def test_has_correct_name(self, stash):
        tool = _create_retrieval_tool(stash)
        assert tool.tool_name == RETRIEVAL_TOOL_NAME

    @pytest.mark.asyncio
    async def test_retrieves_full_content(self, stash):
        ref = await _store_text(stash, "hello world\nline two")
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        content_text = result["content"][0]["text"]
        parsed = json.loads(content_text)
        assert parsed == {"text": "hello world\nline two"}

    @pytest.mark.asyncio
    async def test_searches_with_pattern(self, stash):
        lines = "\n".join(f"line {i}: {'ERROR' if i % 3 == 0 else 'ok'}" for i in range(1, 21))
        ref = await _store_text(stash, lines)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref, "pattern": "ERROR"}})
        assert result["status"] == "success"
        assert "match" in result["content"][0]["text"].lower()

    @pytest.mark.asyncio
    async def test_returns_line_range(self, stash):
        lines = "\n".join(f"line {i}" for i in range(1, 11))
        ref = await _store_text(stash, lines)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func(
            {"toolUseId": "t1", "input": {"reference": ref, "line_range": {"start": 3, "end": 5}}}
        )
        assert result["status"] == "success"
        text = result["content"][0]["text"]
        assert "line 3" in text
        assert "line 5" in text

    @pytest.mark.asyncio
    async def test_returns_error_for_unknown_reference(self, stash):
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": "nonexistent"}})
        assert result["status"] == "error"
        assert "not found" in result["content"][0]["text"]

    @pytest.mark.asyncio
    async def test_retrieves_json_content(self, stash):
        data = json.dumps({"json": {"key": "value"}}).encode("utf-8")
        ref = await stash.store("tool-1", 0, data)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        parsed = json.loads(result["content"][0]["text"])
        assert parsed == {"json": {"key": "value"}}

    @pytest.mark.asyncio
    async def test_returns_error_for_non_text_content_with_pattern(self, stash):
        data = json.dumps({"image": {"format": "png", "source": "bytes"}}).encode("utf-8")
        ref = await stash.store("tool-1", 0, data)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref, "pattern": "foo"}})
        assert result["status"] == "error"
        assert "cannot search non-text content" in result["content"][0]["text"]

    @pytest.mark.asyncio
    async def test_returns_error_for_invalid_line_range(self, stash):
        ref = await _store_text(stash, "line 1\nline 2\nline 3")
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func(
            {"toolUseId": "t1", "input": {"reference": ref, "line_range": {"start": 100, "end": 5}}}
        )
        assert result["status"] == "error"


class TestExtractText:
    """Tests for _extract_text helper."""

    def test_extracts_string(self):
        assert _extract_text("hello") == "hello"

    def test_extracts_text_field(self):
        assert _extract_text({"text": "hello"}) == "hello"

    def test_extracts_json_field(self):
        result = _extract_text({"json": {"key": "val"}})
        assert result is not None
        assert "key" in result

    def test_returns_none_for_non_text(self):
        assert _extract_text({"image": "data"}) is None

    def test_returns_none_for_non_dict(self):
        assert _extract_text(42) is None


class TestShellResultRetrieval:
    """Shell-style ``{output, error, exit_code}`` results are rendered as text, stdout first (#4741)."""

    @staticmethod
    async def _store_shell(stash, output, error="", exit_code=0):
        # The @tool wrapper serializes a dict return into a single-line JSON text block.
        shell_json = json.dumps({"output": output, "error": error, "exit_code": exit_code})
        return await stash.store("shell-1", 0, json.dumps({"text": shell_json}).encode("utf-8"))

    @pytest.mark.asyncio
    async def test_pattern_matches_stdout_line_by_number(self, stash):
        output = "\n".join("Call me Ishmael." if i == 861 else f"line {i}" for i in range(1, 2001))
        ref = await self._store_shell(stash, output)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref, "pattern": "Ishmael"}})
        assert result["status"] == "success"
        text = result["content"][0]["text"]
        assert "[1 match" in text
        assert "> 861| Call me Ishmael." in text
        assert "\\n" not in text

    @pytest.mark.asyncio
    async def test_line_range_indexes_stdout_lines(self, stash):
        output = "\n".join(f"line {i}" for i in range(1, 2001)) + "\n"
        ref = await self._store_shell(stash, output)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func(
            {"toolUseId": "t1", "input": {"reference": ref, "line_range": {"start": 861, "end": 865}}}
        )
        assert result["status"] == "success"
        text = result["content"][0]["text"]
        assert "861| line 861" in text
        assert "865| line 865" in text
        assert "line 866" not in text

    @pytest.mark.asyncio
    async def test_full_retrieval_returns_stdout_then_stderr_and_exit_code(self, stash):
        ref = await self._store_shell(stash, "out 1\nout 2\n", error="boom\nmore", exit_code=2)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        assert result["content"][0]["text"] == "out 1\nout 2\n\n[stderr]\nboom\nmore\n[exit_code: 2]"

    @pytest.mark.asyncio
    async def test_stderr_is_searchable_when_stdout_is_empty(self, stash):
        ref = await self._store_shell(stash, "", error="\n".join(f"err {i}" for i in range(1, 51)), exit_code=1)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref, "pattern": "err 42"}})
        assert result["status"] == "success"
        text = result["content"][0]["text"]
        assert "[1 match" in text
        assert "> 44| err 42\n" in text  # line 1 is the empty stdout, line 2 the [stderr] header

    @pytest.mark.asyncio
    async def test_omits_stderr_section_when_empty(self, stash):
        ref = await self._store_shell(stash, "ok")
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["content"][0]["text"] == "ok\n[exit_code: 0]"

    @pytest.mark.asyncio
    async def test_full_retrieval_clips_stdout_but_keeps_stderr_and_exit_code(self, stash):
        ref = await self._store_shell(stash, "x" * 50_000, error="boom", exit_code=1)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        text = result["content"][0]["text"]
        assert text == "x" * 39_996 + "\n\n[... 10,004 chars of stdout truncated]\n[stderr]\nboom\n[exit_code: 1]"

    @pytest.mark.asyncio
    async def test_full_retrieval_clips_stderr_to_a_quarter_of_the_budget(self, stash):
        ref = await self._store_shell(stash, "ok", error="e" * 50_000, exit_code=1)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        text = result["content"][0]["text"]
        assert text == "ok\n[stderr]\n" + "e" * 10_000 + "\n\n[... 40,000 chars of stderr truncated]\n[exit_code: 1]"

    def test_renders_json_block_form(self):
        rendered = _render_shell_result({"json": {"output": "a\nb", "error": "", "exit_code": 0}})
        assert rendered == "a\nb\n[exit_code: 0]"

    @pytest.mark.parametrize(
        "data",
        [
            {"text": json.dumps({"output": "a", "error": "", "exit_code": 0, "host": "db-1"})},
            {"text": json.dumps({"output": "a", "error": ""})},
            {"text": json.dumps({"output": "a", "error": "", "exit_code": "0"})},
            {"text": json.dumps({"output": "a", "error": "", "exit_code": True})},
            {"text": json.dumps({"output": ["a"], "error": "", "exit_code": 0})},
            {"text": "not json at all {"},
            {"text": "[1, 2, 3]"},
        ],
    )
    def test_other_shapes_are_not_rendered(self, data):
        assert _render_shell_result(data) is None


class TestFullTextTruncation:
    """Tests for full-content retrieval truncation."""

    @pytest.mark.asyncio
    async def test_truncates_large_full_content(self, stash):
        ref = await _store_text(stash, "x" * 50_000)
        tool = _create_retrieval_tool(stash, max_result_tokens=10)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        text = result["content"][0]["text"]
        assert text.endswith("\n\n[truncated]")


class TestMediaBlockRetrieval:
    """Tests for media block retrieval (image/document/video)."""

    @pytest.mark.asyncio
    async def test_restores_image_block(self, stash):
        data = json.dumps({"image": {"format": "png", "source": {"bytes": "UE5HX0RBVEE="}}}).encode("utf-8")
        ref = await stash.store("tool-1", 0, data)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        assert "image" in result["content"][0]
        assert result["content"][0]["image"]["format"] == "png"
        assert result["content"][0]["image"]["source"]["bytes"] == b"PNG_DATA"

    @pytest.mark.asyncio
    async def test_restores_document_block(self, stash):
        data = json.dumps({"document": {"format": "pdf", "source": {"bytes": "UERGX0RBVEE="}}}).encode("utf-8")
        ref = await stash.store("tool-1", 0, data)
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        assert "document" in result["content"][0]
        assert result["content"][0]["document"]["format"] == "pdf"
        assert result["content"][0]["document"]["source"]["bytes"] == b"PDF_DATA"

    @pytest.mark.asyncio
    async def test_returns_text_content_normally(self, stash):
        ref = await _store_text(stash, "hello world")
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref}})
        assert result["status"] == "success"
        assert "hello world" in result["content"][0]["text"]


class TestInvalidLineRange:
    """Tests for malformed line_range input."""

    @pytest.mark.asyncio
    async def test_returns_error_for_missing_start_key(self, stash):
        ref = await _store_text(stash, "line 1\nline 2")
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func({"toolUseId": "t1", "input": {"reference": ref, "line_range": {"end": 2}}})
        assert result["status"] == "error"
        assert "invalid line_range" in result["content"][0]["text"]

    @pytest.mark.asyncio
    async def test_returns_error_for_non_numeric_values(self, stash):
        ref = await _store_text(stash, "line 1\nline 2")
        tool = _create_retrieval_tool(stash)
        result = await tool._tool_func(
            {"toolUseId": "t1", "input": {"reference": ref, "line_range": {"start": "abc", "end": 2}}}
        )
        assert result["status"] == "error"
        assert "invalid line_range" in result["content"][0]["text"]


class TestTrackRetrievalToolUseIds:
    """Tests for _track_retrieval_tool_use_ids."""

    def test_tracks_retrieve_context_tool_use_ids(self):
        skip_set: set[str] = set()
        message = Message(
            role="assistant",
            content=[
                ContentBlock(toolUse=ToolUse(toolUseId="tu-ret", name=RETRIEVAL_TOOL_NAME, input={})),
            ],
        )
        _track_retrieval_tool_use_ids(message, skip_set)
        assert "tu-ret" in skip_set

    def test_ignores_non_retrieval_tool_uses(self):
        skip_set: set[str] = set()
        message = Message(
            role="assistant",
            content=[
                ContentBlock(toolUse=ToolUse(toolUseId="tu-other", name="bash", input={})),
            ],
        )
        _track_retrieval_tool_use_ids(message, skip_set)
        assert len(skip_set) == 0

    def test_ignores_user_messages(self):
        skip_set: set[str] = set()
        message = Message(role="user", content=[ContentBlock(text="hello")])
        _track_retrieval_tool_use_ids(message, skip_set)
        assert len(skip_set) == 0
