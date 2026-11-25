#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Fail if a #[pyfunction] in crates/libreburn-py has no stub in _core.pyi (NF-9)."""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "crates/libreburn-py/src/lib.rs"
STUB = ROOT / "python/libreburn/src/libreburn/_core.pyi"


def main() -> int:
    rust = set(re.findall(r"#\[pyfunction\]\s*(?:#\[[^\]]*\]\s*)*fn\s+(\w+)", RUST.read_text()))
    stub = set(re.findall(r"^def\s+(\w+)", STUB.read_text(), re.MULTILINE))
    ok = True
    for name in sorted(rust - stub):
        print(f"{STUB.relative_to(ROOT)}: no stub for {name}()")
        ok = False
    for name in sorted(stub - rust):
        print(f"{STUB.relative_to(ROOT)}: stub {name}() has no #[pyfunction]")
        ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
