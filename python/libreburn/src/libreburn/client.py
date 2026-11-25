# SPDX-License-Identifier: AGPL-3.0-or-later
"""HTTP client for the libreburn service API (docs/api.md)."""

from __future__ import annotations

import json
import os
import struct
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator
from pathlib import Path
from typing import Any, NamedTuple, cast

from libreburn import _ws
from libreburn.types import (
    Event,
    FrameShape,
    ImportReport,
    Job,
    ObjectSummary,
    PlanSummary,
    ProjectLayer,
    ProjectSummary,
)

DEFAULT_URL = "http://127.0.0.1:7878"


class LibreburnError(RuntimeError):
    """The service could not be reached or returned an error.

    `code` is the stable error code (such as ``LB-IMPORT-002``), `hint` says
    what to try next, and `status` is the HTTP status, when there was one.
    """

    def __init__(
        self, message: str, code: str = "", hint: str = "", status: int | None = None
    ) -> None:
        super().__init__(message)
        self.code = code
        self.hint = hint
        self.status = status


class PackedMove(NamedTuple):
    """One move from `Client.moves_binary`."""

    burn: bool
    layer: int
    x: float
    y: float
    power_pct: float | None


_PACKED = struct.Struct("<BHfff")


def unpack_moves(data: bytes) -> list[PackedMove]:
    """Decode `/api/v1/job/moves.bin` (15 bytes per move)."""
    out = []
    for kind, layer, x, y, power in _PACKED.iter_unpack(data):
        out.append(PackedMove(kind == 1, layer, x, y, None if power != power else power))
    return out


