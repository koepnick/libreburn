# SPDX-License-Identifier: AGPL-3.0-or-later
"""`python -m libreburn`: open a console with a connected client."""

import libreburn
from libreburn import repl


def main() -> None:
    repl.start({"libreburn": libreburn, "client": libreburn.Client()})


if __name__ == "__main__":
    main()
