# SPDX-License-Identifier: AGPL-3.0-or-later
from libreburn_desktop import theme


def test_dark_uses_mocha_and_light_uses_latte() -> None:
    assert theme.palette_for(True) is theme.MOCHA
    assert theme.palette_for(False) is theme.LATTE


def test_stylesheet_uses_palette_colors() -> None:
    css = theme.stylesheet(theme.MOCHA)
    assert theme.MOCHA.base in css
    assert theme.MOCHA.text in css
