# SPDX-License-Identifier: AGPL-3.0-or-later
"""Python SDK for the libreburn laser control service."""

from libreburn._core import core_version, validate_profile
from libreburn.client import Client, LibreburnError

__version__ = core_version()

__all__ = ["Client", "LibreburnError", "__version__", "core_version", "validate_profile"]
