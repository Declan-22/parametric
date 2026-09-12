# Fillet modifier status

## Implemented

- Added a dedicated parametric fillet evaluator in `src/core/fillet.rs`.
- Added document-level fillet modifiers. A modifier stores the two source
  segments, shared corner, radius, and owned derived arc geometry.
- Source segments are not rewritten when a fillet is created.
- Added support for line, circular arc, and Bezier endpoint tangents when
  evaluating a fillet.
- Added Fillet tool UI in the toolbar and `F` keyboard shortcut.
- Added two-segment selection and shared-corner clicking.
- Added a live hover preview of a valid fillet.
- Added generated tangent points, center, control point, and arc segment when
  a modifier is committed.
- Added point-on-segment and tangent constraints for the derived feature.
- Added the existing numeric dimension input overlay immediately after a
  fillet is created.
- Radius edits rebuild the derived arc while preserving the source segments.
- Fillet geometry refreshes while source geometry is dragged.
- Added a Modifiers section to the Inspector with delete support.
- Deleting a modifier removes its generated arc, construction points, radius
  dimension, and related derived geometry.
- Fill-loop point evaluation replaces a filleted corner with the tangent arc,
  so filled closed loops follow the effective filleted outline.
- Added SQLite persistence for the source-level fillet definition.
- Correct fillet orientation for every segment endpoint ordering. The
  evaluator now derives explicit away-rays (corner → far end) per segment —
  lines from the far endpoint, arcs from the resolved circle-travel winding,
  beziers from the end-handle ray — so rectangle corners fillet inside the
  shape instead of mirroring outside it when the corner sits at a segment
  end rather than its start.
- Invalid-radius clamping via `Fillet::max_radius`. Tangent points must land
  inside the far ends, so oversize radii evaluate to `None` instead of
  emitting geometry past the segment ends. Creation clamps the 24.0 default
  down to what the picked edges fit; typed radius edits clamp and store the
  applied value.
- Corner-side selection: when a picked pair shares more than one endpoint,
  the corner nearest the cursor wins (previously the first found).
- Failure and clamp UX through the toast stack: degenerate pairs and
  too-short edges fail loudly ("Can't fillet these edges"), and a clamped
  radius edit reports what was applied.
- Hover preview clamps like creation (never promises an unbuildable
  fillet) and repaints on geometry change, not just show/hide.
- Expanded evaluator tests: right-angle orientation matrix (all four
  start/end orderings agree), acute/obtuse tangent geometry, degenerate
  angles, oversize-radius rejection plus `max_radius` value, a line+arc
  travel-direction case, and a corner-at-end bezier case.

## Remaining work

- Nested-loop holes and concave-loop visuals still want human visual QA:
  fills composite in layer (painter) order under the GPU's nonzero fill,
  which no headless check can sign off.

## Completed this pass

- Persisted generated feature IDs. This also fixed two latent bugs found
  while mapping the subsystem: the fillet `INSERT` lived in `load_document`
  (where `modifiers` is always empty) while `save_document` deleted fillets
  without re-inserting them, so fillets survived no round-trip at all; and
  deleting a loaded fillet leaked its orphan arc, points, and dimension
  because the modifier record had lost its derived ids. The `fillets` table
  now stores the wedge side plus arc/tangent/center/control ids (nullable,
  additive migration); load resolves them against the verbatim arenas and
  degrades to the lazy path when stale.
- Radius input reopen. The Inspector R value reopens the radius value input
  for any modifier, loaded   fillets included (relinked via the persisted
  arc id to its radius dimension).
- Dedicated fillet dimension layout. Fillet arcs no longer reuse the
  center→endpoint dashed line: the leader runs from a point on the fillet
  edge itself (slide picks the angle across the arc span) outward along the
  radial, with the container riding a free offset. The container line's own
  arrowheads land one on the edge (the attached arrow) and one at the
  container. Slide is clamped to the arc span so the arrow can never leave
  the edge; dragging the container edits slide/offset through the same
  frame. Non-fillet radius dims keep the legacy layout untouched.
