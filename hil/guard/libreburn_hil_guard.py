#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""GPIO interlock and laser-activity guard for the HIL rig (Raspberry Pi 5).

Two halves share this file:

``daemon``  Runs as a sandboxed system service (``libreburn-hil-guard``). It
            watches each machine's interlock, laser-activity and harness-sense
            inputs and publishes their state to /run/libreburn-hil/state.json.
            When an interlock opens or a harness is unplugged it stops the HIL
            runner unit; when laser activity is seen it stops the runner and
            latches it off until someone restarts the guard. Job code cannot
            bypass this: the runner user has no GPIO access and cannot signal a
            root process.

``exec``    Used by hil/run.sh inside a job. Refuses to start the tests unless
            the machine reads safe, kills them if it stops reading safe, and
            fails the run if any laser activity was seen while they ran (HIL
            profiles cap power at zero, so any activity is a bug).

``status``  Prints the current state, for commissioning the wiring.

The guard is off until ``enabled = true`` in /etc/libreburn/hil-guard.toml.
While it is off, ``exec`` runs the command unguarded and says so.

Signals are active low with the Pi's internal pull-ups, so a broken wire reads
"interlock open" and "harness missing": fail-safe. See docs/lab-setup.md.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import signal
import subprocess
import sys
import time
import tomllib
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol

CONFIG_PATH = Path(os.environ.get("LIBREBURN_HIL_GUARD_CONFIG", "/etc/libreburn/hil-guard.toml"))
STATE_PATH = Path(os.environ.get("LIBREBURN_HIL_GUARD_STATE", "/run/libreburn-hil/state.json"))

STALE_AFTER_S = 2.0  # exec fails closed if the daemon has not written for this long
HEARTBEAT_S = 0.5
RESTART_HOLD_S = 5.0  # inputs must read safe this long before the runner is started again

EXIT_UNSAFE = 3  # interlock open, harness missing, guard down or latched
EXIT_LASER = 4  # laser activity while the tests ran


# --- configuration -----------------------------------------------------------


@dataclass(frozen=True)
class MachinePins:
    name: str
    interlock: str  # low = interlock chain closed (safe)
    laser_active: str  # low = laser firing signal present
    harness: str  # low = harness plugged in (loopback to GND in the plug)

    def lines(self) -> tuple[str, str, str]:
        return (self.interlock, self.laser_active, self.harness)


@dataclass(frozen=True)
class Config:
    enabled: bool = False
    runner_unit: str = "libreburn-runner@ghr-hil.service"
    start_runner_when_safe: bool = True
    machines: tuple[MachinePins, ...] = ()

    def machine(self, name: str) -> MachinePins | None:
        return next((m for m in self.machines if m.name == name), None)


DEFAULT_PINS = {
    "ruida": MachinePins("ruida", interlock="GPIO17", laser_active="GPIO27", harness="GPIO5"),
    "galvo": MachinePins("galvo", interlock="GPIO23", laser_active="GPIO24", harness="GPIO6"),
}


def parse_config(data: Mapping[str, Any]) -> Config:
    machines: list[MachinePins] = []
    raw_machines = data.get("machines", {})
    for name, raw in raw_machines.items():
        if not raw.get("enabled", True):
            continue
        default = DEFAULT_PINS.get(name)
        pins = MachinePins(
            name=name,
            interlock=raw.get("interlock", default.interlock if default else ""),
            laser_active=raw.get("laser_active", default.laser_active if default else ""),
            harness=raw.get("harness", default.harness if default else ""),
        )
        if not all(pins.lines()):
            raise ValueError(f"machine {name!r}: interlock, laser_active and harness are required")
        machines.append(pins)
    all_lines = [line for m in machines for line in m.lines()]
    if len(all_lines) != len(set(all_lines)):
        raise ValueError("a GPIO line is assigned twice")
    return Config(
        enabled=bool(data.get("enabled", False)),
        runner_unit=str(data.get("runner_unit", Config.runner_unit)),
        start_runner_when_safe=bool(data.get("start_runner_when_safe", True)),
        machines=tuple(machines),
    )


def load_config(path: Path = CONFIG_PATH) -> Config:
    if not path.exists():
        return Config()
    with path.open("rb") as f:
        return parse_config(tomllib.load(f))


# --- policy (pure, unit tested) ---------------------------------------------


@dataclass
class MachineState:
    interlock_safe: bool = False
    harness_present: bool = False
    laser_active: bool = False
    activity_events: int = 0
    was_active: bool = False

    @property
    def safe(self) -> bool:
        return self.interlock_safe and self.harness_present and not self.laser_active


@dataclass
class Decision:
    stop_runner: bool = False
    start_runner: bool = False
    log: list[str] = field(default_factory=list)


