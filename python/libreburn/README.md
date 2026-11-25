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

## Files, layers and jobs

```python
client.load_profile("profiles/omtech-k40-ruida.toml")   # or start libreburnd --profile
report = client.import_file("parts.svg")                # SVG, DXF, PNG, JPEG, WebP
client.update_layer("#ff0000", power_pct=40, speed_mm_s=20, passes=2)
client.update_layer("#0000ff", {"hatch": {"spacing_mm": 0.08}}, mode="fill")
plan = client.plan()
print(plan["estimate"]["seconds"], [w["message"] for w in plan["warnings"]])
outline = client.frame_outline("hull")

for event in client.events():      # live changes from every client
    print(event)
```

Errors raise `libreburn.LibreburnError` with `.code` (such as `LB-IMPORT-002`),
`.hint` and `.status`. Responses are typed with the TypedDicts in
`libreburn.types`, so editors autocomplete their keys.

## Offline

`libreburn.Project` runs the Rust importer and planner in-process, with no
service or machine:

```python
project = libreburn.Project()
project.import_file("parts.dxf")
project.layer("CUT")["settings"]["power_pct"] = 40
plan = project.plan("profiles/omtech-k40-ruida.toml")
```
