"""Shared types and constants for the shell tool."""

import json
from typing import TypedDict

from ...sandbox.errors import SandboxTimeoutError


class ShellOutput(TypedDict):
    """Output of a shell command execution.

    Attributes:
        output: Standard output captured from the command.
        error: Standard error captured from the command. Empty when there was none.
        exit_code: Exit code of the command. Non-zero means the command failed.
    """

    output: str
    error: str
    exit_code: int


class ShellTimeoutError(SandboxTimeoutError):
    """Raised when a sandbox-routed shell command exceeds its timeout.

    Subclasses :class:`SandboxTimeoutError` so existing handlers keep working. The output
    captured before the kill is on ``partial`` and appended to the message as JSON, so the
    model still sees it on the failed tool result. Mirrors ``ShellTimeoutError`` in
    ``strands-ts/src/vended-tools/shell/types.ts``.
    """

    def __init__(self, seconds: float | None, stdout: str = "", stderr: str = "") -> None:
        """Initialize from the timeout duration and the output captured so far.

        Args:
            seconds: The timeout duration, in seconds, that elapsed.
            stdout: Standard output captured before the kill.
            stderr: Standard error captured before the kill.
        """
        super().__init__(seconds, stdout, stderr)
        # 124 is the timeout(1) convention for a command killed by its time limit.
        self.partial: ShellOutput = {"output": stdout, "error": stderr, "exit_code": 124}
        self.args = (f"{self.args[0]}\n{json.dumps(self.partial)}",)


class ShellExecutionError(RuntimeError):
    """Raised when a sandbox-routed shell command fails.

    Subclasses :class:`RuntimeError` so existing ``except RuntimeError`` handlers
    keep working, while giving callers a shell-specific type to branch on. Mirrors
    ``ShellExecutionError`` in ``strands-ts/src/vended-tools/shell/types.ts``.
    """


SANDBOX_SHELL_DESCRIPTION = (
    "Executes shell commands and returns output (stdout), error (stderr), and exit_code (non-zero means the "
    "command failed). Each call runs in a fresh shell; "
    "state such as variables and the working directory does not persist across calls."
)
"""Description for the shell tool."""
