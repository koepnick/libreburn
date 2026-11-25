# SPDX-License-Identifier: AGPL-3.0-or-later
"""HTTP client for the libreburn service API."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from typing import Any

DEFAULT_URL = "http://127.0.0.1:7878"


class LibreburnError(RuntimeError):
    """The service could not be reached or returned an error."""


class Client:
    """A connection to one libreburn service."""

    def __init__(
        self, base_url: str | None = None, token: str | None = None, timeout: float = 5.0
    ) -> None:
        self.base_url = (base_url or os.environ.get("LIBREBURN_URL") or DEFAULT_URL).rstrip("/")
        self.token = token or os.environ.get("LIBREBURN_TOKEN")
        self.timeout = timeout

    def __repr__(self) -> str:
        return f"Client({self.base_url!r})"

    def _get(self, path: str) -> bytes:
        req = urllib.request.Request(f"{self.base_url}{path}")
        if self.token:
            req.add_header("Authorization", f"Bearer {self.token}")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as res:
                return res.read()
        except (urllib.error.URLError, OSError) as e:
            raise LibreburnError(
                f"[LB-SDK-001] GET {self.base_url}{path} failed: {e}. "
                "Try: start the service with `libreburnd`, or set LIBREBURN_URL"
            ) from e

    def health(self) -> bool:
        """Return True when the service answers its health check."""
        return self._get("/api/v1/health") == b"ok"

    def version(self) -> dict[str, Any]:
        """Return the service's version information."""
        return json.loads(self._get("/api/v1/version"))
