# Clay — Architecture Proposal for the Reinvented Vector Tool

> Purpose: build spec for Clay, the unified draw tool that replaces the
> line / arc / bezier pen modes. One tool, zero modes, zero handles, zero
> tool exits. Status: PROPOSAL (Sep 2026) — converges a design discussion;
> open calls are flagged `OPEN:` inline. No code changes proposed here.

## 1. Thesis

- Directness with total control. The user touches the curve itself; every
  touch is exact (typeable), graded (G0/G1/G2), and constrainable.
- Lines, arcs, beziers are not modes — they fall out of gestures.
- While a path is *active* it stays alive: extend, reshape, regrade, trace,
  never leave. `Tab` flips Object ↔ Edit Mode; no modes exist *inside*
  Edit.
- Organic hand + parametric brain: the hand proposes (fast, sloppy), the
  solver/fit disposes (tentative → committed), a number proves it.

## 2. Core concepts (5)

1. **Two modes — Blender's.** `Tab` = Object ↔ Edit. Object = wholes:
   select, transform (G/R/S + locks + numerics), duplicate, delete, join,
   order. Edit = INSIDE the active object: components, creation, refinement.
   Inside Edit: no sub-modes. The boundary is absolute: Object never touches
   components, Edit never touches other objects (except explicit
   join/connect ops, which target by cursor).
   - Path (Clay): Object = whole (move/rotate/scale/duplicate/delete/join).
     Edit = full §§3–9 (joints, spans, grades, extrude, fillet, loop cut,
     combs, tracing).
   - Line: not a special case — a 2-joint path. Same tools, trivially
     (length/angle via inspector; grades moot at ends).
   - Standalone arc: Object = whole. Edit = center + radius + sweep +
     endpoints (arc-span editing minus neighbors).
   - Legacy bezier: Object = whole. Edit = entering converts it
     (derive-on-touch → Clay joints, notice shown). No handle rendering
     anywhere, ever.
   - Primitives — rect/circle, when they exist: Object = whole. Edit =
     DEFINING GRIPS ONLY (rect corners, circle center+radius); structure
     locked, no loop cut on a circle. Freedom requires "convert to path"
     (Blender's Alt+C): bakes to a Clay path, params become joints/dims.
   - Dimension: Object = whole (move label, delete). Edit = retarget
     witnesses + type value.
   - Constraint: NOT an object — a relation. Badges/inspector in both modes;
     no Edit Mode of its own.
   - Lone/construction point: Object = whole. Edit = position (trivial).
- **Mode clarity (REQUIRED, for all objects — go further than Blender):**
  mode switcher: a dropdown in the second topbar (`Object` | `Edit`,
  steel blue vs safety orange, shows active object: `Edit · Path 12`). Accent shift: interactive UI tints orange
  in Edit, neutral in Object. Canvas: Edit dims non-active objects to ~40%,
  active full + skeleton; Object shows committed truth ONLY — zero joints,
  ticks, centers, combs, tentatives (their absence IS the signal). Cursor:
  arrow (Object) vs crosshair (Edit). Hint bar swaps key lists completely.
- **Transition semantics (Blender's, exactly):** the Object selection set
  carries into Edit — every selected object editable, ALL their components
  visible even with nothing selected inside Edit. Non-selected objects render
  ~35% opacity, dead (clicks pass through; hint bar offers `Tab`). Exit
  (Tab) restores the identical Object selection; components vanish.
  Multi-object Edit is IN v1 (selected set = editable set). Creation ops
  (extrude, trace) target the ACTIVE object (last-selected in Object,
  lightest highlight). Tab with empty Object selection = nothing.
- **Isolation (ONE object at a time):** entering Edit FREEZES the scope
  (islands of the selection; empty selection = everything). Out-of-scope
  strokes, fills, dims, and chips paint at 35% (`Background::opacity` —
  no color surgery); points stay full-bright (affordances). Out-of-scope
  dim/chip hit-testing is dead (hover, edit, drag, click — nothing
  happens); out-of-scope dim/chip selections die on mode-enter. Out-of-scope
  picks/hover/marquee/adopt dissolve. Clay creation always joins the scope
  (your new geometry is editable). Scope clears on Object-enter. Tool-
  specific pick flows (dimension/fillet/constraint tools) are exempt —
  explicit ops, solver-honest results.
- **Object Mode shows zero components, ever.** No joints on click, hover, or
  corners — hover highlights the whole object uniformly (+ quiet `Tab to
  edit` hint). Double-click enters Edit on that object. The wall has no
  holes: if a joint responds to your cursor, you are in Edit, full stop.
- **Tools × modes (reference ≠ edit):** modes decide WHAT EXISTS, tools
  decide what CLICKS DO. Snap-reference is allowed across the wall;
  manipulation is not.
  - Select: BOTH. Object = objects; Edit = components of the editable set.
    Same tool, mode decides granularity.
  - Draw/create: Object = new-path gesture creates + enters Edit. (No
    creation loiters in Object.)
  - Dimension / constraint tools: BOTH. Object = attach anywhere (objects
    AND their snap points as read-only targets) + cross-object dims/relations
    (ONLY creatable here). Edit = dims/relations wholly inside the editable
    set; foreign targets show as read-only badges (cross-object refs are
    Object's job).
  - With a referencing tool in Object, hovering a corner shows a SNAP GLYPH
    (hollow, snap-accent, cursor-local, transient) — explicitly NOT a joint
    glyph: no grade shape, no selection state, vanishes with the cursor,
    undraggable, ungradable. A targeting reticle, not an object. Click
    attaches a witness; never moves anything.
  - Select tool in Object shows NO snap glyphs (whole-highlight only) — the
    wall stands where no reference is needed.
  - Batch vs manual split: Object = modifiers/batch over wholes
    (fillet-all-corners, offset); Edit = per-corner manual craft. Same op
    family, mode decides scope.
  - View (pan/zoom) is modeless, both.
- **Toolbars (mode-scoped — swapped on Tab, active tool remembered per
  mode; inapplicable tools hidden, not greyed):**
  - Object: Select · Draw Path (creates + enters Edit) · Dimension ·
    Constraint (popover: H/V/coincident/parallel/tangent/equal) · Fillet
    (batch: all corners, radius in redo strip).
  - Edit: Select (granularity via `1`/`2`/`3` segmented control in header)
    · Place Joint · Extrude · Fillet · Loop Cut · Relax · Dimension ·
    Constraint (editable-set scope only).
  - Shared/persistent zone (both): Pan · Zoom · snap toggle · undo/redo.
  - Key-only by design (hint bar carries them, never hidden): slide,
    proportional, merge/split/join, grades (grades also live as inspector
    buttons for mouse users). Toolbar is the on-ramp; keys are the highway.
    Hover shows shortcut; cursor + hint bar sync to the active tool.
- **RMB context menu (decided — the mouse on-ramp):** right-click on
  joint (grade submenu G0/G1/G2, fullness, merge, delete) · span (convert
  type, insert, loop-cut, flip sweep, delete) · object (join, duplicate,
  convert to path, fillet-all, delete) · empty (select-all, paste, snap
  toggles). Constraint submenu (H/V/coincident/parallel/tangent/equal) on
  edges AND points. Every item shows its key; every item mirrors a key
  exactly (menus teach, keys replace). `Esc`/click-away dismisses.
- **Tab/Esc rules:** new-path gesture in Object → creates + enters Edit.
  Tab in Edit → Object + harden tentatives. Enter → harden, stay in Edit.
  Esc with pending grab/extrude → cancel it. Esc on an active Clay path →
  END the path and keep geometry (implemented Phase 2.9: per-click commits
  own their undo steps — undo explicitly to remove). Esc in Edit on a
  committed path → exit to Object (nothing destroyed).
2. **Active path.** Zero or one path held by the tool. Full skeleton visible
   (joints, centers, ticks); all draggable at any time. Extension at the
   active end; clicking near either end flips the active end.
3. **Joints** (§3). Points *on* the curve; real point entities.
4. **Spans** (§4). Curve between adjacent joints. Exactly three types: line,
   arc, bezier.
5. **Tentative → committed** (§7). Inferences are suggestions (badges), never
   silent changes; harden on commit (`Enter`); die with the path on `Esc`.

## 3. Joints — the system

```rust
Joint {
    point: PointId,          // the parametric citizen (existing entity)
    grade: G0 | G1 | G2,     // continuity contract with neighbors
    full_in / full_out: f32, // per-side fullness (default 1.0)
    ang_in / ang_out: f32,   // per-side angle offset (default 0 = automatic)
    links: (prev, next),     // topology; marks active end / closed loop
}
// Transient (never seen by solver): selected / active / tentative-halo.
```

- **Rule: the curve passes through every joint. Always.** Fit points, not
  control points. What you grab is where the curve goes.
- **Grades are construction, not constraints** (see §6 layer rule):
  - `G0` corner — adjacent spans end with independent tangents aimed at
    their neighbors. Kink by construction. THE cusp grade (`V` + fullness).
  - `G1` smooth — tangent direction derives from the neighbor chord
    (`next − prev`, normalized); sides collinear/mirrored. Fullness scales
    each arm independently (same direction, different weight).
  - `G2` curvature-matched — arm lengths solved numerically in the editor
    for curvature continuity (consumes one fullness DOF; §8 for the count).
- **Angles:** `0` = derive from chord (the 99% default). Dragging a tangent
  tick or typing stores an explicit per-side offset. G1 keeps sides
  collinear (edit one, other mirrors). Corners aim via neighbors (exact,
  typeable positions) — `OPEN (decided-lean):` corner angle offsets deferred;
  add only if cusp-aiming-by-neighbors proves insufficient.
- **Fullness 0** = arm collapses onto the joint = needle cusp from that side,
  regardless of neighbors.
- **Mobility classes:** `Free` (placed; user/solver moves them — 95%) vs
   `Derived` (computed from construction, e.g. fillet-arc tangent joints;
  grabs never fight a derivation — editor guards).
- **Active joint** (white highlight — Blender's active element): extrude grows
  from it, rotate pivots on it, merge targets it.
- **DOF per interior joint** (continuous): G0 = 4 (pos 2 + full 2); G1 auto
  = 4; G1 overridden = 5 (parity with pen-smooth); G2 = 3–4. Grade itself is
  a discrete regime DOF the pen lacks. Full table + rationale in discussion
  (Sep 2026): surrendered DOF vs the pen is exactly "directions follow
  neighbors" — the directness thesis in numbers.
- **Lifecycle:** place (click/`E`) → tentative → commit (hardens point +
  constraints) · grade flip (`V`/`H`/`Shift+H`) · insert (`Ctrl+click`,
  `Ctrl+R` — shape-preserving on smooth spans) · merge (`M`, to active) ·
  split (`P`) · delete (`Backspace` last, `Delete` selected; neighbors
  re-link, grades re-derive) · click-connect = coincident constraint (two
  joints, parametric link) vs `J` = fuse into one shared joint (one
  topology). Closed paths link last→first; start joint wears a ring.
- **Visuals:** square = G0, circle = G1, ringed dot = G2. Active = white,
  selected = accent, tentative = dashed halo.

## 4. Spans — the three types

```rust
Span = Line { a, b: JointId }
     | Arc { start, end: JointId, center: PointId, radius: f64,
             side: +1 | -1, provenance: Fillet | Placed }
     | Bezier { .. } // existing entity; controls DERIVED, never user-placed
```

- **Arc center is a real point entity** (grabbable, constrainable:
  coincident centers = concentricity free). Radius is always a live dim.
- **Provenances:** `Fillet` (corner replaced by a TRUE tangent arc + two
  derived G1 tangent joints; pure construction — NOT Blender's bevel, which
  chamfers/approximates: no segments, one exact span, radius is a dim from
  birth) vs `Placed` (free endpoints; tangency via tentative tangent
  constraints → solver → hardened on commit).
- **Uniform span-drag rule:** drag an unselected span = reshape — line bends
  *into* an arc (bulge follows cursor; center+radius appear live), bezier
  adjusts fullness, arc adjusts radius (center/joints stay). Selected span
  (mode `2`) + `G` = rigid move. Touching sculpts; explicit grab transforms.
- **Arc sweep flip:** drag the center through the chord (no key). Redo strip
  also carries a flip toggle.
- **Conversions:** line↔arc lossless both ways (flatten = radius → ∞);
  arc→bezier is lossy (warn/refuse outside explicit convert — arcs are
  exact); bezier→arc is fitting (phase-2 to-arc morph). Redo strip carries a
  type switcher where legal.
- **Out of v1:** full-circle primitive (arcs are always open spans;
  circles later or as two 180° arcs).

## 5. Curve math (no handles)

- Interpolating spline through fit joints → cubic spans. Hidden arms derived
  per span from immediate neighbors (tangent ∥ neighbor chord; arm length =
  fullness × |adjacent chord| × k). Influence strictly local (±1 neighbor).
- **Centripetal parameterization** (provably loop/cusp-free on tight
  spacing) — REQUIRED, not optional (cures Catmull-Rom overshoot disease).
- **Local by default** (predictable creation) + **relax op** (select run →
  global minimum-bending re-solve; joints pinned, arms settle — the
  draftsman's-spline moment; fullness acts as duck weights). Sculpt-smooth
  made exact: energy solve, points as hard constraints.
- G2 arm-length solve: numeric, editor-side, per joint.
- Subdivision of a smooth span is shape-preserving (new fit joints land on
  the existing curve) — loop cut adds control with zero deformation risk.
- Degenerate guards REQUIRED: duplicate/stacked joints (zero-length chords)
  must not NaN the derivation — spec the fallback (treat as G0, zero arms).

## 6. Solver interplay — the layer rule

- Relations constrain *points* (solver). Grades shape *spans* (editor
  derivation, post-solve). The layers never argue; smoothness costs zero
  solver DOF; SOL backlog untouched.
- Live solve during draw/perturb is over small systems (active path) — ms
  scale, no perf fear. Post-solve, editor rebuilds span controls from joint
  positions (+ grades + fullness + angles).
- Every joint parameter (angle, fullness, grade, radius) is solver-legible:
  constrainable/dimensionable/drivable in principle. Handles were dead
  widgets; these are citizens.
- Branch discipline for arcs follows SOL-03 (unsigned equations + captured
  branch + barrier); sweep `side` is stored explicitly, flipped deliberately.

## 7. Tentative constraints — lifecycle

- Inferences (H/V, coincident, tangent) appear live as badges during draw.
- Semantics: suggestions. Never applied silently; one keystroke/click to
  kill (badge + `Delete`).
- `Enter` (commit) hardens tentatives into real constraints. `Tab` (exit
  Edit → Object) hardens like `Enter`. `Esc` (nothing pending) cancels the
  path AND its tentatives. `Backspace` drops last joint (+ its tentatives).
- `OPEN (decided-lean):` auto-harden on commit (reviewing every H/V snap
  would murder flow). Reversible if it misfires in practice.

## 8. Gestures, keybinds, support systems

- **Modes:** `Tab` Object↔Edit · `Space` search all ops.
- **Select (in Draw):** `1`/`2`/`3` joints/spans/path · `A` all · `Alt+A`
  none · `L` path under cursor · `Ctrl+click` joint→joint = select between.
- **Transform (universal grammar, anything grabbed):** `G` grab (toggle,
  follows cursor) · `R` rotate about active · `S` scale fullness/selection ·
  `X`/`Y` axis lock (red/green full-canvas guide through grab origin + live
  readout; type to set exactly) · `Shift+X`/`Y` = the other axis · hold
  `Ctrl` snap · hold `Shift` precision · type numbers anytime · `O`
  proportional (scroll = radius, falloff in redo strip) · `G,G` slide along
  neighbor span · `LMB`/`Enter` confirm · `RMB`/`Esc` cancel.
- **Grow:** ghost preview ALWAYS live while a path is active (derived from
  the active end + cursor — not a mode, never toggled). Click commits the
  tip and CONTINUES · `Enter` commits the tip and FINISHES (that's the whole
  difference) · `E` starts a path keyboard-only, otherwise no-op · `C`
  closes · `Esc` ends-keeping · `Backspace` drops last. Click = smooth
  joint (two joints = line; curves emerge) · click existing point = connect
  (coincident) · click far end = flip active end.
- **Grades:** `V` G0 · `H` G1 · `Shift+H` G2 · `Alt+click` joint = quick flip.
- **Arcs:** `Ctrl+B` fillet (drag = radius → becomes dim, scroll = nudge
  radius, redo strip = exact number + `Arc | Chamfer` toggle — chamfer is the
  same gesture with a line span) · `Alt+drag` corner = same, mouse-inline. (One-shot arc key DELIBERATELY
  dead — fillet covers it, `A` stays select-all.)
- **Structure:** `Ctrl+click` span = insert one joint · `Ctrl+R` loop cut
  (hover preview → scroll = count, arclength-even → click lands in slide;
  scopes: span / select-between region / whole closed loop) · `F` connect ·
  `M` merge at cursor (to active) · `P` split · `J` join paths · `X` idle /
  `Delete` = delete · `F9` re-open last strip.
- **Mouse:** drag joint (reflow) / span (bend) / tick / center · scroll =
  zoom (radius / segments / count in modal contexts) · badge + `Delete`
  kills an inference.
- **Axis guides:** infinite red (X) / green (Y) line through grab origin,
  under path over grid; brighten on snap engage. (Blender colors.)
- **Redo strip:** after every gesture, its parameters appear in the second
  topbar's redo region
  (`Fillet — Type [Arc|Chamfer] · Radius [12.5] · Flip [ ]`) and re-run THAT
  op live. One deep; replaced on next gesture (old params persist in the
  Tool tab). Fast hands, exact home address.
- **Combs:** `Shift+C` toggles curvature combs on the active path (default
  ON in Edit). Teeth inside the bend + a tip ENVELOPE polyline per span.
  Teeth are anti-crossing clamped (two-pass: no tooth exceeds its
  neighbor's length by more than their spacing, so the envelope never
  breaks). Color = curvature VARIATION (|Δk| between neighbors, normalized
  per path): theme blue (fair) -> snap orange -> destructive red (worst).
  True arcs/straights read blue, wiggles glow red — the goal is literal:
  fair until the red dies. Length reads clean (clamped), color reads true
  (pre-clamp). G1 shows a color step at joints, G2 flows, G0 collides.
  Scale in inspector/redo strip; density per span. Auto-show during relax
  and grade flips even if toggled off. Closed-form frames + canvas clip —
  cheap, active path only.
- **Second topbar (NEW — decided):** sits below the main topbar, above the
  toolbar; spans left window edge → inspector left edge (canvas width only).
  Contents left→right: mode dropdown (`Object` | `Edit`, 2 options) ·
  redo-strip region (flex; populated after a gesture, empty state collapses
  so the bar stays slim) · conflict chip at the right end (visible only when
  nonzero; click isolates the fighters). Implemented Phase 2.9 as a
  last-action readout (`label — key hints`, e.g. `Extrude — click / Enter
  commit · Esc cancel`): narration teaches the grammar until Phase 3 puts
  live editable op params here.
- **Hint bar:** bottom edge, teaches keys mid-gesture (discoverability for a
  reinvented tool — REQUIRED).
- **Three layers (keep crisp):** redo strip = fix the last gesture
  (transient) · constraints/dims = persistent truth · undo stack = escape
  hatch (capped 100, tested).
- `OPEN:` line-drag-bends-into-arc — favorite rule, hence most suspicious.
  Keep, or bend-only-with-`Alt` (explicit, boring, safe)?

## 9. Tracing — measured precision (v1)

- Handles trace by illusion (looks right at this zoom). Clay traces by
  measurement: stroke-fit with guaranteed max deviation.
- Loop: stroke over reference (fast, loose; resampled by arclength) → fit
  (lines / true arcs via least-squares / beziers via Schneider; corners
  auto-split at curvature threshold → `V`) → per-span error badges, worst
  highlighted → click-to-subdivide or set **tolerance number**
  (e.g. `≤ 0.25px`) for auto-subdivide-to-pass → commit.
- Vector reference: coincident snapping = exact by construction. Raster:
  dimmed underlay + stroke-fit (full one-click autotrace = phase 2).
- Fit output is native Clay (joints + grades + tentatives + arc centers /
  radius dims) — trace, then keep editing with every tool above.

## 10. Object Mode + Edit Mode instruments

- Object Mode: paths as wholes — select, G/R/S whole paths, `Ctrl+J` join,
  duplicate, delete. Never touches components. Starting a new path in Object
  drops you straight into Edit on it.
- Edit Mode: inside the active path — ALL of §§3–9 (creation AND refinement:
  extrude, grades, fillet, loop cut, tracing) PLUS the instruments below.
  Edit never touches other objects (except explicit join/connect ops).
- **Inspector:** every number of the selection, typeable (the redo strip was
  one deep; inspector is the whole truth).
- **Constraint authority:** see/add/remove/pin relations; drive values;
  **definition coloring** (locked vs free joints at a glance).
- **Analysis:** curvature combs (settable scale), min-radius, max-deviation,
  inflection markers. Fairness as readings, not feelings.
- **Surgery:** simplify-to-tolerance (inverse of trace refine, same
  guarantee), resample evenly, relax-a-run, to-arc (phase 2), reverse,
  open/close, fillet-all-corners, equalize spacing.
- **Foreign matching** (phase-2 UI): tangent/curvature mate to external
  geometry, constraint shown + removable.
- **Object Mode ops:** select/transform whole paths, join (`Ctrl+J`),
  duplicate, delete. Creation (extrude, stroke-fit, end growth) lives in
  Edit — a new path started in Object drops you straight into Edit on it.
  Double-click enters Edit: empty interior (inside the box, on nothing)
  keeps the selection; a hit on a multi-object selection narrows to that
  island first, and a hit when the selection already is that island goes
  to Edit. Single clicks never mode-flip.
  Object selection renders ONE bounding box around the whole selection
  (SOLID accent outline + display-only corner dots, drag-inside moves).
  The box covers whole islands (point-point Coincident glue = one object;
  slide attachments, dims, and locks never merge) with arcs bounded by
  curve extent, so fillets sit inside it. Grabbing ANY part of an object
  (corner, edge, arc) rigid-moves the whole island — point resize, edge
  stretch, arc kinematics, and fillet gestures are Edit-mode-only.
  Selected edges stay highlighted under the box. Bbox scale / rotate
  handles are explicitly LATER (resizing constrained sketches goes
  through the solver as drag targets — design with SOL-02 hierarchy first).
- **Groups (future object, flat v1 — no nesting):** `Group { id, name,
  members }`, `Ctrl+G` / `Ctrl+Shift+G`. Object treats a group as ONE
  thing: single bbox, move/duplicate/delete together. Edit with a group
  selected dives inside: every member joint/edge editable (groups just join
  the selected-set rule — no new mode logic). Fills already behave this
  way (Object: loop as one; Edit: its segments/joints).
- **Every committed Clay path auto-becomes a group** (`Path 12`, ...).
  Path identity persists in the doc (not just editor bookkeeping) — this
  is the model move the panel needs. Legacy ungrouped segments stay flat
  under their layer until explicitly grouped.

## 11. Capability audit (from discussion; keep honest)

- **Impossible (pen can't either):** non-polynomial exactness (ellipse,
  parabola, clothoid, elastica — approximated; arcs the only exact v1 curve)
  · true offsets-as-primitive. Escape: TYPED spans — new families slot in
  without touching joints/grades/solver.
- **Hard in v1 (friction, not walls):** freehand-exact inflection placement
  (joint-at-spot + angle override; override UI phase 2) · foreign-tangent
  mating (representable; UI phase 2) · degenerate stacked joints (guard in
  spec) · high-frequency detail (tedious; loop cut + proportional mitigate).
- **Parity or better:** loops/figure-eights (easier than handle loops) ·
  S-curves (fall out of placement) · cusps/stars/barbs (G0 + fullness) ·
  arcs/rounds/holes (1 exact param vs ~6 approximating DOF).
- Pattern: hard cases are placement-precision problems (covered by solver +
  snap + numerics), never expressiveness problems.

## 12. Migration + model deltas (for the build plan)

- Bezier entities KEEP storage; editor DERIVES controls (no user-placed
  controls). Legacy explicit-control beziers: derive-on-touch (open, re-fit
  arms from chord rule at current grades; default G1).
- Handle rendering in paint/Select DIES with handles (diamonds → joint glyphs
  §3; tangent ticks display-only in v1).
- New editor state: `active_path + active_end`, tentative-constraint set
  (distinct from real), granularity mode (1/2/3), proportional state,
  redo-strip op record, tracing tolerance.
- New derivation layer placement: editor-side (near `editor/dims.rs` or a
  new `editor/spans.rs`), runs post-solve; pure function of
  joints → span controls. Unit-testable without GPUI.
- Pen tool (`editor/pen.rs`) is superseded by Clay; retire after Clay v1
  lands (keep file until then — no flag-day).

## 13. Phase plan

- **v1:** §§2–9 (minus OPEN items at lean-defaults) + inspector + simplify +
  definition coloring + degenerate guards + hint bar. No full circles, no
  auto-arc-recognition, no tangent-tick dragging (offsets stored, UI later).
- **Phase 2:** to-arc morph · mirror drawing (live symmetric constraints) ·
  sculpt passes (smooth/pinch, energy-based) · near-circular auto-arc ·
  foreign tangent/curvature mating UI · one-click raster autotrace · spiral /
  ellipse span types · per-side corner angle offsets (if needed).

## 15. Panel tabs (the floating menu, mode-scoped like Blender)

- The floating panel becomes a tabbed sidebar. Tab set swaps with the mode
  (like Blender's Properties context); per-mode memory of last open tab.
- Object tabs: **Item** (name, transform numbers, bbox size) · **Style**
  (stroke/fill/width — appearance is OBJECT-level: Edit shapes, Object
  styles, never mixed) · **Constraints** (object-level relations) ·
  **Dims** (dimensions on the object) · **Tool** (active tool settings).
- Edit tabs: **Item** (component numbers: joint pos/grade/fullness/angles,
  span type/radius/sweep — the inspector) · **Constraints** (relations on
  the selection: add/remove/pin/drive + definition coloring legend) ·
  **Fair** (combs scale/density, min-radius, max-deviation, inflection
  markers, tracing tolerance) · **Tool** (active tool settings + last-op
  params persist here after the redo strip dies).
- Shared: **View** (grid, guides, snap registry + toggles) in both modes.
- **Layer panel (far-left dock, left of the toolbar rail):** lists
  `doc.layers` (name + element count); click = active layer (new Clay
  geometry lands there — replaces the layers[0] assumption); eye = show /
  hide (NEEDS doc support: `visible: bool` on Layer + paint skip +
  persistence — schema touch, queued); double-click rename; `+` adds.
  RMB: delete / rename. Queued after Clay creation (needs the schema +
  active-layer plumbing first). The panel is a TREE, not a list: Layer >
  Group/Path (expandable) > member segments (leaf rows, read-only). Canvas
  sees one object (one bbox); the panel sees inside it. Selection syncs
  both directions: panel row click = canvas select (and vice versa).
- Rules: tabs never duplicate canvas gestures (panel edits numbers,
  canvas moves things); every panel field is typeable + solver-legible;
  hint bar + `Space` reach every tab action (nothing panel-only).

## 16. Cross-cutting policies (the gaps, closed with defaults)

- **Save with tentatives:** save hardens (commit-on-save). No half-state in
  the file, no silent loss. Tentatives never persist as tentative.
- **Undo granularity:** one confirmed gesture = one step (grab-confirm,
  extrude-commit, grade-flip, loop-cut). Tentative badges ride along with
  their gesture's step. Matches Blender, matches the 100-cap stack.
- **Naming:** paths auto-name (`Path 12`); rename in Item tab. Second-topbar
  mode switch shows the active object (`Edit · Path 12`).
- **SVG fidelity:** export writes DERIVED cubics/arcs (exact current
  geometry), never fit joints. Round-trips as plain paths (grades/fullness
  are editor-side; reimport = derive-on-touch defaults).
- **Snap registry (View tab):** joints, centers, midpoints, chord extensions,
  grid, increments — one ordered list, toggles + priority. Snap glyphs
  (hollow, transient) vs component glyphs (solid, graded) — never confused.
- **Out of scope (named, not forgotten):** text objects, touch/stylus
  gestures, cross-document copy/paste, first-run onboarding overlay (hint
  bar + `Space` + double-click carry v1).

## 14. Open calls (all `OPEN:` in one place)

1. §3: corner angle offsets — defer (lean yes) unless cusp-aiming needs it.
2. §7: auto-harden tentatives on commit — lean yes.
3. §8: line-drag-bends-into-arc vs bend-only-with-`Alt`.
4. ~~Edit inside selection vs third Tab stop~~ RESOLVED: Object/Edit per
   Blender, `Tab` toggles. Edit holds creation + instruments; Object holds
   wholes.
5. Fullness home: joint sides (recommendation — survives splits/merges) vs
   span. Locked tentatively to joint sides pending objection.
