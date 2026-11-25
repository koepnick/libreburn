# libreburn-desktop

The libreburn desktop client: a native Wayland Qt 6 app (PySide6) with an
embedded IPython console. Run it with `libreburn`. It holds no machine state;
everything it shows comes from the service API.

Install the `console` extra for the embedded IPython console. Without it the
console panel explains how to add it, and `python -m libreburn` still gives a
terminal REPL.
