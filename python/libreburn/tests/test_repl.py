# SPDX-License-Identifier: AGPL-3.0-or-later
import importlib.util

import pytest

from libreburn import repl


def test_explicit_choice_wins() -> None:
    assert repl.choose_backend("python") == "python"
    assert repl.choose_backend("idle") == "idle"


def test_falls_back_without_ipython(monkeypatch: pytest.MonkeyPatch) -> None:
    fake_specs = {"IPython": None, "idlelib": object()}
    monkeypatch.setattr(importlib.util, "find_spec", lambda name: fake_specs[name])
    monkeypatch.delenv("LIBREBURN_REPL", raising=False)
    monkeypatch.delenv("DISPLAY", raising=False)
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    assert repl.choose_backend() == "python"
    monkeypatch.setenv("WAYLAND_DISPLAY", "wayland-0")
    assert repl.choose_backend() == "idle"
