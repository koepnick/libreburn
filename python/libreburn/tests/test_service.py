# SPDX-License-Identifier: AGPL-3.0-or-later
"""End-to-end tests against a real libreburnd, built from this checkout."""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import threading
import time
from collections.abc import Iterator
from pathlib import Path

import pytest

from libreburn import Client, LibreburnError
from libreburn.types import Event

REPO = Path(__file__).resolve().parents[3]
SVG = b"""<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="50mm" viewBox="0 0 50 50">
<rect x="10" y="10" width="20" height="20" fill="none" stroke="#ff0000"/></svg>"""


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def libreburnd() -> Path:
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("cargo is not installed")
    subprocess.run(
        [cargo, "build", "--quiet", "--locked", "-p", "libreburn-service", "--bin", "libreburnd"],
        cwd=REPO,
        check=True,
        timeout=900,
    )
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    return target / "debug" / "libreburnd"


@pytest.fixture
def service(libreburnd: Path) -> Iterator[Client]:
    port = _free_port()
    env = {**os.environ, "LIBREBURN_TOKEN": "0123456789abcdef-test"}
    proc = subprocess.Popen(
        [
            str(libreburnd),
            "--listen",
            f"127.0.0.1:{port}",
            "--profile",
            str(REPO / "profiles" / "omtech-k40-ruida.toml"),
        ],
        env=env,
    )
    client = Client(f"http://127.0.0.1:{port}", token="0123456789abcdef-test", timeout=10)
    deadline = time.monotonic() + 20
    while True:
        try:
            if client.health():
                break
        except LibreburnError:
            if time.monotonic() > deadline or proc.poll() is not None:
                proc.kill()
                raise
            time.sleep(0.1)
    yield client
    proc.terminate()
    proc.wait(timeout=10)


def test_import_plan_and_moves(service: Client) -> None:
    report = service.import_file(SVG, "box.svg")
    assert report["new_layers"] == ["#ff0000"]
    layer = service.update_layer("#ff0000", power_pct=10, speed_mm_s=50)
    assert layer["settings"]["power_pct"] == 10
    assert layer["settings"]["mode"] == "cut"

    summary = service.plan()
    assert not summary["blocked"]
    assert summary["estimate"]["seconds"] > 0
    moves = service.moves()["moves"]
    packed = service.moves_binary()
    assert len(packed) == len(moves) == summary["moves"]
    assert packed[-1].burn
    assert packed[-1].x == pytest.approx(moves[-1]["x"], abs=1e-4)

    obj = report["object_ids"][0]
    moved = service.move_object(obj, 5, 0)
    assert moved["bounds"] is not None
    assert moved["bounds"]["min_x"] == pytest.approx(15, abs=1e-3)
    with pytest.raises(LibreburnError) as err:
        service.job()
    assert err.value.code == "LB-API-003"


def test_wrong_token_is_rejected(service: Client) -> None:
    bad = Client(service.base_url, token="nope")
    with pytest.raises(LibreburnError) as err:
        bad.summary()
    assert err.value.status == 401
    assert err.value.code == "LB-API-004"


def test_events_follow_changes(service: Client) -> None:
    seen: list[Event] = []
    ready = threading.Event()

    def listen() -> None:
        for ev in service.events(timeout=10):
            seen.append(ev)
            ready.set()
            if ev.get("reason") == "import":
                return

    t = threading.Thread(target=listen, daemon=True)
    t.start()
    assert ready.wait(10)
    service.import_file(SVG, "box.svg")
    t.join(10)
    assert seen[0].get("reason") == "connected"
    assert seen[-1]["type"] == "project_changed"
    assert seen[-1].get("reason") == "import"
