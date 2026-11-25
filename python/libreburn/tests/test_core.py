# SPDX-License-Identifier: AGPL-3.0-or-later
from pathlib import Path

import pytest

import libreburn
from libreburn import _core

REPO = Path(__file__).resolve().parents[3]


def test_version_matches_rust_core() -> None:
    assert libreburn.__version__ == _core.core_version()


def test_ruida_scramble_round_trips() -> None:
    data = bytes(range(256))
    assert bytes(_core.ruida_unscramble(bytes(_core.ruida_scramble(data, 0x88)), 0x88)) == data


def test_shipped_profiles_validate() -> None:
    for path in (REPO / "profiles").glob("*.toml"):
        assert libreburn.validate_profile(path.read_text())


def test_invalid_profile_raises_with_code() -> None:
    with pytest.raises(ValueError, match="LB-PROFILE-001"):
        libreburn.validate_profile("not = [valid")
