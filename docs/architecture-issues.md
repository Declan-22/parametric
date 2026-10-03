# Architecture Issues — Solver, Constraints, Beziers, Editor

> Purpose: machine-readable issue registry for AI agents. Each entry is
> independently addressable. Fix in numeric order within a track unless
> stated otherwise. No rewrite: `core/` data model (points-as-entities,
> generational arenas) and the LM solver core are KEEP.

## Notation

- `ID`: stable issue id. Format `TRACK-NN` where TRACK ∈ {`SOL`,
  `ED`, `BEZ`, `UX`}.
- `LOC`: primary file(s) + symbol. Line numbers are approximate (Sep 2026).
- `SEV`: `P0` = wrong geometry / crash-y, `P1` = perf / freeze, `P2` = velocity.
- `FIX`: concrete change, not a wish. Each ends with `DONE WHEN:` (testable).

## Track SOL — solver / constraint communication

### SOL-01 — No constraint-graph layer; flat Vec compiled to flat Eqs
- `LOC`: `src/core/document.rs` (`constraints: Vec<Constraint>`,
  `dimensions: Vec<Dimension>`), `src/core/solver.rs:211-1017` (`Solver::build`).
- `SEV`: P0 (root cause of SOL-02..SOL-05).
- `SYMPTOM`: every new constraint type needs a new heuristic in `build()`.
- `CAUSE`: no DOF count, no subsystem decomposition, no redundancy /
  conflict sets. "Communication" between constraints = weight competition.
- `FIX`: insert `core/graph.rs`: adjacency → connected subsystems →
  per-subsystem DOF (`dof = 2*n_points - rank`). `Solver::build` takes ONE
  subsystem, not the whole BFS component. `DONE WHEN:` `cargo test graph`
  decomposes a rectangle (4 pts, 4 H/V eqs) as `dof=4` (rigid+scale) and two
  disjoint rectangles as 2 subsystems.

  Status (DONE): `core/graph.rs` — union-find over segment cliques +
  constraint/dimension links, per-subsystem equation counts mirroring the
  solver's `Eq` construction, `dof_estimate = 2n − eq`. 4 tests green
  (rectangle dof=4, disjoint pair, coincident/dimension joins, empty doc).
  Advisory only, no solver behavior changed. Consumer lands in SOL-04.

### SOL-02 — Priority by magic weights (ill-conditioning)
- `LOC`: `src/core/solver.rs:58-76` (`EQ_WEIGHT=1e9`, `DIM_WEIGHT=1e9`,
  `DRAG_WEIGHT=1e3`, `ANCHOR_WEIGHT=1.0`, `*_FACTOR = 2.5..10.0`).
- `SEV`: P0.
- `SYMPTOM`: drags teleport / jitter ("flies everywhere"). JᵀJ condition
  number ≈ (1e9/1.0)² ≈ 1e18 > f64 epsilon⁻¹ (≈1e16). Lambda + `STEP_CAP=50`
  mask it until they don't.
- `CAUSE`: hierarchy (hard constraint > drag target > anchor preference)
  encoded as floats instead of levels.
- `FIX`: hierarchical solve: (1) project hard Eqs exactly (existing
  `project_hard_constraints`), (2) solve drag targets in the nullspace,
  (3) anchors as Tikhonov on the remaining nullspace only. Remove all
  `*_FACTOR` constants. `DONE WHEN:` teleport repro (H-locked rect corner
  drag, 100 random cursor targets) never moves any point > `STEP_CAP` and
  `max_lin_residual` stays ≤1e-3 on satisfiable systems.

  Status (OPEN — two approaches tried and REVERTED Sep 2026, solver is
  byte-clean vs HEAD):
  (a) staged LM (hard unit-weight, then soft + projection) — broke
  SOL-03 barriers: stage-B acceptance compared soft cost only, so it
  walked across branches the joint cost had held. Flip-harness proved it.
  (b) anchors-out-of-JtJ + Euler relaxation — changed follower dynamics;
  triangles with angle dims glitched vs the joint solve, flip tests broke
  (solver build auto-generates aux followers, so nothing is aux-empty).
  Lesson: the joint LM + per-trial projection + full-cost acceptance is
  load-bearing DYNAMICS, not just conditioning. A future fix must preserve
  it exactly — candidates: Jacobi preconditioning inside assembly (no rows
  removed), or anchor subspace handled in acceptance. `*_FACTOR`s stay:
  intra-level relatives, preserve tuned feel. Drag-layer chase clamp +
  adaptive shrink (drag.rs, solver-untouched) kept as the freeze safety net.