class Client:
    """A connection to one libreburn service."""

    def __init__(
        self, base_url: str | None = None, token: str | None = None, timeout: float = 30.0
    ) -> None:
        self.base_url = (base_url or os.environ.get("LIBREBURN_URL") or DEFAULT_URL).rstrip("/")
        self.token = token or os.environ.get("LIBREBURN_TOKEN")
        self.timeout = timeout

    def __repr__(self) -> str:
        return f"Client({self.base_url!r})"

    # ------------------------------------------------------------ transport

    def _request(
        self,
        method: str,
        path: str,
        body: bytes | None = None,
        content_type: str = "application/json",
    ) -> bytes:
        url = f"{self.base_url}{path}"
        req = urllib.request.Request(url, data=body, method=method)
        if body is not None:
            req.add_header("Content-Type", content_type)
        if self.token:
            req.add_header("Authorization", f"Bearer {self.token}")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as res:
                return res.read()
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                err = json.loads(raw)
                raise LibreburnError(
                    str(err.get("message") or err), err.get("code", ""), err.get("hint", ""), e.code
                ) from None
            except (ValueError, AttributeError):
                raise LibreburnError(
                    f"[LB-SDK-002] {method} {url} failed with HTTP {e.code}: "
                    f"{raw[:200].decode(errors='replace')}",
                    "LB-SDK-002",
                    status=e.code,
                ) from None
        except (urllib.error.URLError, OSError) as e:
            raise LibreburnError(
                f"[LB-SDK-001] {method} {url} failed: {e}. "
                "Try: start the service with `libreburnd`, or set LIBREBURN_URL",
                "LB-SDK-001",
                "start the service with `libreburnd`, or set LIBREBURN_URL",
            ) from e

    def _get(self, path: str) -> bytes:
        return self._request("GET", path)

    def _json(self, method: str, path: str, payload: Any = None) -> Any:
        body = None if payload is None else json.dumps(payload).encode()
        raw = self._request(method, path, body)
        return json.loads(raw) if raw else None

    # ------------------------------------------------------------ service

    def health(self) -> bool:
        """Return True when the service answers its health check."""
        return self._get("/api/v1/health") == b"ok"

    def version(self) -> dict[str, Any]:
        """Return the service's version information."""
        return json.loads(self._get("/api/v1/version"))

    # ------------------------------------------------------------ project

    def project(self) -> dict[str, Any]:
        """Return the whole project, including geometry."""
        return self._json("GET", "/api/v1/project")

    def summary(self) -> ProjectSummary:
        """Return layers and objects without geometry."""
        return self._json("GET", "/api/v1/project/summary")

    def clear(self) -> None:
        """Remove every object and layer."""
        self._json("DELETE", "/api/v1/project")

    def import_file(
        self,
        source: str | os.PathLike[str] | bytes,
        name: str | None = None,
        *,
        raster_dpi: float | None = None,
        svg_dpi: float | None = None,
        dxf_unitless_mm: float | None = None,
    ) -> ImportReport:
        """Import an SVG, DXF, PNG, JPEG or WebP file.

        `source` is a path, or the file's bytes together with `name`.
        """
        if isinstance(source, bytes):
            if not name:
                raise ValueError("name is required when importing bytes, e.g. name='part.svg'")
            data = source
        else:
            path = Path(source)
            data = path.read_bytes()
            name = name or path.name
        query: dict[str, str] = {"name": name}
        for key, value in (
            ("raster_dpi", raster_dpi),
            ("svg_dpi", svg_dpi),
            ("dxf_unitless_mm", dxf_unitless_mm),
        ):
            if value is not None:
                query[key] = str(value)
        raw = self._request(
            "POST",
            f"/api/v1/project/import?{urllib.parse.urlencode(query)}",
            data,
            "application/octet-stream",
        )
        return json.loads(raw)

    def layer(self, key: str) -> ProjectLayer:
        """Return one layer by key (a color such as ``#ff0000``, a DXF layer name, or ``image``)."""
        return self._json("GET", f"/api/v1/project/layers/{urllib.parse.quote(key, safe='')}")

    def update_layer(
        self, key: str, patch: dict[str, Any] | None = None, **settings: Any
    ) -> ProjectLayer:
        """Change a layer. Keyword arguments go into its ``settings``.

        ``client.update_layer("#ff0000", power_pct=30, speed_mm_s=20)`` changes
        power and speed; pass `patch` for other sections, such as
        ``{"hatch": {"spacing_mm": 0.08}}``.
        """
        body: dict[str, Any] = dict(patch or {})
        if settings:
            body.setdefault("settings", {}).update(settings)
        return self._json(
            "PATCH", f"/api/v1/project/layers/{urllib.parse.quote(key, safe='')}", body
        )

    def update_object(self, object_id: int, **fields: Any) -> ObjectSummary:
        """Change an object's ``name``, ``layer`` or ``transform`` ([a, b, c, d, e, f])."""
        return self._json("PATCH", f"/api/v1/project/objects/{object_id}", fields)

    def move_object(self, object_id: int, dx: float, dy: float) -> ObjectSummary:
        """Move an object by `dx`, `dy` mm."""
        obj = self._json("GET", f"/api/v1/project/objects/{object_id}")
        a, b, c, d, e, f = obj["transform"]
        return self.update_object(object_id, transform=[a, b, c, d, e + dx, f + dy])

    def delete_object(self, object_id: int) -> None:
        """Remove an object."""
        self._request("DELETE", f"/api/v1/project/objects/{object_id}")

    # ------------------------------------------------------------ machine

    def profile(self) -> dict[str, Any]:
        """Return the loaded machine profile."""
        return self._json("GET", "/api/v1/machine/profile")

    def load_profile(self, source: str | os.PathLike[str]) -> dict[str, Any]:
        """Load a machine profile from a TOML file path or TOML text."""
        text = str(source)
        if "\n" not in text and Path(text).is_file():
            text = Path(text).read_text()
        raw = self._request("PUT", "/api/v1/machine/profile", text.encode(), "application/toml")
        return json.loads(raw)

    # ------------------------------------------------------------ job

    def plan(self, **options: Any) -> PlanSummary:
        """Plan the project for the loaded profile and return the summary.

        Options match ``PlanOptions``: ``tolerance_mm``, ``optimize_order`` and
        ``estimate={"time_scale": 1.1}``.
        """
        return self._json("POST", "/api/v1/job/plan", options)

    def job(self) -> PlanSummary:
        """Return the current plan's summary."""
        return self._json("GET", "/api/v1/job")

    def moves(self) -> Job:
        """Return the current plan's layers and moves as JSON."""
        return self._json("GET", "/api/v1/job/moves")

    def moves_binary(self) -> list[PackedMove]:
        """Return the current plan's moves from the compact binary endpoint."""
        return unpack_moves(self._get("/api/v1/job/moves.bin"))

    def frame_outline(self, shape: FrameShape = "bounding_box") -> list[tuple[float, float]]:
        """Return the outline framing would trace for the current plan."""
        out = self._json("GET", f"/api/v1/job/frame?shape={shape}")
        return [(x, y) for x, y in out["outline"]]

    # ------------------------------------------------------------ events

    def events(self, timeout: float | None = None) -> Iterator[Event]:
        """Yield change events from the service until the connection closes.

        The first event has ``reason == "connected"`` and the current revision.
        """
        headers = {"Authorization": f"Bearer {self.token}"} if self.token else {}
        try:
            sock = _ws.connect(f"{self.base_url}/api/v1/events", headers, timeout)
        except OSError as e:
            raise LibreburnError(
                f"[LB-SDK-003] Could not open the event stream at {self.base_url}: {e}. "
                "Try: check the service is running and the token is right",
                "LB-SDK-003",
            ) from e
        for text in _ws.messages(sock):
            yield cast(Event, json.loads(text))