- Radius drag affordance. Grabbing a fillet center in the Fillet tool starts
  a dedicated gesture (pinned center, cursor distance becomes the radius),
  applied geometrically each frame (exact, no source wobble) with silent
  clamping, committed as one undo step on release. Typing keeps the solver
  path below.
- First-class solver radius edits. `update_fillet_radius` now solves the
  modifier's component jointly (radius dimension + tangency + point-on-line
  equations, corner hard-pinned, trial seeded at the exact new geometry so
  radius-captured equations agree from the start) instead of only moving
  derived points, with the exact geometric rebuild as a guaranteed fallback.
  `refresh_fillets` remains as the exactness pass and the curve-source
  fallback. Source drags already solved jointly through the derived
  constraints.
- Fill-loop robustness. `loop_points` now collapses degenerate sample runs,
  requires ≥3 distinct points, and rejects zero-signed-area loops
  (collapsed and self-cancelling bowtie windings), which every consumer
  already treats as "broken loop" and skips. Point order is deliberately
  preserved (marquee maps indices back to segments), so no winding
  reordering was introduced.
- Concave (reflex) corner sides. `FilletSide::{Inner, Outer}` on the
  modifier (persisted): Outer mirrors tangent points onto the line
  extensions and bulges the arc into the reflex wedge — the useful side for
  notches, and an outside round on convex corners. The solver's point-on-line
  equations are infinite-line, so derived constraints carry over unchanged.
  Creation defaults to Inner; the Inspector side chip flips per modifier.
- Fillet-tool hover highlights. The hover handler previously returned the
  preview state without ever setting `hover`, so edges/points never lit up;
  it now tracks the hovered element like the Dimension tool does (the paint
  layer already renders hover outlines tool-independently).
- Persistent drag affordances. Committed fillet centers now always paint
  while the Fillet tool is active, plus for any selected fillet arc —
  previously the only center dot was the hover preview's, which vanished
  the moment the cursor left the edge (the disk-based reveal state was
  computed but never consumed by any paint code).
- Source-edge trimming in rendering. Filleted line sources draw far end to
  tangent point (exact), arc/bezier sources truncate cached samples past
  the tangent (guarded, full-span fallback) — the sharp corner stub no
  longer sticks out past the arc in strokes, hover outlines, or selection
  outlines. The document stays untouched (trim is paint-only).
- Source-matched fillet stroke. Fillet arcs resolve weight + color live
  from the first stroked source edge and paint nothing when unstroked,
  instead of the old always-on dark stroke plus the text-primary evaluator
  polyline (which double-painted every fillet). The evaluator polyline now
  only covers unlinkable modifiers, styled the same way.
- Reliable center grabs. Grabbing a committed fillet center starts the
  radius drag in Move and Fillet alike (before point-grab/marquee logic,
  which is how dots used to miss and end in marquee-selecting the whole
  shape). The drag also drives an open value input live (stored value plus
  measured baseline sync, typed buffer untouched), so typing and dragging
  compose. Pressing an already-filleted corner reuses the modifier
  (selects the arc, reopens its input) instead of stacking a duplicate.
- Constraint chip rows pitch at 30px against 22px chips (was edge-touching
  at 22px).
- Drag follower completion for arc bodies. `solve_drag` now completes
  every touched arc's four defining points into the follower set, so
  dragging near (but not including) a fillet no longer hard-pins its
  tangent contact/center while the edge moves — that pin was making
  tangency/radius equations unsatisfiable, every frame was rejected, and
  corner drags on filleted rectangles froze. Coincident (and all other)
  equations stay pure-relative; only the follower set changed, with soft
  anchors still regularizing. Same fix covers edge-stretch and group
  drags touching fillet arcs.
