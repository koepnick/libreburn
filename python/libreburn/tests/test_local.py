# SPDX-License-Identifier: AGPL-3.0-or-later
"""Offline import and planning through the PyO3 bindings."""

import json

import pytest

import libreburn
from libreburn import _core
from libreburn.local import frame_outline

SVG = b"""<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="50mm" viewBox="0 0 50 50">
<rect x="10" y="10" width="20" height="20" fill="none" stroke="#ff0000"/>
<circle cx="40" cy="40" r="5" fill="#0000ff"/></svg>"""

PROFILE = """[machine]
name = "test"
driver = "ruida"
laser = "co2"
[work_area]
width_mm = 300
height_mm = 200
[limits]
max_power_pct = 60
max_speed_mm_s = 400
[framing]
default_source = "laser_off"
"""


def test_import_and_plan_offline() -> None:
    project = libreburn.Project()
    report = project.import_file(SVG, "parts.svg")
    assert report["new_layers"] == ["#ff0000", "#0000ff"]
    project.layer("#ff0000")["settings"].update(power_pct=40, speed_mm_s=20)
    blue = project.layer("#0000ff")
    blue["settings"].update(mode="fill", power_pct=80)
    blue["hatch"]["spacing_mm"] = 0.5

    plan = project.plan(PROFILE)
    assert plan["job"]["layers"][0]["name"] == "#ff0000"
    # 80% asks for more than the 60% cap: a warning names the layer and values.
    warning = next(w for w in plan["warnings"] if w["code"] == "LB-LIMIT-001")
    assert warning["layer"] == "#0000ff"
    assert (warning["limit_value"], warning["value"]) == (60, 80)
    assert plan["estimate"]["seconds"] > 4

    outline = frame_outline(plan)
    xs = [x for x, _ in outline]
    assert min(xs) == pytest.approx(10, abs=1e-3)
    assert max(xs) == pytest.approx(45, abs=0.02)


def test_import_errors_carry_codes() -> None:
    with pytest.raises(ValueError, match="LB-IMPORT-002"):
        libreburn.Project().import_file(b"%PDF", "x.pdf")


def test_bad_options_are_reported() -> None:
    with pytest.raises(ValueError, match="LB-API-001"):
        _core.plan_project("{}", PROFILE, "not json")


def test_dither_preserves_tone() -> None:
    dots = _core.dither(16, 16, bytes([64] * 256), json.dumps({"kind": "floyd_steinberg"}))
    assert len(dots) == 256
    assert 0.7 < sum(dots) / 256 < 0.8
    mode = json.dumps({"kind": "threshold", "level": 128})
    assert _core.dither(2, 1, bytes([0, 255]), mode) == b"\x01\x00"