class Policy:
    """Turns input levels and edges into runner stop/start decisions."""

    def __init__(self, config: Config, now: float) -> None:
        self.config = config
        self.machines = {m.name: MachineState() for m in config.machines}
        self.latched: str | None = None
        # On a fresh start the guard owns the runner's state: it starts it once safe.
        self.stopped_by_guard = config.start_runner_when_safe
        self.safe_since: float | None = None
        self.started = now

    def update(
        self,
        levels: Mapping[str, bool],
        falling_edges: Iterable[str],
        runner_active: bool,
        now: float,
    ) -> Decision:
        """``levels`` maps line name to physical level (True = high)."""
        decision = Decision()
        falls = list(falling_edges)
        for pins in self.config.machines:
            st = self.machines[pins.name]
            st.interlock_safe = levels.get(pins.interlock, True) is False
            st.harness_present = levels.get(pins.harness, True) is False
            st.laser_active = levels.get(pins.laser_active, True) is False
            pulses = sum(1 for line in falls if line == pins.laser_active)
            if st.laser_active and pulses == 0 and not st.was_active:
                pulses = 1  # steady-on with no edge seen (e.g. already on at start-up)
            st.activity_events += pulses
            st.was_active = st.laser_active
            if pulses and self.latched is None:
                self.latched = f"laser activity on {pins.name}"
                decision.log.append(f"LATCHED: {self.latched}; restart the guard to clear")

        all_safe = self.latched is None and all(s.safe for s in self.machines.values())
        if not all_safe:
            self.safe_since = None
            if runner_active:
                decision.stop_runner = True
                self.stopped_by_guard = True
                decision.log.append(f"stopping {self.config.runner_unit}: {self.reason()}")
        else:
            if self.safe_since is None:
                self.safe_since = now
            held = now - self.safe_since >= RESTART_HOLD_S
            if self.stopped_by_guard and held and not runner_active:
                decision.start_runner = True
                self.stopped_by_guard = False
                decision.log.append(f"inputs safe; starting {self.config.runner_unit}")
            elif runner_active:
                self.stopped_by_guard = False
        return decision

    def reason(self) -> str:
        if self.latched:
            return self.latched
        parts = []
        for name, st in self.machines.items():
            if not st.harness_present:
                parts.append(f"{name} harness missing")
            elif not st.interlock_safe:
                parts.append(f"{name} interlock open")
            elif st.laser_active:
                parts.append(f"{name} laser active")
        return ", ".join(parts) or "safe"

    def snapshot(self, now: float) -> dict[str, Any]:
        return {
            "version": 1,
            "updated_unix": now,
            "latched": self.latched,
            "machines": {
                name: {
                    "interlock_safe": st.interlock_safe,
                    "harness_present": st.harness_present,
                    "laser_active": st.laser_active,
                    "activity_events": st.activity_events,
                }
                for name, st in self.machines.items()
            },
        }


# --- daemon -------------------------------------------------------------------


class Inputs(Protocol):
    def levels(self) -> dict[str, bool]: ...
    def wait_falling(self, timeout_s: float) -> list[str]: ...


class GpiodInputs:
    """libgpiod v2 (python3-libgpiod >= 2.0). Lines are looked up by name, e.g. GPIO17."""

    def __init__(self, line_names: Sequence[str]) -> None:
        import gpiod  # pyright: ignore[reportMissingImports]
        from gpiod.line import Bias, Direction, Edge  # pyright: ignore[reportMissingImports]

        self._gpiod = gpiod
        found: dict[str, tuple[str, int]] = {}
        for chip_path in sorted(Path("/dev").glob("gpiochip*")):
            with gpiod.Chip(str(chip_path)) as chip:
                for name in line_names:
                    if name in found:
                        continue
                    with contextlib.suppress(OSError, ValueError):
                        found[name] = (str(chip_path), chip.line_offset_from_id(name))
        missing = [n for n in line_names if n not in found]
        if missing:
            raise RuntimeError(f"GPIO lines not found: {', '.join(missing)}")
        chips = {path for path, _ in found.values()}
        if len(chips) != 1:
            raise RuntimeError(f"expected all lines on one chip, got {sorted(chips)}")
        settings = gpiod.LineSettings(
            direction=Direction.INPUT, bias=Bias.PULL_UP, edge_detection=Edge.BOTH
        )
        self._by_offset = {offset: name for name, (_, offset) in found.items()}
        self._req = gpiod.request_lines(
            chips.pop(),
            consumer="libreburn-hil-guard",
            config={tuple(self._by_offset): settings},
        )

    def levels(self) -> dict[str, bool]:
        values = self._req.get_values(list(self._by_offset))
        active = self._gpiod.line.Value.ACTIVE
        return {
            self._by_offset[o]: v == active for o, v in zip(self._by_offset, values, strict=True)
        }

    def wait_falling(self, timeout_s: float) -> list[str]:
        if not self._req.wait_edge_events(timeout_s):
            return []
        falling = self._gpiod.EdgeEvent.Type.FALLING_EDGE
        return [
            self._by_offset[e.line_offset]
            for e in self._req.read_edge_events()
            if e.event_type == falling
        ]


def write_state(snapshot: Mapping[str, Any], path: Path = STATE_PATH) -> None:
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(snapshot))
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def systemctl(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["systemctl", *args], capture_output=True, text=True, check=False)


