# SPDX-License-Identifier: AGPL-3.0-or-later
"""Catppuccin Latte (light) and Mocha (dark) palettes (NF-4)."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Palette:
    name: str
    base: str
    mantle: str
    surface0: str
    text: str
    subtext0: str
    accent: str  # mauve
    red: str


LATTE = Palette(
    name="Catppuccin Latte",
    base="#eff1f5",
    mantle="#e6e9ef",
    surface0="#ccd0da",
    text="#4c4f69",
    subtext0="#6c6f85",
    accent="#8839ef",
    red="#d20f39",
)

MOCHA = Palette(
    name="Catppuccin Mocha",
    base="#1e1e2e",
    mantle="#181825",
    surface0="#313244",
    text="#cdd6f4",
    subtext0="#a6adc8",
    accent="#cba6f7",
    red="#f38ba8",
)


def palette_for(dark: bool) -> Palette:
    return MOCHA if dark else LATTE


def stylesheet(p: Palette) -> str:
    return f"""
    QWidget {{ background: {p.base}; color: {p.text}; }}
    QMainWindow::separator, QDockWidget::title {{ background: {p.mantle}; }}
    QStatusBar {{ background: {p.mantle}; color: {p.subtext0}; }}
    QPushButton {{ background: {p.surface0}; border-radius: 6px; padding: 4px 10px; }}
    QPushButton:hover {{ border: 1px solid {p.accent}; }}
    """
