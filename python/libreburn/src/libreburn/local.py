# SPDX-License-Identifier: AGPL-3.0-or-later
"""Offline use of the Rust core, without a running service.

Useful in notebooks and tests: import files, plan, and inspect the moves on a
machine with no laser attached. The service API (`libreburn.Client`) is the
way to drive a real machine.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, cast

from libreburn import _core
from libreburn.types import FrameShape, ImportReport, Plan


class Project:
    """A project held in this Python process."""

    def __init__(self, data: dict[str, Any] | None = None) -> None:
        self.data: dict[str, Any] = data or {"layers": [], "objects": []}

    def __repr__(self) -> str:
        return f"Project({len(self.data['layers'])} layers, {len(self.data['objects'])} objects)"

    @property
    def layers(self) -> list[dict[str, Any]]:
        """Layers in run order (mutable)."""
        return self.data["layers"]

    def layer(self, key: str) -> dict[str, Any]:
        """The layer with `key`."""
        for layer in self.layers:
            if layer["key"] == key:
                return layer
        raise KeyError(key)

    def import_file(
        self, source: str | os.PathLike[str] | bytes, name: str | None = None, **options: float
    ) -> ImportReport:
        """Import a file; see `libreburn.Client.import_file` for the options."""
        if isinstance(source, bytes):
            if not name:
                raise ValueError("name is required when importing bytes")
            data = source
        else:
            data = Path(source).read_bytes()
            name = name or Path(source).name
        project_json, report = _core.project_import(
            json.dumps(self.data), data, name, json.dumps(options)
        )
        self.data = json.loads(project_json)
        return cast(ImportReport, json.loads(report))

    def plan(self, profile: str | os.PathLike[str], **options: Any) -> Plan:
        """Plan for a machine profile, given as a TOML path or TOML text."""
        text = str(profile)
        if "\n" not in text and Path(text).is_file():
            text = Path(text).read_text()
        return cast(
            Plan, json.loads(_core.plan_project(json.dumps(self.data), text, json.dumps(options)))
        )


def frame_outline(plan: Plan, shape: FrameShape = "bounding_box") -> list[tuple[float, float]]:
    """The outline framing would trace for `plan`."""
    return _core.frame_outline(json.dumps(plan["job"]), shape)
