#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Fail if a commit in BASE..HEAD lacks a Signed-off-by line matching its author.

Commits by GitHub bot accounts (Dependabot and the like) are exempt: they sign
off with an address that differs from their author address, and they are not
contributors who can certify the DCO.
"""

import subprocess
import sys


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


def main(base: str, head: str) -> int:
    bad = []
    for sha in git("rev-list", "--no-merges", f"{base}..{head}").split():
        author = git("show", "-s", "--format=%an <%ae>", sha).strip()
        if author.split(" <")[0].endswith("[bot]"):
            continue
        body = git("show", "-s", "--format=%B", sha)
        if f"Signed-off-by: {author}" not in body:
            bad.append((sha[:12], author))
    for sha, author in bad:
        print(f"{sha}: missing 'Signed-off-by: {author}'. Fix with: git commit --amend -s")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
