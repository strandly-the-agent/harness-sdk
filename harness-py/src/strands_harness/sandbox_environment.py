"""A sandbox that knows where it runs can say so via a duck-typed ``environment`` attribute
(``platform``/``cwd``/``shell``), as the CLI's workspace sandbox does. Consumers prefer it over probing
with shell commands, which have no meaning on a host without a POSIX shell."""

from __future__ import annotations

from typing import Any


def described_environment(sandbox: Any) -> dict[str, str | None]:
    """Return ``platform``/``cwd``/``shell`` from ``sandbox.environment``; missing or malformed values are ``None``."""
    described = getattr(sandbox, "environment", None)

    def pick(key: str) -> str | None:
        value = described.get(key) if isinstance(described, dict) else getattr(described, key, None)
        return value if isinstance(value, str) and value else None

    return {"platform": pick("platform"), "cwd": pick("cwd"), "shell": pick("shell")}
