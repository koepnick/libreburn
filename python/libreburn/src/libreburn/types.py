# SPDX-License-Identifier: AGPL-3.0-or-later
"""Typed shapes of the JSON the service returns (see docs/api.md).

These are TypedDicts, so values stay plain dicts at runtime and editors still
autocomplete their keys (NF-9).
"""

from __future__ import annotations

from typing import Literal, NotRequired, TypedDict

LayerMode = Literal["cut", "fill", "image", "galvo_hatch"]
MoveKind = Literal["travel", "burn"]
Severity = Literal["info", "warning", "blocking"]
FrameShape = Literal["bounding_box", "hull"]


class Bounds(TypedDict):
    min_x: float
    min_y: float
    max_x: float
    max_y: float


class LayerSettings(TypedDict):
    name: str
    mode: LayerMode
    speed_mm_s: float
    power_pct: float
    passes: int
    frequency_khz: float | None
    pulse_width_ns: int | None
    q_pulse_ns: NotRequired[int | None]


class Dither(TypedDict):
    kind: Literal["threshold", "floyd_steinberg", "atkinson", "ordered", "grayscale"]
    level: NotRequired[int]


class HatchSettings(TypedDict):
    angle_deg: float
    spacing_mm: float
    sets: int
    angle_increment_deg: float
    bidirectional: bool
    offset_mm: float


class RasterSettings(TypedDict):
    line_interval_mm: float
    dither: Dither
    bidirectional: bool
    overscan_mm: float
    min_power_pct: float
    invert: bool


class ProjectLayer(TypedDict):
    key: str
    color: str
    output: bool
    settings: LayerSettings
    hatch: HatchSettings
    raster: RasterSettings


class ObjectSummary(TypedDict):
    id: int
    name: str
    layer: str
    kind: Literal["vector", "raster"]
    bounds: Bounds | None


class ProjectSummary(TypedDict):
    revision: int
    layers: list[ProjectLayer]
    objects: list[ObjectSummary]
    bounds: Bounds | None


class ImportReport(TypedDict):
    object_ids: list[int]
    new_layers: list[str]
    warnings: list[str]
    revision: NotRequired[int]


class Warning(TypedDict):
    code: str
    severity: Severity
    layer: str | None
    limit: str | None
    limit_value: float | None
    value: float | None
    message: str


class LayerEstimate(TypedDict):
    name: str
    seconds: float
    burn_seconds: float
    burn_mm: float
    travel_mm: float


class Estimate(TypedDict):
    seconds: float
    layers: list[LayerEstimate]


class Move(TypedDict):
    kind: MoveKind
    x: float
    y: float
    z: float | None
    tool: int | None
    aux: int | None
    layer: int
    power_pct: float | None


class Job(TypedDict):
    layers: list[LayerSettings]
    moves: list[Move]


class PlanSummary(TypedDict):
    revision: int
    moves: int
    layers: list[LayerSettings]
    bounds: Bounds | None
    estimate: Estimate
    warnings: list[Warning]
    blocked: bool


class Plan(TypedDict):
    job: Job
    estimate: Estimate
    warnings: list[Warning]


class Event(TypedDict):
    type: Literal["project_changed", "profile_changed", "plan_ready", "resync"]
    revision: NotRequired[int]
    reason: NotRequired[str]
    summary: NotRequired[PlanSummary]
