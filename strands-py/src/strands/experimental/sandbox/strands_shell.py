"""Strands Shell sandbox -- runs commands in an in-process virtual shell.

`Strands Shell <https://github.com/strands-agents/shell>`_ is a Bourne-compatible
shell with its own virtual filesystem, URL allowlist, and credential injection.
It runs in-process with no fork/exec, so commands start in under a millisecond
and only the host paths you bind are reachable.

Requires the ``strands-shell`` extra: ``pip install 'strands-agents[strands-shell]'``.
"""

import asyncio
import functools
import logging
import queue
import shlex
import threading
import uuid
import weakref
from collections.abc import AsyncGenerator, Callable
from concurrent.futures import Future
from typing import TYPE_CHECKING, Any, TypeAlias, TypeVar

from ...sandbox.base import Sandbox
from ...sandbox.constants import LANGUAGE_PATTERN
from ...sandbox.errors import SandboxPathNotFoundError
from ...sandbox.posix_shell import build_shell_env_prefix
from ...sandbox.types import ExecutionResult, FileInfo, StreamChunk
from ...types.tools import AgentTool
from ...vended_tools.file_editor import make_file_editor
from ...vended_tools.file_editor.file_editor import DEFAULT_FILE_EDITOR_DESCRIPTION
from ...vended_tools.shell import make_shell
from ...vended_tools.shell.types import SANDBOX_SHELL_DESCRIPTION

if TYPE_CHECKING:
    import strands_shell

logger = logging.getLogger(__name__)

T = TypeVar("T")
_RequestQueue: TypeAlias = "queue.SimpleQueue[tuple[Callable[[Any], Any], Future[Any]] | None]"


def _serve_shell(make_shell: "Callable[[], strands_shell.Shell]", requests: _RequestQueue) -> None:
    """Own the native shell for the worker thread's lifetime, serving queued operations until ``None``."""
    shell = make_shell()
    while (request := requests.get()) is not None:
        operation, future = request
        try:
            future.set_result(operation(shell))
        except BaseException as error:  # noqa: BLE001 - pyo3 panics are BaseExceptions
            future.set_exception(error)


