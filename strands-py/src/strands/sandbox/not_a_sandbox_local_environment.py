"""Host execution environment used as the default when no sandbox is configured.

:class:`NotASandboxLocalEnvironment` runs commands, code, and file operations
directly on the host with **no isolation**. The deliberately blunt name (mirrored
from ``strands-ts/src/sandbox/not-a-sandbox-local-environment.ts``) is a warning:
this is the fallback an :class:`~strands.agent.agent.Agent` uses when no sandbox
is passed, not a security boundary.

Mirroring the TypeScript oracle, this extends :class:`~strands.sandbox.base.Sandbox`
directly: file operations use **native** :mod:`pathlib`/:mod:`os` calls (avoiding a
shell and reporting real ``size`` metadata), while command and code execution spawn a
local ``sh``.
"""

import base64
import contextlib
import os
import shlex
import sys
import tempfile
import uuid
from collections.abc import AsyncGenerator
from pathlib import Path
from typing import Any

from .base import Sandbox
from .constants import LANGUAGE_PATTERN
from .errors import SandboxPathNotFoundError
from .posix_shell import build_shell_env_prefix, validate_env_keys
from .stream_process import _stream_process
from .types import ExecutionResult, FileInfo, StreamChunk

_POWERSHELL_ARGS = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]


def _host_env_with(extra: dict[str, str] | None) -> dict[str, str]:
    """The host environment with ``extra`` applied, matching names case-insensitively as Windows does."""
    env = dict(os.environ)
    for key, value in (extra or {}).items():
        for existing in [name for name in env if name.lower() == key.lower()]:
            del env[existing]
        env[key] = value
    return env


def _powershell_script(command: str) -> str:
    """Wrap ``command`` for PowerShell: UTF-8 in and out, and an exit code that mirrors ``sh -c``.

    The exit code is the last statement's success, or the failing native command's own code.
    ``$ErrorActionPreference`` is left alone because ``Stop`` turns native stderr output into a
    terminating error on PowerShell 5.1.
    """
    return "\n".join(
        [
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8",
            "$OutputEncoding = [System.Text.UTF8Encoding]::new($false)",
            command,
            "$ok = $?",
            "if (-not $ok) { if ($LASTEXITCODE) { exit $LASTEXITCODE } else { exit 1 } }",
        ]
    )