### SOL-03 — Unsigned equations allow 180° branch flips
- `LOC`: `solver.rs` `Eq::Distance`, `Parallel` (`cross/m`), `Perpendicular`
  (`dot/m`), `Tangent` (`dot/m`), `Angle` wrap; barriers `DistanceBranch`,
  `ArcBend` (`solver.rs:95,112,1238,1453`).
- `SEV`: P0.
- `SYMPTOM`: arc inverts through chord, line flips to anti-parallel,
  radius dim settles mirrored branch.
- `CAUSE`: `cross/m=0` satisfied at 0° AND 180°. Barriers persist side only
  for arcs/distances, not for parallel/perp/tangent. Comment claims dragged
  points are "eliminated" (`solver.rs:8-14`) — false, they are soft
  (`residuals_into`, `solver.rs:1688-1704`), so LM can trade branch for
  drag-cost.
- `FIX`: per-Eq persistent `branch: i8` captured at gesture/commit start
  (sign of dot/cross/side); residual = `value` if branch matches else
  `value + BRANCH_PENALTY` (or directed residual `m - signed_value`).
  Covers Distance, Parallel, Perpendicular, Tangent, ArcRadius, ArcBend.
  `DONE WHEN:` flip-harness (drag each constrained edge across its chord /
  180°) asserts branch unchanged and no frame moves a point > 2× cursor
  delta.

  Status (DONE): `branch: f32` on Parallel/Perpendicular/Tangent/
  LineBezierTangent, captured at build (unit dot for cross-form eqs, unit
  cross for dot-form ones, 0 when degenerate), enforced by a barrier
  residual that is zero while matched (approaches never fight it) with
  analytic gradients, mirrored in `eq_residual_max` diagnostics. Fillet
  tangency unaffected (strip removes tangent + barrier together). 3
  flip-harness tests green; parallel + perpendicular verified to FAIL with
  barriers disabled (real flips) and pass with them; tangent holds either
  way (continuity) and locks the behavior.

### SOL-04 — Whole-component free set (no decomposition ⇒ O(n³) drags)
- `LOC`: `solver.rs:749-770` (component BFS), `editor/mod.rs:3474-3583`
  (transitive follower closure), `solver.rs:1796-1866` (denseiff `n≤128`
  else sparse CG), `MAX_ITER=60`.
- `SEV`: P1.
- `SYMPTOM`: extreme lag dragging heavily constrained objects; cost grows
  superlinearly with component size.
- `CAUSE`: every reachable point becomes a soft follower → `n_free`
  explodes → dense Gauss O(n³)×60/frame. Per-frame `HashMap` adjacency
  rebuild (`solver.rs:224-273`) + `Vec::contains` closure (O(n²)) on top.
- `FIX`: (a) persistent adjacency + dirty-bit incremental rebuild (see
  SOL-06); (b) articulation-point / block-triangular split, solve blocks
  smallest-first; (c) early-out when `cost < TOL` already exists — add
  per-block skip when block residual < tol. `DONE WHEN:` bench: 200-pt
  H/V grid corner drag p95 frame < 8 ms (measure with existing harness).

  Status (PARTIAL — anchor partition done): `Solver::build` now pins
  anchor-only followers (free vars touched by no active equation and no
  drag target) exactly at their anchors — their sole residuals are soft
  anchors, so the pinned optimum is bit-identical while the dense system
  shrinks cubically. `positions` coverage is unchanged (pinned slots still
  report), dragged slots are never eligible. Shared `eq_slots` helper
  replaced three copies of the variant→slots match. New test green
  (follower pinned, constrained pair still solves, output exact). True
  articulation/block splitting across the remaining equation graph is still
  open, as is the frame-time bench.

### SOL-05 — All-or-nothing commit (stuck drags)
- `LOC`: `editor/mod.rs:3642-3649`
  (`if !solution.constraints_satisfied() { return true; }` without applying).