class StrandsShellSandbox(Sandbox):
    """Execute commands and file operations in an in-process Strands Shell.

    Constructor arguments mirror :class:`strands_shell.Shell`. Each command runs in
    a subshell, so working directory and variable changes do not leak between
    calls. File operations go straight to the shell's virtual filesystem; host
    paths are visible only through ``binds``.

    The per-command ``timeout`` on ``execute``/``execute_code`` is not supported by
    the native shell and is ignored; use the constructor's ``timeout`` (Strands
    Shell defaults to 30 seconds). A timed-out command returns a normal result
    with a non-zero exit code and the shell's timeout message on stderr.

    The native shell object may only be used (and dropped) on the thread that
    created it, so it lives on a dedicated worker thread owned by this sandbox and
    every call is dispatched there. The native call holds the GIL for its duration.
    """

    def __init__(
        self,
        *,
        binds: "list[strands_shell.Bind] | None" = None,
        credentials: "list[strands_shell.Cred] | None" = None,
        allowed_urls: list[str] | None = None,
        env: dict[str, str] | None = None,
        umask: int | None = None,
        timeout: float | None = None,
        limits: "strands_shell.Limits | None" = None,
        config_file: str | None = None,
    ) -> None:
        """Initialize the sandbox; the underlying shell is created on first use.

        Args:
            binds: Host paths to mount into the virtual filesystem.
            credentials: Credential injection rules, applied per URL on outbound requests.
            allowed_urls: URL prefixes commands such as ``curl`` may reach.
            env: Environment variables seeded into the shell.
            umask: File-creation mask for the virtual filesystem.
            timeout: Per-command wall-clock timeout in seconds. ``None`` uses the
                Strands Shell default.
            limits: Resource caps (output size, file size, inodes, ...).
            config_file: Path to a Strands Shell TOML config; explicit arguments win.

        Raises:
            ImportError: If the ``strands-shell`` extra is not installed.
        """
        try:
            import strands_shell
        except ImportError as error:
            raise ImportError(
                "StrandsShellSandbox requires the 'strands-shell' extra. "
                "Install with: pip install 'strands-agents[strands-shell]'"
            ) from error

        self._strands_shell = strands_shell
        self._binds = binds or []
        self._shell_kwargs: dict[str, Any] = {
            "binds": binds,
            "credentials": credentials,
            "allowed_urls": allowed_urls,
            "env": env,
            "umask": umask,
            "timeout": timeout,
            "limits": limits,
            "config_file": config_file,
        }
        self._requests: _RequestQueue = queue.SimpleQueue()
        self._worker: threading.Thread | None = None
        # Stop the worker (and drop the shell on its own thread) once the sandbox is collected.
        weakref.finalize(self, self._requests.put, None)

    async def _call(self, operation: "Callable[[strands_shell.Shell], T]") -> T:
        """Run ``operation`` against the native shell on the worker thread."""
        if self._worker is None:
            # The worker must not reference self, or the sandbox could never be collected.
            make_shell = functools.partial(self._strands_shell.Shell, **self._shell_kwargs)
            self._worker = threading.Thread(
                target=_serve_shell, args=(make_shell, self._requests), name="strands-shell", daemon=True
            )
            self._worker.start()
        future: Future[T] = Future()
        self._requests.put((operation, future))
        return await asyncio.wrap_future(future)

    async def execute_streaming(
        self,
        command: str,
        *,
        timeout: float | None = None,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        **kwargs: Any,
    ) -> AsyncGenerator[StreamChunk | ExecutionResult, None]:
        """Execute a command in a subshell, yielding its output once it completes.

        The native shell buffers output, so this yields at most one stdout chunk and
        one stderr chunk, then the final :class:`ExecutionResult`.

        Args:
            command: The shell command to execute.
            timeout: Ignored; see the class docstring.
            cwd: Working directory for this command.
            env: Environment variables exported for this command only.
            **kwargs: Additional keyword arguments for forward compatibility.

        Yields:
            :class:`StreamChunk` objects for output, then a final
            :class:`ExecutionResult`.

        Raises:
            ValueError: If an environment variable name is invalid.
        """
        if timeout is not None:
            logger.debug("timeout=<%s> | per-command timeout ignored by StrandsShellSandbox", timeout)
        prefix = build_shell_env_prefix(env)
        if cwd is not None:
            prefix += f"cd {shlex.quote(cwd)} && "
        output = await self._call(lambda shell: shell.run(f"( {prefix}{command}\n)"))

        if output.stdout:
            yield StreamChunk(data=output.stdout, stream_type="stdout")
        if output.stderr:
            yield StreamChunk(data=output.stderr, stream_type="stderr")
        yield ExecutionResult(exit_code=output.status, stdout=output.stdout, stderr=output.stderr)

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
        """Execute code by writing it to a temporary file and running ``<language> <file>``.

        Strands Shell ships Lua (``language="lua"``); other interpreters are only
        available if the shell exposes them.

        Args:
            code: The source code to execute.
            language: The interpreter to use (e.g. ``"lua"``).
            timeout: Ignored; see the class docstring.
            cwd: Working directory for execution.
            env: Environment variables exported for this execution only.
            **kwargs: Additional keyword arguments for forward compatibility.

        Yields:
            :class:`StreamChunk` objects for output, then a final
            :class:`ExecutionResult`.

        Raises:
            ValueError: If ``language`` contains invalid characters.
        """
        if not LANGUAGE_PATTERN.fullmatch(language):
            raise ValueError(f"language parameter contains invalid characters: {language}")
        script_path = f"/tmp/strands-code-{uuid.uuid4().hex}"
        await self.write_file(script_path, code.encode())
        # Output is buffered by the native shell, so collecting before yielding loses nothing and
        # guarantees cleanup even when the consumer stops at the final ExecutionResult.
        try:
            stream = self.execute_streaming(f"{language} {script_path}", timeout=timeout, cwd=cwd, env=env, **kwargs)
            chunks = [chunk async for chunk in stream]
        finally:
            await self._call(lambda shell: shell.remove_file(script_path))
        for chunk in chunks:
            yield chunk

    async def read_file(self, path: str, **kwargs: Any) -> bytes:
        """Read a file from the virtual filesystem.

        Args:
            path: Path to the file to read.
            **kwargs: Additional keyword arguments for forward compatibility.

        Returns:
            The file contents as raw bytes.

        Raises:
            FileNotFoundError: If the file does not exist.
            OSError: If the path cannot be read (e.g. it is a directory).
        """
        try:
            return await self._call(lambda shell: shell.read_file(path))
        except self._strands_shell.ShellError as error:
            raise self._as_os_error(error) from error

    async def write_file(self, path: str, content: bytes, **kwargs: Any) -> None:
        """Write a file to the virtual filesystem.

        Args:
            path: Path to the file to write.
            content: The raw bytes to write.
            **kwargs: Additional keyword arguments for forward compatibility.

        Raises:
            OSError: If the file cannot be written (read-only mount, size cap, missing parent).
        """
        try:
            await self._call(lambda shell: shell.write_file(path, content))
        except self._strands_shell.ShellError as error:
            raise self._as_os_error(error) from error

    async def remove_file(self, path: str, **kwargs: Any) -> None:
        """Remove a file from the virtual filesystem.

        Args:
            path: Path to the file to remove.
            **kwargs: Additional keyword arguments for forward compatibility.

        Raises:
            FileNotFoundError: If the file does not exist.
            OSError: If the file cannot be removed.
        """
        try:
            await self._call(lambda shell: shell.remove_file(path))
        except self._strands_shell.ShellError as error:
            raise self._as_os_error(error) from error

    async def list_files(self, path: str, **kwargs: Any) -> list[FileInfo]:
        """List a directory in the virtual filesystem.

        Args:
            path: Path to the directory to list.
            **kwargs: Additional keyword arguments for forward compatibility.

        Returns:
            :class:`FileInfo` entries with name, ``is_dir``, and ``size``.

        Raises:
            SandboxPathNotFoundError: If the directory does not exist or ``path`` is not a directory.
            OSError: If the listing fails for another reason.
        """
        try:
            entries = await self._call(lambda shell: shell.list_files(path))
        except self._strands_shell.FileNotFoundError as error:
            raise SandboxPathNotFoundError(path) from error
        except self._strands_shell.ShellError as error:
            if "not a directory" in error.message.lower():
                raise SandboxPathNotFoundError(path) from error
            raise self._as_os_error(error) from error
        return sorted(
            (FileInfo(name=entry.name, is_dir=entry.is_dir, size=entry.size) for entry in entries),
            key=lambda info: info.name,
        )

    @staticmethod
    def _as_os_error(error: "strands_shell.ShellError") -> OSError:
        """Typed shell errors already subclass OSError; wrap the generic kind so callers can ``except OSError``."""
        return error if isinstance(error, OSError) else OSError(error.message)

    def get_tools(self) -> list[AgentTool]:
        """Default sandbox-compatible tools auto-registered with this sandbox.

        Returns:
            The tools bound to this sandbox, with descriptions naming the bound host paths.
        """
        mounts = ", ".join(f"{bind.source} -> {bind.destination}" for bind in self._binds)
        location = f" Host paths mounted: {mounts}." if mounts else " No host paths are mounted."
        return [
            make_file_editor(
                sandbox=self,
                name="sandbox_file_editor",
                description=f"{DEFAULT_FILE_EDITOR_DESCRIPTION} Files are in a Strands Shell filesystem.{location}",
            ),
            make_shell(
                sandbox=self,
                name="sandbox_shell",
                description=f"{SANDBOX_SHELL_DESCRIPTION} Runs in a Strands Shell virtual environment.{location}",
            ),
        ]