class NotASandboxLocalEnvironment(Sandbox):
    """Run commands, code, and file operations on the host with no isolation.

    Used as the default execution environment when an :class:`Agent` is created
    without a ``sandbox``. Command and code execution spawn a local ``sh``; file
    operations use the host filesystem directly.

    .. warning::
        This provides **no isolation**. Commands run with the full privileges of
        the host process. Pass an explicit sandbox (e.g.
        :class:`~strands.sandbox.docker.DockerSandbox`) when isolation matters.
    """

    @staticmethod
    def _resolve_path(path: str) -> Path:
        """Resolve ``path`` against the current working directory if relative."""
        return Path(path if os.path.isabs(path) else os.path.join(os.getcwd(), path))

    async def execute_streaming(
        self,
        command: str,
        *,
        timeout: float | None = None,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        **kwargs: Any,
    ) -> AsyncGenerator[StreamChunk | ExecutionResult, None]:
        """Execute a command on the host in its shell (``sh -c``; PowerShell on Windows), streaming output.

        Args:
            command: The shell command to execute.
            timeout: Maximum execution time in seconds. ``None`` means no timeout.
            cwd: Working directory for this command. Defaults to the process's
                current working directory.
            env: Environment variables to set on top of the host environment.
            **kwargs: Additional keyword arguments for forward compatibility.

        Yields:
            :class:`StreamChunk` objects for output, then a final
            :class:`ExecutionResult`.

        Raises:
            ValueError: If an environment variable name is invalid.
            SandboxTimeoutError: If execution exceeds ``timeout`` seconds.
        """
        target_cwd = cwd if cwd is not None else os.getcwd()
        if sys.platform == "win32":
            # No ``sh`` on Windows: run the command in PowerShell from a temp script (no command-line length
            # cap, no quoting layer); cwd and env go through the process rather than a shell prefix.
            if env:
                validate_env_keys(env)
            fd, script = tempfile.mkstemp(prefix="strands-shell-", suffix=".ps1")
            # The BOM makes PowerShell 5.1 read the file as UTF-8 rather than the ANSI code page.
            with os.fdopen(fd, "w", encoding="utf-8-sig") as handle:
                handle.write(_powershell_script(command))
            try:
                async for chunk in _stream_process(
                    "powershell.exe",
                    [*_POWERSHELL_ARGS, script],
                    timeout=timeout,
                    cwd=target_cwd,
                    env=_host_env_with(env),
                    enoent_message="PowerShell not found: powershell.exe is required to run commands on Windows",
                ):
                    yield chunk
            finally:
                with contextlib.suppress(OSError):
                    os.unlink(script)
            return
        env_prefix = build_shell_env_prefix(env)
        full_command = f"cd {shlex.quote(target_cwd)} && {env_prefix}{command}"
        async for chunk in _stream_process("sh", ["-c", full_command], timeout=timeout):
            yield chunk

    async def execute_code_streaming(
        self,
        code: str,
        language: str,
        *,
        timeout: float | None = None,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        **kwargs: Any,
    ) -> AsyncGenerator[StreamChunk | ExecutionResult, None]:
        """Execute code on the host by feeding it to a language interpreter on stdin.

        On POSIX the code is base64-encoded and decoded inside a quoted heredoc, then piped
        to the interpreter (``base64 -d << 'EOF' | <lang>``), so arbitrary source — including
        shell metacharacters, quotes, and newlines — reaches the interpreter without injection
        risk. On Windows the interpreter is spawned directly with the code on stdin. ``language``
        is validated against :data:`~strands.sandbox.constants.LANGUAGE_PATTERN` first.

        Args:
            code: The source code to execute.
            language: The interpreter to use (e.g., ``"python3"``, ``"node"``).
            timeout: Maximum execution time in seconds. ``None`` means no timeout.
            cwd: Working directory for execution. Defaults to the process's current
                working directory.
            env: Environment variables to set on top of the host environment.
            **kwargs: Additional keyword arguments for forward compatibility.

        Yields:
            :class:`StreamChunk` objects for output, then a final
            :class:`ExecutionResult`.

        Raises:
            ValueError: If ``language`` contains invalid characters or an environment
                variable name is invalid.
            SandboxTimeoutError: If execution exceeds ``timeout`` seconds.
        """
        if not LANGUAGE_PATTERN.fullmatch(language):
            raise ValueError(f"language parameter contains invalid characters: {language}")
        if sys.platform == "win32":
            # No shell involved: the interpreter is spawned directly and reads the code from stdin.
            if env:
                validate_env_keys(env)
            async for chunk in _stream_process(
                language,
                [],
                timeout=timeout,
                cwd=cwd if cwd is not None else os.getcwd(),
                env=_host_env_with(env),
                input=code,
                enoent_message=f"Language interpreter not found: {language}",
            ):
                yield chunk
            return
        encoded = base64.b64encode(code.encode()).decode("ascii")
        eof = f"STRANDS_EOF_{uuid.uuid4().hex[:16]}"
        command = f"base64 -d << '{eof}' | {language}\n{encoded}\n{eof}"
        async for chunk in self.execute_streaming(command, timeout=timeout, cwd=cwd, env=env, **kwargs):
            yield chunk

    async def read_file(self, path: str, **kwargs: Any) -> bytes:
        """Read a file from the host filesystem as raw bytes.

        Args:
            path: Path to the file. Relative paths resolve against the current
                working directory.
            **kwargs: Additional keyword arguments for forward compatibility.

        Returns:
            The file contents as raw bytes.

        Raises:
            FileNotFoundError: If the file does not exist.
            OSError: If the file cannot be read.
        """
        return self._resolve_path(path).read_bytes()

    async def write_file(self, path: str, content: bytes, **kwargs: Any) -> None:
        """Write raw bytes to a file on the host, creating parent directories.

        Args:
            path: Path to the file. Relative paths resolve against the current
                working directory.
            content: The content to write.
            **kwargs: Additional keyword arguments for forward compatibility.

        Raises:
            OSError: If the file cannot be written.
        """
        full_path = self._resolve_path(path)
        full_path.parent.mkdir(parents=True, exist_ok=True)
        full_path.write_bytes(content)

    async def remove_file(self, path: str, **kwargs: Any) -> None:
        """Remove a file from the host filesystem.

        Args:
            path: Path to the file. Relative paths resolve against the current
                working directory.
            **kwargs: Additional keyword arguments for forward compatibility.

        Raises:
            FileNotFoundError: If the file does not exist.
        """
        self._resolve_path(path).unlink()

    async def list_files(self, path: str, **kwargs: Any) -> list[FileInfo]:
        """List directory contents from the host filesystem, sorted by name.

        Unlike the shell-based base implementation, this reports native ``is_dir``
        and ``size`` metadata. If an entry's metadata cannot be read, it is still
        listed with ``is_dir``/``size`` left as ``None``.

        Args:
            path: Path to the directory. Relative paths resolve against the
                current working directory.
            **kwargs: Additional keyword arguments for forward compatibility.

        Returns:
            A list of :class:`FileInfo` entries for the directory contents.

        Raises:
            SandboxPathNotFoundError: If the directory does not exist or ``path``
                is not a directory. Permission and other errors propagate so
                callers can surface them.
        """
        full_path = self._resolve_path(path)
        results: list[FileInfo] = []
        try:
            scanner = os.scandir(full_path)
        except (FileNotFoundError, NotADirectoryError) as e:
            # A missing path (or a file where a directory was expected) is non-existence;
            # permission and other errors propagate so callers can surface them.
            raise SandboxPathNotFoundError(path) from e
        with scanner as entries:
            for entry in sorted(entries, key=lambda e: e.name):
                try:
                    stat = entry.stat()
                    results.append(FileInfo(name=entry.name, is_dir=entry.is_dir(), size=stat.st_size))
                except OSError:
                    results.append(FileInfo(name=entry.name))
        return results
