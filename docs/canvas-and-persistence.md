# Canvas & Persistence — Plan

Working plan for the design engine (`core/`), editing state (`editor/`), and storage (`persistence/`).

---

## Decisions

### Storage: SQLite
- Single-file document format: `*.parametric` is an SQLite database.
- Documents live in `Documents\Parametric\` (user files, not app data); prefs/recents will use `%APPDATA%\Parametric` later.
- Autosave: the editor thread never touches SQLite. Mutations go over a channel to a dedicated writer thread (`persistence/writer.rs`) that batches queued ops and commits each batch as one WAL transaction. Main thread never blocks on disk.
- Crash-safe by default; undo/redo can later persist to an `operations` table.
- SQLite lives **only** in `persistence/`. `core/` never sees the database.
- Crate: `rusqlite` (bundled feature). Page cache capped (~2 MB) via `cache_size`.

### Document model: points are entities
- Schema v2: `points(id, x, y)`; shapes/constraints reference point ids instead of owning coordinates. This is what makes constraints possible.
- In memory: flat arenas with generational IDs (`core/ids.rs`, `core/document.rs`). Cache-friendly, no per-frame allocation, stale IDs resolve to None.
- Shape bounds are derived from two corner points, not stored.

### Constraints (vector-design focused)
- Constraints bind to **points**, not shapes; each reduces to equations over point positions (`core/constraints.rs`).
- Solver plan: incremental — on any edit, re-solve only the affected subgraph (BFS from moved points) with a projection/relaxation pass. Budget ≤1 ms/frame. DOF count surfaced in UI later.
- Roadmap tiers:
  - Geometric: coincident, horizontal, vertical, distance, angle, parallel, perpendicular, midpoint, point-on-curve.
  - Design-specific: symmetry, equal spacing/distribute, equal size, concentric, tangent, aspect-ratio locks, alignment-to-shape.
  - Relational ("revolutionary" tier): ratio constraints, linked dimensions, padding/inset, parametric grids, offset curves.
  - Behavioral: corner-radius propagation, tangent/smooth continuity, viewport pinning.

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
- `paths.rs`: filesystem locations (Documents\Parametric for documents).
- `database.rs`: open/create document file, WAL mode, schema migrations, all queries.
- `writer.rs`: background autosave thread owning the Database; batches ops per transaction.
- Schema v2:
  - `meta(key TEXT PRIMARY KEY, value TEXT)` — schema version, doc name
  - `layers(id INTEGER PRIMARY KEY, name TEXT, order_index INTEGER)`
  - `points(id INTEGER PRIMARY KEY, x REAL, y REAL)`
  - `shapes(id INTEGER PRIMARY KEY, layer_id INTEGER REFERENCES layers, kind TEXT, p1 INTEGER REFERENCES points, p2 INTEGER REFERENCES points)`
  - `constraints(id INTEGER PRIMARY KEY, kind TEXT, value REAL NULLABLE, p1 INTEGER REFERENCES points, p2 INTEGER REFERENCES points)`
- API shaped around commands; the writer thread applies them in batches.

### 4. Wiring (later steps, not in this pass)
- Renderer reads document via camera transform.
- UI actions produce commands → mutate `Document` → persistence writes → undo stack records command.

---

## Non-Goals (for now)
- Multi-document tabs (titlebar space is reserved for them).
- Constraint solver execution (schema + model are in; solving is the next pass).
- Text, paths/beziers, groups.
