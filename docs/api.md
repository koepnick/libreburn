<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Service API (v1)

`libreburnd` serves HTTP and a WebSocket on `127.0.0.1:7878` by default. All
geometry is in millimetres, origin at the top-left of the work area, Y down.

When `LIBREBURN_TOKEN` is set, every endpoint except `/api/v1/health` needs
`Authorization: Bearer <token>` (or `?token=<token>`, for WebSocket clients
that cannot set headers).

## Errors

Every error is JSON with a stable code (NF-1):

```json
{"code": "LB-IMPORT-002", "what": "Cannot import \"x.pdf\"",
 "why": "the file is not SVG, DXF, PNG, JPEG or WebP",
 "hint": "export the design as SVG or DXF, or the image as PNG",
 "message": "[LB-IMPORT-002] ..."}
```

| Code | HTTP | Meaning |
| --- | --- | --- |
| `LB-IMPORT-001` | 422 | The file could not be read or converted |
| `LB-IMPORT-002` | 415 | Not a supported file type |
| `LB-PLAN-001` | 422 | The project cannot be planned (empty, rotated image, bad settings) |
| `LB-PROFILE-001` | 422 | The machine profile is invalid |
| `LB-API-001` | 422 | Malformed request body |
| `LB-API-002` | 404 | No such layer or object |
| `LB-API-003` | 409 | Not ready: no profile loaded, or no current plan |
| `LB-API-004` | 401 | Missing or wrong token |

## Endpoints

| Method and path | Body | Returns |
| --- | --- | --- |
| `GET /api/v1/health` | | `ok` |
| `GET /api/v1/version` | | version, API version, architecture |
| `GET /api/v1/project` | | the full project, with base64 raster pixels, plus `revision` |
| `GET /api/v1/project/summary` | | layers, objects (id, name, layer, kind, bounds) and bounds, without geometry |
| `DELETE /api/v1/project` | | clears the project |
| `POST /api/v1/project/import?name=<file name>` | raw file bytes | `object_ids`, `new_layers`, `warnings`, `revision` |
| `GET /api/v1/project/layers/{key}` | | one layer (URL-encode `#` as `%23`) |
| `PATCH /api/v1/project/layers/{key}` | JSON merge patch | the updated layer |
| `GET /api/v1/project/objects/{id}` | | one object with geometry |
| `PATCH /api/v1/project/objects/{id}` | merge patch of `name`, `layer`, `transform` | object summary |
| `DELETE /api/v1/project/objects/{id}` | | 204 |
| `GET /api/v1/machine/profile` | | the loaded profile |
| `PUT /api/v1/machine/profile` | profile TOML | the parsed profile |
| `POST /api/v1/job/plan` | optional `PlanOptions` JSON | plan summary: moves, layers, bounds, estimate, warnings, `blocked` |
| `GET /api/v1/job` | | the current plan summary |
| `GET /api/v1/job/moves` | | the full `Job` as JSON |
| `GET /api/v1/job/moves.bin` | | packed moves, see below |
| `GET /api/v1/job/frame?shape=bounding_box\|hull` | | `{"outline": [[x, y], ...]}` |
| `GET /api/v1/events` | WebSocket | change events, see below |

Import options go in the query string: `raster_dpi` (default 254), `svg_dpi`
(96) and `dxf_unitless_mm` (1).

A layer patch only names what changes, for example
`{"settings": {"mode": "fill", "power_pct": 30}, "hatch": {"spacing_mm": 0.08}}`.
An object's `transform` is the affine `[a, b, c, d, e, f]` mapping
`(x, y)` to `(a·x + c·y + e, b·x + d·y + f)`.

Any change to the project or profile clears the current plan; plan again
before fetching moves.

### Binary moves

`/api/v1/job/moves.bin` packs each move in 15 little-endian bytes: `kind: u8`
(0 travel, 1 burn), `layer: u16`, `x: f32`, `y: f32`, `power_pct: f32` (NaN
when the move uses its layer's power; 0 marks a laser-off move at layer
speed, used for raster gaps and overscan).

### Events

`/api/v1/events` sends JSON text messages:

- `{"type": "project_changed", "revision": 3, "reason": "import"}`; the first
  message after connecting has reason `connected`.
- `{"type": "profile_changed", "revision": 4}`
- `{"type": "plan_ready", "revision": 4, "summary": {...}}`
- `{"type": "resync"}` when the client fell behind and should re-fetch everything.

Machine control (connect, jog, frame, run) arrives with the drivers. Framing
will go through the same safety check as the drivers: the main laser source
never fires for framing without a per-session opt-in.
