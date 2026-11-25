# SPDX-License-Identifier: AGPL-3.0-or-later
"""Interactive console: IPython when available, otherwise IDLE or the standard console."""

from __future__ import annotations

import code
import importlib.util
import os
from typing import Any, Literal

Backend = Literal["ipython", "idle", "python"]


def choose_backend(preferred: str | None = None) -> Backend:
    """Pick the console to use. IPython wins when installed, unless overridden."""
    preferred = preferred or os.environ.get("LIBREBURN_REPL")
    if preferred == "idle":
        return "idle"
    if preferred == "python":
        return "python"
    if importlib.util.find_spec("IPython") is not None:
        return "ipython"
    has_display = bool(os.environ.get("WAYLAND_DISPLAY") or os.environ.get("DISPLAY"))
    if has_display and importlib.util.find_spec("idlelib") is not None:
        return "idle"
    return "python"


def start(namespace: dict[str, Any], preferred: str | None = None) -> None:
    """Start a console with `namespace` as its globals."""
    backend = choose_backend(preferred)
    banner = "libreburn console. Objects: " + ", ".join(sorted(namespace))
    if backend == "ipython":
        from IPython import start_ipython

        start_ipython(argv=[], user_ns=namespace, display_banner=False)
    elif backend == "idle":
        from idlelib import pyshell  # pyright: ignore[reportMissingImports]

        # IDLE cannot take a namespace; it starts with a hint instead.
        print(banner + "\nIn IDLE, run: import libreburn; client = libreburn.Client()")
        pyshell.main()
    else:
        code.interact(banner=banner, local=namespace)