def runner_is_active(unit: str) -> bool:
    return systemctl("is-active", "--quiet", unit).returncode == 0


def run_daemon(config: Config, inputs: Inputs | None = None) -> int:
    if not config.enabled:
        print("hil-guard: disabled in config (enabled = false); exiting", flush=True)
        return 0
    if not config.machines:
        print("hil-guard: no machines configured", file=sys.stderr)
        return 2
    lines = [line for m in config.machines for line in m.lines()]
    inputs = inputs or GpiodInputs(lines)
    STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    policy = Policy(config, time.monotonic())
    last_write = 0.0
    stop = False

    def on_term(_sig: int, _frame: object) -> None:
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, on_term)
    signal.signal(signal.SIGINT, on_term)
    print(f"hil-guard: watching {', '.join(m.name for m in config.machines)}", flush=True)
    while not stop:
        falls = inputs.wait_falling(0.1)
        now = time.monotonic()
        decision = policy.update(inputs.levels(), falls, runner_is_active(config.runner_unit), now)
        for line in decision.log:
            print(f"hil-guard: {line}", flush=True)
        if decision.stop_runner:
            systemctl("stop", "--no-block", config.runner_unit)
        if decision.start_runner:
            systemctl("start", "--no-block", config.runner_unit)
        if decision.log or now - last_write >= HEARTBEAT_S:
            write_state(policy.snapshot(time.time()))
            last_write = now
    STATE_PATH.unlink(missing_ok=True)
    return 0


# --- exec (inside the HIL job) -----------------------------------------------


def read_state(path: Path = STATE_PATH) -> dict[str, Any] | None:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def check(state: Mapping[str, Any] | None, machine: str, now: float) -> tuple[int, str]:
    """Return (exit code, message); code 0 means safe to run."""
    if state is None:
        return EXIT_UNSAFE, "guard state missing: is libreburn-hil-guard running?"
    if now - float(state.get("updated_unix", 0)) > STALE_AFTER_S:
        return EXIT_UNSAFE, "guard state is stale: libreburn-hil-guard is not updating"
    if state.get("latched"):
        return EXIT_UNSAFE, f"guard latched: {state['latched']}"
    m = state.get("machines", {}).get(machine)
    if m is None:
        return EXIT_UNSAFE, f"machine {machine!r} is not configured in the guard"
    if not m.get("harness_present"):
        return EXIT_UNSAFE, f"{machine}: interlock harness not plugged in"
    if not m.get("interlock_safe"):
        return EXIT_UNSAFE, f"{machine}: interlock open"
    if m.get("laser_active"):
        return EXIT_LASER, f"{machine}: laser activity signal is on"
    return 0, f"{machine}: interlock closed, no laser activity"


def run_guarded(
    machine: str,
    cmd: Sequence[str],
    read: Callable[[], dict[str, Any] | None] = read_state,
    poll_s: float = 0.2,
) -> int:
    code, msg = check(read(), machine, time.time())
    print(f"hil-guard: {msg}", flush=True)
    if code:
        return code
    start_state = read()
    assert start_state is not None
    start_events = int(start_state["machines"][machine]["activity_events"])
    proc = subprocess.Popen(cmd, start_new_session=True)

    def events(state: Mapping[str, Any] | None) -> int:
        if state is None:
            return start_events
        return int(state.get("machines", {}).get(machine, {}).get("activity_events", 0))

    def kill() -> None:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=10)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            with contextlib.suppress(ProcessLookupError):
                os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()

    while proc.poll() is None:
        time.sleep(poll_s)
        state = read()
        if events(state) > start_events:
            kill()
            print(f"hil-guard: FAIL: laser activity on {machine} during the run", flush=True)
            return EXIT_LASER
        code, msg = check(state, machine, time.time())
        if code:
            kill()
            print(f"hil-guard: aborted: {msg}", flush=True)
            return code
    time.sleep(max(poll_s, HEARTBEAT_S) * 2)  # catch an edge that lands as the tests exit
    if events(read()) > start_events:
        print(f"hil-guard: FAIL: laser activity on {machine} during the run", flush=True)
        return EXIT_LASER
    return proc.returncode


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0] if __doc__ else None)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("daemon", help="watch the GPIO inputs (system service)")
    sub.add_parser("status", help="print the current state")
    ex = sub.add_parser("exec", help="run a command only while the machine reads safe")
    ex.add_argument("--machine", required=True)
    ex.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    config = load_config()

    if args.cmd == "daemon":
        return run_daemon(config)
    if args.cmd == "status":
        print(json.dumps({"enabled": config.enabled, "state": read_state()}, indent=2))
        return 0
    cmd = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not cmd:
        parser.error("exec needs a command after --")
    if not config.enabled:
        print(
            "hil-guard: GPIO guard disabled (/etc/libreburn/hil-guard.toml); "
            "relying on the zero power cap and removed keys",
            flush=True,
        )
        os.execvp(cmd[0], cmd)
    return run_guarded(args.machine, cmd)


if __name__ == "__main__":
    sys.exit(main())