- Fillet-arc body drags redirect to the center behavior (radius resize,
  or corner-drag when locked) instead of the kinematic arc scale, which
  fought the fillet constraints and warped.
- Radius-drag direction corrected: outward along corner→center grows the
  fillet, inward shrinks it (was reversed).
- Drag follower completion for arc bodies. `solve_drag` completes every
  touched arc's four defining points into the follower set, so dragging
  near (but not including) a fillet no longer hard-pins its tangent
  contact/center while the edge moves. Tangent hard-pinning explicitly
  skips fillet arcs (their contacts slide by definition). Coincident
  equations were already pure-relative; only the follower set changed.
- Fillet equations stay out of live drags. `Solver::strip_fillet_equations`
  drops exactly the creation-derived tangent / circle-point /
  equal-radius / on-line equations from interactive solves (matched on
  modifier ids, never user rows); radius Distance equations stay and hold.
  `refresh_fillets` re-seats everything exactly after every commit, so
  drags touching filleted geometry solve a small consistent system
  instead of fighting stiff fillet equations and rejecting every frame
  (which read as frozen drags and one-sided stretching). Discrete
  applies and validations keep the full system. Same contract in the
  corner-drag trial solve.
- Fillet-arc body drags redirect to the center behavior (radius resize,
  or corner-drag when locked) instead of the kinematic scale.
- Solo source-edge drags convert to the center behavior past the click
  threshold (clicks still select the edge); tangent-point grabs route
  there too, resizing symmetrically by construction instead of fighting
  the solver.
- Branch-flip protection. Fillet arcs keep the bend barrier on every
  solver build (dimension applies pass empty drag sets, where radius
  equations alone can settle the mirrored branch), and both fillet solves
  verify the solved control against the exact branch — flips fall back
  (radius) or hold last-valid (corner drag) instead of obliterating the
  fill.
- Native edge-stretch restored on fillet source edges (a prior turn
  hijacked them into radius drags, which broke rectangle scaling — that
  redirect is reverted; only the fillet arc body itself redirects to the
  center behavior). Tangent hard-pinning skips fillet arcs, and
  fillet-adjacent tangency is exempt from the post-solve line rotation
  (refresh owns fillet exactness), so edge/corner drags no longer fight
  or slant H/V-locked sketches.
- Corner-drag uses rigid-translation targets (both tangent points offset
  by the cursor delta from fixed grab references — never both-to-one
  point, which is structurally infeasible under a fixed radius) and
  strict per-frame commits, so infeasible pulls resist instead of
  slanting locked constraints.
- Transitive drag followers. The follower set closes over the whole
  constraint/component graph instead of one hop, so opposite corners and
  fillet contacts follow softly rather than pinning at canvas positions
  and freezing corner drags (or stretching one side while the rest
  moves). Anchors still regularize; constraints dominate.
- Fillet derived points are never drag targets. Solo source-edge presses
  drag the far end only (the tangent end follows via constraints +
  refresh instead of fighting it); group drags filter derived points out
  of the target set. Kinematic arc plans and shift-spin both skip fillet
  arcs, so derived geometry can never lead a gesture it would then fight.
- Fillet arc-body drags move the whole corner (corner-drag semantics)
  except while the value input is open, when they drive its number —
  never the kinematic scale, which fought the fillet and warped.
- Staged handle grabs. Center/tangent presses stage instead of grabbing
  immediately: real movement converts past the click threshold, clean
  releases emulate the consumed click (fill/edge selects, shift
  semantics). Handles no longer hijack clicks near filleted corners.
- Esc clears selection (after tool reset) before falling through to
  menus.
- Verified by headless harness (real solver core): single, sequential,
  and rigid-group drags on a subdivided+dimensioned rectangle all
  converge with H/V exact — including app-faithful follower sets.