- `SEV`: P1 (reads as freeze).
- `SYMPTOM`: heavily constrained drag refuses to move at all.
- `CAUSE`: correct intent (don't break locks) with no fallback: can't slide
  along feasible direction or report WHICH lock blocks.
- `FIX`: on reject, (1) return `SolveReject { max_lin, max_angle,
  worst_eq }` to `overconstraint.rs`, (2) attempt nullspace slide: project
  cursor delta onto feasible tangent (one J-nullspace step) and apply if it
  reduces drag residual without growing hard residual. Depends on SOL-01.
  `DONE WHEN:` infeasible pull moves along feasible axis ≥50% of cursor
  delta OR toast names the blocking Eq (see UX-01).

### SOL-06 — Per-frame allocation rebuild (HashMaps, closures, follower Vecs)
- `LOC`: `solver.rs:224-273`, `editor/mod.rs:3208-3292, 3474-3583`.
- `SEV`: P1.
- `SYMPTOM`: jank even on medium scenes; allocator churn visible in profiles.
- `CAUSE`: `Solver::build` reconstructs `adjacency/touch_line/touch_curve/
  index/slots` per mousemove; editor closure uses `Vec::contains` scans.
- `FIX`: `Document` owns `TopologyCache { adjacency, seg_owner, generation }`
  bumped on topology ops (`add/remove/merge_segment/point`) only; solver
  borrows it. Replace `Vec::contains` with `HashSet` (already used inside
  solver BFS — extend to editor closure).   `DONE WHEN:` drag-frame alloc
  count (measured via `#[bench]` or `tracing_alloc`) drops ≥5× on 100-pt doc.
  Editor-closure `Vec::contains` half still open (see SOL-06 status above).

  Status (DONE, solver half): `Topology` in `graph.rs` (adjacency +
  line/curve ownership, identical construction to the old inline code)
  cached on `Document` behind a structural fingerprint (ids, generations,
  wiring, kinds, targets, modes — positions/values/layers/fills ignored, so
  drags always hit cache). `Solver::build` borrows it instead of rebuilding
  three HashMaps per frame. `Document` got a manual `PartialEq` excluding
  the cache so history comparison and round-trips stay exact; clones share
  the `Rc` (no rebuild). 3 new tests (cache survives moves, rebuilds on
  rewire, equality ignores cache, adjacency parity). Full suite green.
  Editor-closure `Vec::contains` half still open.

## Track ED — editor structure

### ED-01 — `editor/mod.rs` god object (6736 lines / ~314 KB)
- `LOC`: `src/editor/mod.rs` (struct `Editor` ~50 fields; drag + pen +
  dims + constraint-apply + gating in one file).
- `SEV`: P2 (velocity; blocks all other fixes; blows AI context).
- `SYMPTOM`: merge conflicts, untestable drag logic, every fix touches one file.
- `FIX`: extract WITHOUT behavior change, one module per PR:
  `editor/session.rs` (struct + ctor), `editor/drag.rs` (`solve_drag`,
  follower closure, snap consensus), `editor/constraints_apply.rs`
  (`apply_constraint_from_menu`, `solve_constraint_now`),
  `editor/gates.rs` (`hv_candidates`, `coincident_candidates`,
  `tangent_candidate`, `required_coords`, `locked_dir`).
  `mod.rs` keeps re-exports only. `DONE WHEN:` `mod.rs` < 800 lines,
  `cargo test` green, no `pub(crate)` leakage beyond new modules.

  Status (ED-01 in progress — pure moves, each byte-verified vs HEAD):
  - DONE: `gates.rs` (~480 lines, gating candidates/feasibility),
    `drag.rs` (~1100, `solve_drag` + arc kinematics), `constraints_apply.rs`
    (~880, menu/tool creation + trial solves + exact passes), `pen.rs`
    (~830, chained path tool), `events.rs` (`~1440`, down/drag/up/hover).
    `mod.rs` 6945 → ~2440. Check clean, tests at baseline.
  - LEFT (approx lines, any order): dim-tool wiring (`resolve_dim_target`,
    `place_point_pair`, `dim_placement`, `dim_input_key`, `begin_dim_edit`,
    `dim_at`, `drag_point_pair`, `dim_drag_update`, `dim_escape`, ~650) →
    `dims_apply.rs`; inspector handlers (`begin_inspector_input`,
    `commit_inspector_input`, `inspector_nudge/scale/remove/flip/stroke/
    color/opacity`, ~175) → `inspector.rs`; creation
    (`create_rectangle/ruler/line/arc`, `tangent_snap_for_line`,
    `perpendicular_snap_for_line`, `maybe_add_tangent`,
    `arc_creation_shift`, ~300) → `create.rs`; document ops
    (`delete_element`, `merge_selected_points`, `queue_bond_menu`,
    `after_history_restore`, selection helpers, ~300) → `doc_ops.rs`;
    view (`begin/end_pan`, `pan_delta`, `zoom_*`, `selection_bounds`,
    `add_layer`, `visible_bounds`, ~150) → `view.rs`. None of these block
    the SOL/BEZ tracks — mop up independently.

### ED-02 — Three overlapping arc/tangent systems fight
- `LOC`: `plan_arc_drag` kinematics vs LM vs `enforce_tangencies()` post-pass
  vs `strip_fillet_equations()` vs `tangent_arc_pins`
  (`editor/mod.rs:3585-3656`).
- `SEV`: P0 (glitch source).
- `SYMPTOM`: arc warps on line drags; tangent bounces; fillet adjacent drags
  stretch one-sided.
- `CAUSE`: each subsystem corrects the others' output instead of one
  authoritative path.
- `FIX`: single authority rule: kinematic plan OWNS arc-defining points for
  the frame (pins, solver sees them fixed); solver OWNS everything else;
  post-pass becomes assert-only (`debug_assert` residual ≤ tol, no mutation).
  Depends on ED-01 (lives in `drag.rs`). `DONE WHEN:` post-pass deleted or
  assert-only; arc-line-tangent drag harness passes without pins special-case.

  Status (PARTIAL — fillet slice DONE Sep 2026, full rule still needs
  SOL-02 hierarchy): fillet scope already follows single authority —
  `plan_arc_drag` never leads fillet arcs, `tangent_arc_pins` skips fillet
  contacts, `strip_fillet_equations` keeps Radius while refresh owns the
  rest, `enforce_tangencies` skips fillet-adjacent. Locked by 2 tests in
  `editor/fillet.rs`: `fillet_edge_drag_keeps_both_contacts` (edge drag on
  filleted rect moves without freezing, H/V hold live, refresh re-seats
  both contacts with radius intact, no far-side leak, branch kept) and
  `fillet_tangency_postpass_is_readonly` (post-pass moves zero points in
  fillet scope even with tangency broken; refresh re-seats exactness).
  Removing `tangent_arc_pins` / making the general post-pass assert-only
  needs the SOL-02 hierarchy (reverted twice — pins are the current
  mechanism that stops tangent bounces), so the full DONE WHEN stays open.

## Track BEZ — bezier-specific perf

### BEZ-01 — `BezierLength` eq runs ~540×/frame/dim
- `LOC`: `solver.rs:1579-1621` (12-sample + Bernstein chain),
  called 9×/LM-iter (1 + 8 projection) × 60 iters.
- `SEV`: P1 (dominant solver cost when present).
- `SYMPTOM`: length-dimensioned beziers lag far more than any other dim.
- `FIX`: strip `BezierLength` from live drag solves (precedent:
  `strip_fillet_equations`); solve endpoints only per frame, enforce length
  exactly on commit/discrete apply. Keep full system for
  `solve_constraint_now` + validation.   `DONE WHEN:` live drag with 3
  length-dims matches fillet-strip frame time ±20%; commit still converges
  to ≤1e-3.

  Status (DONE): `Solver::strip_curve_length` drops `BezierLength` eqs from
  live drag solves (`solve_drag` calls it next to
  `strip_fillet_equations`); discrete applies keep the full system.
  `canvas_up` calls `enforce_dragged_curve_lengths` before history
  promotion, so violated length dims are re-established exactly inside the
  gesture's undo step. New solver test green (full system converges,
  stripped system solves, eq gone from both lists). Frame-time bench still
  open.

