# Canvas & Persistence — Plan

Working plan for the design engine (`core/`), editing state (`editor/`), and storage (`persistence/`).

---

## Decisions

### Storage: SQLite
- Single-file document format: `*.parametric` is an SQLite database.
- Autosave: every mutation commits as a small transaction (WAL mode). No explicit save step.
- Crash-safe by default; undo/redo can later persist to an `operations` table.
- SQLite lives **only** in `persistence/`. `core/` never sees the database.
- Crate: `rusqlite` (bundled feature).

### Canvas: infinite in document units
- Geometry stored in `f64` **document units**, not pixels.
- Pixels are a view concept: the camera transforms units → screen space.
- Coordinate clamp: ±1,000,000 units (float precision guard, not a UX wall).
- Zoom clamped ~0.01×–100×.
- No hard 65k wall — users hit those and hate them.

---

## Scaffolding Order

### 1. `core/` — the engine (no GPUI)
- `geometry.rs`: `Point2`, `Rect`, basic ops (`f64` based).
- `document.rs`: `Document { layers }`, `Layer { shapes }`, `Shape` enum starting with `Rectangle` / `Ellipse`.
- Pure data + pure functions. Testable without launching the app.

### 2. `editor/` — the session
- `camera.rs`: `Camera { pan: Point2, zoom: f64 }` with `unit_to_screen` / `screen_to_unit`, clamped zoom.
- Later: selection, active tool, snapping, hover.

### 3. `persistence/` — SQLite boundary
- `database.rs`: open/create document file, WAL mode, schema migrations (simple `migrations` table or version pragma).
- Schema v1:
  - `meta(key TEXT PRIMARY KEY, value TEXT)` — schema version, doc name
  - `layers(id INTEGER PRIMARY KEY, name TEXT, order_index INTEGER)`
  - `shapes(id INTEGER PRIMARY KEY, layer_id INTEGER, kind TEXT, x REAL, y REAL, w REAL, h REAL)` — column set grows per shape type later; normalize when it gets complex
- API shaped around commands: `insert_shape`, `update_shape`, `remove_shape`.

### 4. Wiring (later steps, not in this pass)
- Renderer reads document via camera transform.
- UI actions produce commands → mutate `Document` → persistence writes → undo stack records command.

---

## Non-Goals (for now)
- Multi-document tabs (titlebar space is reserved for them).
- Constraints engine.
- Text, paths/beziers, groups.
