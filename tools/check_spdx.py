#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Fail if a source file lacks an SPDX-License-Identifier header."""

import subprocess
import sys

SUFFIXES = (".rs", ".py", ".pyi", ".toml", ".yml", ".yaml", ".sh")
SKIP = {"Cargo.lock", "uv.lock", ".github/CODEOWNERS"}
HEADER = "SPDX-License-Identifier: AGPL-3.0-or-later"


def main() -> int:
    files = subprocess.run(
        ["git", "ls-files"], check=True, capture_output=True, text=True
    ).stdout.splitlines()
    missing = []
    for path in files:
        if path in SKIP or not path.endswith(SUFFIXES):
            continue
        with open(path, encoding="utf-8") as f:
            head = "".join(f.readline() for _ in range(5))
        if HEADER not in head:
            missing.append(path)
    for path in missing:
        print(f"{path}: missing '{HEADER}' in the first 5 lines")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