### BEZ-02 — Curve-dim replica bypasses cache + allocates per frame
- `LOC`: `editor/dims.rs:876-899` (`bezier_replica` → fresh `samples()` +
  offset `Vec` per dim per frame); `paint.rs:83-136` (cache exists but
  replica doesn't use it).
- `SEV`: P1.
- `SYMPTOM`: jank scales with number of curve-length dims, even static.
- `FIX`: route replica through `RenderCache::bezier_samples` + offset into
  reused buffer; store per-dim `Vec` in `CurveDimRender` reuse pool.
  `DONE WHEN:` zero allocs in `bezier_replica` hot path (reuse buffers).

  Status (DONE): `bezier_replica` takes caller samples instead of
  tessellating — resize path passes its scratch buffer, stored-dims path
  passes render-cache samples. Resize path: 3 tessellations + ~6 Vecs per
  bezier per frame → 2 tessellations into 1 shared scratch Vec (start,
  then current, gate, replica only on pass). Stored dims render at cache
  resolution now (was a bespoke count; subpixel delta).

### BEZ-03 — Linear mousemove scans with 48-eval nearest search
- `LOC`: `editor/bezier.rs:100-140` (`nearest_on_curve`),
  callers `editor/mod.rs:840, 2509`; `arc_length()` (`bezier.rs:80-83`)
  allocates 64-sample Vec per call (`mod.rs:5793`).
- `SEV`: P1.
- `SYMPTOM`: hover/pen-chaining lags with many beziers before solver runs.
- `FIX`: (a) cached-bbox reject before `nearest_on_curve` (bbox already in
  `BezEntry.bb` — reuse it, don't recompute cage); (b) coarse-to-fine search
  (12 then refine ±1 step at 48 resolution); (c) `arc_length` → stack-buffer
  16-sample approx for UI, exact 64 only on commit. `DONE WHEN:` 100-bezier
  mousemove (no drag) p95 < 4 ms.

  Status (SHELVED Sep 2026 — beziers abandoned for now, fillets are the
  curve story; revisit only if pen-chaining perf bites).

### BEZ-04 — Drag retessellates every bezier every frame
- `LOC`: `paint.rs:101-135` (fingerprint = full coord bits → miss on any move),
  `bezier::adaptive_samples` up to 160/curve ("each sample becomes a GPU
  primitive").
- `SEV`: P1.
- `SYMPTOM`: dragging any bezier re-flattens ALL beziers.
- `CAUSE`: correct (geometry changed) but unculled: fingerprint check is
  per-curve (good) yet every touched curve still re-flattens fully, and
  untouched curves re-check hash per frame.
- `FIX`: (a) viewport-cull before fingerprint (skip offscreen curves using
  cached `bb`); (b) cap live-drag tessellation (e.g. 64 max during
  `dragging.is_some()`, full on commit).   `DONE WHEN:` offscreen beziers add
  ~0 cost during drags; on-screen drag tessellation ≤ half of commit.

  Status (DONE): cage pre-cull in the paint pass skips cache tessellation
  for fully-offscreen spans (handles still draw; entries refresh on return
  via fingerprint mismatch). `RenderCache.tess_cap` (part of the
  fingerprint) clamps live-drag tessellation to 64, full on release —
  huge curves facet slightly mid-drag, then refine. BEZ-03 still open.

## Track UX — overconstraint reporting

### UX-01 — Solver reports residuals; toasts guess
- `LOC`: `solver.rs:1945-1951` (`max_lin/max_angle`) vs
  `editor/overconstraint.rs` (`required_coords`/`locked_dir` inspection).
- `SEV`: P1 (user can't fix what isn't named).
- `SYMPTOM`: "Dimension conflicts — relax a lock" without saying which Eq.
- `FIX`: solver returns `WorstEq { kind, point_ids, residual }` (top-3 by
  `weight*value²`); `overconstraint.rs` maps Eq→existing Constraint/Dimension
  rows instead of re-inferring. Depends on SOL-01 (subsystem-local Eqs).
  `DONE WHEN:` every `SolveReject` toast names ≥1 exact blocking row id.

### MEM-01 — Uncapped full-document undo stack
- `LOC`: `src/commands/undo.rs` (`flush_pending_history`).
- `SEV`: P1 (unbounded session growth; reported 40→95→250MB on drag-heavy use).
- `CAUSE`: every mutating gesture pushes a full `Document` clone, retained
  forever. Toasts drain per frame, `dim_hitboxes` rebuild, render caches are
  bounded — the undo stack was the only unbounded retainer.
- `FIX`: cap at 100 steps (drop oldest). `DONE WHEN:` 150-gesture test holds
  `len == 100` and undo still works.
- Status (DONE): capped + test green. If RSS still climbs, profile before
  further cuts (remaining per-frame work is transient).

## Fix order (recommended)

1. `ED-01` (unblocks everything, zero behavior risk).
2. `SOL-01` + `SOL-06` (graph + incremental topology; foundation).
3. `SOL-03` (branch persistence; kills "flying" class).
4. `SOL-02` (hierarchy; kills weight-tuning bandaids; delete `*_FACTOR`s).
5. `BEZ-01` + `BEZ-02` (biggest perf wins, small diffs).
6. `SOL-04` + `BEZ-03` + `BEZ-04` (scale work).
7. `SOL-05` + `UX-01` + `ED-02` (feel + reporting).

## Non-goals (do NOT do in these fixes)

- No solver-algorithm swap (LM stays). No `Document` schema change. No
  persistence change. No new constraint TYPES until SOL-01..SOL-03 land —
  new types on the current architecture add heuristics, not capability.