- No silent dimension rewrites anywhere. Radius edits solve jointly with
  flat dims held hard (the shape accommodates both, e.g. the rectangle
  grows, or the edit is refused with the conflicting dims named) —
  earlier code rewrote flat dim values behind the user's back. The radius
  solve no longer requires the dimension to exist (deleted dims drag
  dim-less), and value writes go only to the update-resolved dim row so a
  mid-edit deletion can't corrupt another row.
- Fillet derived points don't vote in snap consensus (their re-derived
  positions would yank whole-drag snap jumps), and stale radius inputs
  close when geometry drags start instead of sitting editing-highlighted
  over them.
- Relative radius drags with a committed-dimension lock. Grabbing the
  center resizes relative to the grab radius along the center→corner axis
  (toward shrinks, away grows) instead of restarting from zero. Once the
  radius input is Entered, the dimension owns the radius and the handle
  locks with an explanatory toast; it drags again only while that input is
  open (driving its number live, typed buffer preserved) or when no radius
  dimension exists — in which case the drag shows a transient accent
  readout. No duplicate modifiers: pressing an already-filleted corner
  reuses it (selects the arc, reopens its input).
- Topological subdivision for line-line fillets. Committing a fillet
  rewires both sources to the tangent points, migrates edge-affine
  constraints (plus exactness-checked H/V) and revalues point dims to the
  new flats, splices the arc into adjacent fill loops, drops unmigratable
  corner constraints (counted in a toast), and consumes the corner point
  with a non-cascading removal. Selecting an edge shows both current
  endpoints; dimensioning it measures the flat, never the fillet. Curves
  keep the legacy referenced-corner model. The evaluator recovers
  subdivided corners live from the line intersection (exact, never stale),
  so refresh needs no changes.
- Heal on delete. Removing a subdivided fillet returns both tangent
  points to the recovered corner, merges t2 into t1 (which becomes the
  corner again), revalues dims back to full spans, and unsplices the arc —
  deleting a fillet restores the pre-fillet topology. Derived constraints
  are cleaned by exact id-pattern match (never user rows), and doomed
  points go with non-cascading removal.
- Locked-center drag resizes from the adjacent edges. With a committed
  radius dimension the center handle drags the virtual corner instead
  (both tangent points chase the cursor in a trial solve; H/V, tangency,
  and the locked radius shape the answer), so a filleted rectangle corner
  resizes like the old sharp corner did. Unlinkable legacy modifiers keep
  the locked toast.
- Inner/outer dim morph. The fillet leader offset is signed: positive
  rides outside, negative dives inside toward the center (container
  reaches it at -r). Dragging through zero morphs the dim continuously
  from outer to inner layout, arrowheads riding both ends throughout.
- Fill loops sample spliced fillet arcs inline (48 samples, either
  direction) instead of chording across them; legacy corner substitution
  is retained and densified to match.
- Round joins plus denser tessellation. Stroked polylines now emit a
  same-color disk per vertex, fusing the butt-jointed quads that cracked
  open at tessellation joints under thick strokes. Arc chord error tightened
  0.5px → 0.25px, beziers sample per 2px instead of 4px, and fillet paint
  paths use adaptive (not fixed-32) sampling.
- Dimension-on-filleted-edge semantics (decision, no code change): dims
  keep measuring the full source entities, never the trimmed flats. This
  matches parametric CAD (overall extents stay driven; flats shrink as the
  fillet consumes them) and falls out of the never-rewrite-sources
  architecture for free — source endpoints don't move under filleting, so
  existing dims neither fight the fillet constraints nor silently change
  meaning. To dimension a flat itself, place a point-pair dimension using
  the tangent points (real, selectable document points).

## Verification

`cargo check` and `cargo check --tests` pass. The repository still reports
its pre-existing warning set — no new warnings come from the fillet
subsystem. The evaluator tests (orientation matrix, acute/obtuse, Outer
mirror, degenerate, max-radius, arc, bezier) compile but have not been
executed in this session (`cargo test` was declined); run `cargo test
fillet` to execute them.
