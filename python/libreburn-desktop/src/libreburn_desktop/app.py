# SPDX-License-Identifier: AGPL-3.0-or-later
"""Main window and entry point (`libreburn` command)."""

from __future__ import annotations

import importlib.util
import sys

from PySide6.QtCore import Qt
from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import (
    QApplication,
    QDockWidget,
    QLabel,
    QMainWindow,
    QWidget,
)

import libreburn
from libreburn_desktop import theme


def _console_widget(namespace: dict[str, object]) -> QWidget:
    """Embedded IPython console, or a hint if qtconsole is not installed (AR-2)."""
    if importlib.util.find_spec("qtconsole") is None:
        label = QLabel(
            "Install libreburn-desktop[console] for the embedded IPython console.\n"
            "Meanwhile, run `python -m libreburn` in a terminal."
        )
        label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        return label
    from qtconsole.inprocess import QtInProcessKernelManager
    from qtconsole.rich_jupyter_widget import RichJupyterWidget

    manager = QtInProcessKernelManager()
    manager.start_kernel(show_banner=False)
    manager.kernel.shell.push(namespace)  # type: ignore[union-attr]
    client = manager.client()
    client.start_channels()
    widget = RichJupyterWidget()
    widget.kernel_manager = manager
    widget.kernel_client = client
    return widget  # pyright: ignore[reportReturnType]  (qtpy wraps PySide6)


class MainWindow(QMainWindow):
    def __init__(self, client: libreburn.Client, with_console: bool = True) -> None:
        super().__init__()
        self.client = client
        self.setWindowTitle("libreburn")
        self.resize(1280, 800)

        canvas = QLabel("Canvas")
        canvas.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.setCentralWidget(canvas)

        if with_console:
            dock = QDockWidget("Console", self)
            dock.setObjectName("console")
            dock.setWidget(_console_widget({"libreburn": libreburn, "client": client}))
            self.addDockWidget(Qt.DockWidgetArea.BottomDockWidgetArea, dock)

        self.statusBar().showMessage(f"Service: {client.base_url}")


def is_dark() -> bool:
    return QGuiApplication.styleHints().colorScheme() == Qt.ColorScheme.Dark


def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName("libreburn")
    app.setDesktopFileName("io.github.koepnick.libreburn")
    app.setStyleSheet(theme.stylesheet(theme.palette_for(is_dark())))
    window = MainWindow(libreburn.Client())
    window.show()
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
