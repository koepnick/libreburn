# SPDX-License-Identifier: AGPL-3.0-or-later
import os

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
pytest.importorskip("PySide6.QtWidgets", exc_type=ImportError)

from PySide6.QtWidgets import QApplication

import libreburn
from libreburn_desktop.app import MainWindow


def test_main_window_builds_offscreen() -> None:
    app = QApplication.instance() or QApplication([])
    window = MainWindow(libreburn.Client("http://127.0.0.1:9"), with_console=False)
    assert window.windowTitle() == "libreburn"
    window.close()
    assert app is not None
