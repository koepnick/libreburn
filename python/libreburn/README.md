# libreburn (Python SDK)

Talks to the libreburn service (`libreburnd`) over its HTTP API, so the same
scripts run in the desktop client's console, a terminal, or Jupyter.

```python
import libreburn

client = libreburn.Client()  # http://127.0.0.1:7878 by default
client.health()
```

`python -m libreburn` opens a REPL with `client` already connected. It uses
IPython when installed (`pip install libreburn[repl]`) and otherwise falls back
to IDLE (`LIBREBURN_REPL=idle`) or the standard Python console.
