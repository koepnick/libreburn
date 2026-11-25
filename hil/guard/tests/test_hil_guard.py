# SPDX-License-Identifier: AGPL-3.0-or-later
"""Policy and exec-wrapper tests for the HIL GPIO guard. No hardware needed."""

from __future__ import annotations

import importlib.util
import sys
import time
from pathlib import Path
from typing import Any

import pytest

_PATH = Path(__file__).resolve().parents[1] / "libreburn_hil_guard.py"
_spec = importlib.util.spec_from_file_location("libreburn_hil_guard", _PATH)
assert _spec and _spec.loader
guard = importlib.util.module_from_spec(_spec)
sys.modules["libreburn_hil_guard"] = guard
_spec.loader.exec_module(guard)

RUIDA = guard.DEFAULT_PINS["ruida"]
SAFE = {RUIDA.interlock: False, RUIDA.harness: False, RUIDA.laser_active: True}


def make_policy(**overrides: Any) -> Any:
    cfg = guard.Config(enabled=True, machines=(RUIDA,), **overrides)
    return guard.Policy(cfg, now=0.0)


def test_default_config_is_disabled(tmp_path: Path) -> None:
    assert guard.load_config(tmp_path / "missing.toml").enabled is False
    cfg = guard.parse_config({"machines": {"ruida": {}, "galvo": {"enabled": False}}})
    assert cfg.enabled is False
    assert [m.name for m in cfg.machines] == ["ruida"]


def test_shared_line_rejected() -> None:
    with pytest.raises(ValueError):
        guard.parse_config({"machines": {"ruida": {}, "galvo": {"interlock": "GPIO17"}}})


def test_floating_inputs_read_unsafe() -> None:
    # A broken wire reads high through the pull-up: interlock open, harness missing.
    p = make_policy()
    d = p.update({}, [], runner_active=True, now=1.0)
    assert d.stop_runner
    assert "harness missing" in p.reason()


def test_starts_runner_only_after_hold() -> None:
    p = make_policy()
    assert not p.update(SAFE, [], runner_active=False, now=1.0).start_runner
    assert not p.update(SAFE, [], runner_active=False, now=5.9).start_runner
    assert p.update(SAFE, [], runner_active=False, now=6.1).start_runner


def test_interlock_open_stops_runner_and_recovers() -> None:
    p = make_policy()
    p.update(SAFE, [], runner_active=True, now=1.0)
    opened = {**SAFE, RUIDA.interlock: True}
    assert p.update(opened, [], runner_active=True, now=2.0).stop_runner
    assert not p.update(SAFE, [], runner_active=False, now=3.0).start_runner
    assert p.update(SAFE, [], runner_active=False, now=8.5).start_runner


def test_manual_stop_is_respected() -> None:
    p = make_policy()
    p.update(SAFE, [], runner_active=True, now=1.0)  # runner running: guard does not own it
    assert not p.update(SAFE, [], runner_active=False, now=20.0).start_runner


def test_laser_pulse_latches() -> None:
    p = make_policy()
    p.update(SAFE, [], runner_active=True, now=1.0)
    d = p.update(SAFE, [RUIDA.laser_active], runner_active=True, now=2.0)
    assert d.stop_runner
    assert p.latched
    assert p.machines["ruida"].activity_events == 1
    assert not p.update(SAFE, [], runner_active=False, now=60.0).start_runner


def test_steady_activity_counts_once() -> None:
    p = make_policy()
    on = {**SAFE, RUIDA.laser_active: False}
    p.update(on, [], runner_active=False, now=1.0)
    p.update(on, [], runner_active=False, now=2.0)
    assert p.machines["ruida"].activity_events == 1


def state(events: int = 0, **machine: Any) -> dict[str, Any]:
    m: dict[str, Any] = {"interlock_safe": True, "harness_present": True, "laser_active": False}
    m.update(machine)
    m["activity_events"] = events
    return {"updated_unix": time.time(), "latched": None, "machines": {"ruida": m}}


def test_check() -> None:
    now = time.time()
    assert guard.check(state(), "ruida", now)[0] == 0
    assert guard.check(None, "ruida", now)[0] == guard.EXIT_UNSAFE
    assert guard.check(state(), "galvo", now)[0] == guard.EXIT_UNSAFE
    assert guard.check(state(interlock_safe=False), "ruida", now)[0] == guard.EXIT_UNSAFE
    stale = {**state(), "updated_unix": now - 10}
    assert guard.check(stale, "ruida", now)[0] == guard.EXIT_UNSAFE


def test_exec_refuses_when_unsafe() -> None:
    rc = guard.run_guarded("ruida", ["true"], read=lambda: state(interlock_safe=False))
    assert rc == guard.EXIT_UNSAFE


def test_exec_passes_through_exit_code() -> None:
    assert guard.run_guarded("ruida", ["true"], read=state, poll_s=0.01) == 0
    assert guard.run_guarded("ruida", ["false"], read=state, poll_s=0.01) == 1


def test_exec_fails_on_activity_during_run() -> None:
    reads = iter([state(0), state(0)])

    def read() -> dict[str, Any]:
        return next(reads, state(1))

    rc = guard.run_guarded("ruida", ["sleep", "5"], read=read, poll_s=0.01)
    assert rc == guard.EXIT_LASER
