use crate::core::constraints::{Constraint, ConstraintKind, DimMode, DimTarget, ElementRef};
use crate::core::document::{Document, SegmentKind};
use crate::core::fillet::{EvaluatedFillet, Fillet, FilletSide};
use crate::core::geometry::Point2;
use crate::core::ids::{PointId, SegmentId};
use crate::editor::{BlockChip, Editor, LockKind, ToastRequest, HANDLE_TOL_PX};
use crate::editor::pick::{self, Picker};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilletPreview {
    pub first: SegmentId,
    pub second: SegmentId,
    pub geometry: EvaluatedFillet,
}

/// Live radius-drag gesture. The grab point is the center itself, where an
/// absolute cursor distance would always read ~0 (the "restart from zero"
/// bug) — so the drag is RELATIVE: cursor displacement projected onto the
/// outward (corner→center) axis, added to the grab radius. Pulling outward
/// grows the fillet, pushing inward shrinks it.
#[derive(Clone, Copy, Debug)]
pub struct FilletRadiusDrag {
    pub modifier_index: usize,
    /// Grab position (doc units).
    pub down: Point2,
    /// Unit outward (corner→center) direction captured at grab.
    pub dir: (f64, f64),
    /// Radius at grab.
    pub start_radius: f64,
}

/// A staged center/tangent press, waiting past the click threshold to
/// convert into a real gesture. Lets plain clicks (fill selects, edge
/// selects, marquees) pass through instead of hijacking them.
#[derive(Clone, Copy, Debug)]
pub struct FilletPress {
    pub modifier_index: usize,
    pub down: Point2,
}

/// Live corner-drag: with a committed radius dimension the center handle
/// drags the virtual corner instead of resizing. Targets are a rigid
/// translation of both tangent points by the cursor delta (never
/// both-to-one-point, which is structurally infeasible under a fixed
/// radius); H/V, tangency, and the locked radius shape the answer.
#[derive(Clone, Copy, Debug)]
pub struct FilletCornerDrag {
    pub modifier_index: usize,
    /// Press position (doc units): cursor delta is measured from here.
    pub down: Point2,
    /// Tangent positions at grab: targets stay rigid to these, so the
    /// first frame has zero delta and nothing jumps.
    pub t1_start: Point2,
    pub t2_start: Point2,
}

/// Default creation radius; clamped down to what the picked edges fit.
const DEFAULT_RADIUS: f64 = 24.0;
/// Nothing usable fits below this — creation fails with a toast instead.
const MIN_USABLE_RADIUS: f64 = 1.0;

/// Shared corner of two segments, preferring the one nearest the cursor
/// when they share more than one endpoint (closed loops, doubled edges).
/// None when the pair shares no endpoint.
fn shared_corner(
    doc: &Document,
    a: SegmentId,
    b: SegmentId,
    near: Option<Point2>,
) -> Option<PointId> {
    let (sa, sb) = (doc.segment(a)?, doc.segment(b)?);
    let mut shared = vec![sa.start, sa.end];
    shared.retain(|p| [sb.start, sb.end].contains(p));
    shared.dedup();
    if shared.len() == 1 {
        return Some(shared[0]);
    }
    let at = near?;
    shared.into_iter().min_by(|x, y| {
        let dist = |id: PointId| {
            doc.point(id)
                .map(|p| pick::distance(p, at))
                .unwrap_or(f64::MAX)
        };
        dist(*x)
            .partial_cmp(&dist(*y))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Angular fraction of `ang` inside the arc span from `a1` to `a2`,
/// measured the way that passes through `a_ctrl` (the fillet's own
/// branch), clamped to the span. Keeps dim leaders and arrows on the
/// fillet edge instead of wandering onto extensions.
pub(crate) fn span_fraction(a1: f64, a2: f64, a_ctrl: f64, ang: f64) -> f64 {
    const TAU: f64 = std::f64::consts::PI * 2.;
    let d = (a2 - a1).rem_euclid(TAU);
    let ang_diff = |x: f64| (x - a_ctrl).rem_euclid(TAU).min((a_ctrl - x).rem_euclid(TAU));
    // Midpoint of each candidate direction; the one nearer ctrl wins.
    let mid_pos = a1 + d / 2.;
    let mid_neg = a1 - (TAU - d) / 2.;
    let (from, span) = if ang_diff(mid_pos) <= ang_diff(mid_neg) {
        (a1, d)
    } else {
        (a1, -(TAU - d))
    };
    if span.abs() < 1e-9 {
        return 0.5;
    }
    ((ang - from) / span).clamp(0., 1.)
}

/// Angle at span fraction `f`, using the same branch as `span_fraction`.
pub(crate) fn span_angle(a1: f64, a2: f64, a_ctrl: f64, f: f64) -> f64 {
    const TAU: f64 = std::f64::consts::PI * 2.;
    let d = (a2 - a1).rem_euclid(TAU);
    let ang_diff = |x: f64| (x - a_ctrl).rem_euclid(TAU).min((a_ctrl - x).rem_euclid(TAU));
    let mid_pos = a1 + d / 2.;
    let mid_neg = a1 - (TAU - d) / 2.;
    if ang_diff(mid_pos) <= ang_diff(mid_neg) {
        a1 + d * f.clamp(0., 1.)
    } else {
        a1 - (TAU - d) * f.clamp(0., 1.)
    }
}

/// Nearer tangent point to a reference position (t1 on any doubt).
fn nearer_pos(doc: &Document, ref_pos: Point2, t1: PointId, t2: PointId) -> PointId {
    let (p1, p2) = (doc.point(t1), doc.point(t2));
    match (p1, p2) {
        (Some(p1), Some(p2)) => {
            let d1 = (p1.x - ref_pos.x).powi(2) + (p1.y - ref_pos.y).powi(2);
            let d2 = (p2.x - ref_pos.x).powi(2) + (p2.y - ref_pos.y).powi(2);
            if d1 <= d2 {
                t1
            } else {
                t2
            }
        }
        _ => t1,
    }
}

/// True when a solved control sits on the mirrored branch (arc flipped
/// inside-out): the solved chord side disagrees with the exactly
/// evaluated one. Radius equations are branch-agnostic distances, so
/// without this check a solve can commit the complement arc and
/// obliterate the fill. Degenerate (near-flat) arcs have no meaningful
/// side and always pass.
fn control_flipped(
    solved_t1: Point2,
    solved_t2: Point2,
    solved_ctrl: Point2,
    exact_t1: Point2,
    exact_t2: Point2,
    exact_ctrl: Point2,
) -> bool {
    let s = (exact_t2.x - exact_t1.x) * (exact_ctrl.y - exact_t1.y)
        - (exact_t2.y - exact_t1.y) * (exact_ctrl.x - exact_t1.x);
    if s.abs() < 1e-9 {
        return false;
    }
    let q = (solved_t2.x - solved_t1.x) * (solved_ctrl.y - solved_t1.y)
        - (solved_t2.y - solved_t1.y) * (solved_ctrl.x - solved_t1.x);
    s * q < 0.
}

/// Nearer tangent point to another point id.
fn nearer_id(doc: &Document, other: PointId, t1: PointId, t2: PointId) -> PointId {
    nearer_pos(
        doc,
        doc.point(other).unwrap_or(Point2::new(0., 0.)),
        t1,
        t2,
    )
}

/// Transitive component closure over segments (endpoints + construction
/// slots) and constraints (pairs + point-on-segment edges). Shared by the
/// fillet trial solves so radius edits and corner drags free the same
/// neighborhood.
fn component_points(doc: &Document, mut seeds: Vec<PointId>) -> Vec<PointId> {
    let mut i = 0;
    while i < seeds.len() {
        let pid = seeds[i];
        for (_, s) in doc.all_segments() {
            if s.start == pid || s.end == pid || s.ctrl == Some(pid) || s.center == Some(pid) {
                for linked in [Some(s.start), Some(s.end), s.ctrl, s.center]
                    .into_iter()
                    .flatten()
                {
                    if doc.point(linked).is_some() && !seeds.contains(&linked) {
                        seeds.push(linked);
                    }
                }
            }
        }
        for c in &doc.constraints {
            let mut linked = c.a == pid || c.b == pid;
            if let Some(segment_id) = c.point_on_segment
                && let Some(seg) = doc.segment(segment_id)
            {
                linked |= seg.start == pid
                    || seg.end == pid
                    || seg.ctrl == Some(pid)
                    || seg.center == Some(pid);
                if linked {
                    for point in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center]
                        .into_iter()
                        .flatten()
                    {
                        if doc.point(point).is_some() && !seeds.contains(&point) {
                            seeds.push(point);
                        }
                    }
                }
            }
            if linked {
                for point in [c.a, c.b] {
                    if doc.point(point).is_some() && !seeds.contains(&point) {
                        seeds.push(point);
                    }
                }
            }
        }
        i += 1;
    }
    seeds
}

/// Re-measures every point-pair / point-line dimension touching `pid`
/// from live positions. Used after topology surgery (subdivide/heal)
/// so dims keep driving their new flats instead of stale spans.
fn revalue_dims_for_point(doc: &mut Document, pid: PointId) {
    struct Pending {
        idx: usize,
        value: f64,
    }
    let mut pending = Vec::new();
    for (idx, d) in doc.dimensions.iter().enumerate() {
        let value = match d.target {
            DimTarget::Points { a, b, mode } if a == pid || b == pid => {
                let (Some(pa), Some(pb)) = (doc.point(a), doc.point(b)) else {
                    continue;
                };
                Some(match mode {
                    DimMode::Aligned => {
                        ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2)).sqrt()
                    }
                    DimMode::X => (pb.x - pa.x).abs(),
                    DimMode::Y => (pb.y - pa.y).abs(),
                })
            }
            DimTarget::PointLine { p, line } if p == pid => {
                let Some(seg) = doc.segment(line) else {
                    continue;
                };
                let (Some(pp), Some(la), Some(lb)) = (
                    doc.point(p),
                    doc.point(seg.start),
                    doc.point(seg.end),
                ) else {
                    continue;
                };
                let (dx, dy) = (lb.x - la.x, lb.y - la.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                Some(((pp.x - la.x) * (-dy / l) + (pp.y - la.y) * (dx / l)).abs())
            }
            _ => None,
        };
        if let Some(value) = value {
            pending.push(Pending { idx, value });
        }
    }
    for p in pending {
        if let Some(d) = doc.dimensions.get_mut(p.idx) {
            d.value = p.value;
        }
    }
}

/// Maps one constraint off a consumed filleted corner onto a tangent
/// point. Edge-affine references follow their edge exactly (the rectangle
/// case); H/V also survive an exactness check (identical coordinate as
/// the other end); tangent-family point refs ride to the nearer tangent
/// (their equations read segments, not these points). None when the
/// constraint can't keep its meaning without the corner.
fn map_dead_corner(
    doc: &Document,
    mut c: Constraint,
    corner: PointId,
    t1: PointId,
    t2: PointId,
    far1: PointId,
    far2: PointId,
) -> Option<Constraint> {
    if c.a == corner && c.b == corner {
        return None;
    }
    let nearer = |doc: &Document, other: PointId| -> PointId {
        let (op, p1, p2) = (doc.point(other), doc.point(t1), doc.point(t2));
        match (op, p1, p2) {
            (Some(op), Some(p1), Some(p2)) => {
                let d1 = (p1.x - op.x).powi(2) + (p1.y - op.y).powi(2);
                let d2 = (p2.x - op.x).powi(2) + (p2.y - op.y).powi(2);
                if d1 <= d2 { t1 } else { t2 }
            }
            _ => t1,
        }
    };
    match c.kind {
        ConstraintKind::Horizontal | ConstraintKind::Vertical => {
            let other = if c.a == corner { c.b } else { c.a };
            let cand = if other == far1 {
                Some(t1)
            } else if other == far2 {
                Some(t2)
            } else {
                // Exactness fallback only: the equation must read
                // bit-identically after migration.
                let (op, p1, p2) = (doc.point(other)?, doc.point(t1)?, doc.point(t2)?);
                let eq = |p: Point2| {
                    if c.kind == ConstraintKind::Horizontal {
                        (p.y - op.y).abs() <= 1e-6
                    } else {
                        (p.x - op.x).abs() <= 1e-6
                    }
                };
                match (eq(p1), eq(p2)) {
                    (true, false) => Some(t1),
                    (false, true) => Some(t2),
                    (true, true) => Some(nearer(doc, other)),
                    _ => None,
                }
            }?;
            if c.a == corner {
                c.a = cand;
            } else {
                c.b = cand;
            }
            Some(c)
        }
        ConstraintKind::Coincident => {
            // Affinity only: a coincident glue can't survive a position
            // change, so only same-edge references migrate.
            if c.a == corner {
                c.a = if c.b == far1 {
                    t1
                } else if c.b == far2 {
                    t2
                } else {
                    return None;
                };
            }
            if c.b == corner {
                c.b = if c.a == far1 {
                    t1
                } else if c.a == far2 {
                    t2
                } else {
                    return None;
                };
            }
            Some(c)
        }
        ConstraintKind::Tangent | ConstraintKind::Parallel | ConstraintKind::Perpendicular => {
            if c.a == corner {
                c.a = nearer(doc, c.b);
            }
            if c.b == corner {
                c.b = nearer(doc, c.a);
            }
            Some(c)
        }
    }
}

impl Editor {
    /// Topological subdivision for LINE-line fillets, run once at commit:
    /// both sources are rewired to the tangent points, edge-affine
    /// constraints/dims follow their edge (dims revalued to the new
    /// flats), the arc is spliced into adjacent fill loops, unmigratable
    /// corner constraints are dropped (counted for the toast), and the
    /// corner point is consumed. Returns the drop count, or None when the
    /// pair isn't subdividable (curves keep the legacy model: the corner
    /// stays referenced and live).
    fn subdivide_corner(
        &mut self,
        first: SegmentId,
        second: SegmentId,
        corner: PointId,
        t1: PointId,
        t2: PointId,
        arc: SegmentId,
    ) -> Option<usize> {
        let (sa, sb) = (self.doc.segment(first)?, self.doc.segment(second)?);
        if sa.kind != SegmentKind::Line || sb.kind != SegmentKind::Line {
            return None;
        }
        if ![sa.start, sa.end].contains(&corner) || ![sb.start, sb.end].contains(&corner) {
            return None;
        }
        let far1 = if sa.start == corner { sa.end } else { sa.start };
        let far2 = if sb.start == corner { sb.end } else { sb.start };
        // 1. Rewire sources to tangent points.
        if let Some(s) = self.doc.segment_mut(first) {
            if s.start == corner {
                s.start = t1;
            } else {
                s.end = t1;
            }
        }
        if let Some(s) = self.doc.segment_mut(second) {
            if s.start == corner {
                s.start = t2;
            } else {
                s.end = t2;
            }
        }
        // 2. Migrate constraints off the dead corner (order-preserving
        // rebuild; unmigratable ones are dropped and counted).
        let mut dropped = 0usize;
        let old = std::mem::take(&mut self.doc.constraints);
        let mut kept = Vec::with_capacity(old.len());
        for c in old {
            if c.a != corner && c.b != corner {
                kept.push(c);
                continue;
            }
            match map_dead_corner(&self.doc, c, corner, t1, t2, far1, far2) {
                Some(mapped) => kept.push(mapped),
                None => dropped += 1,
            }
        }
        self.doc.constraints = kept;
        // 3. Migrate point dims off the corner (edge affinity, else the
        // nearer tangent — dims always survive the split), then revalue
        // them to the new flats. Two passes so values derive from final
        // topology; t1/t2 are fresh ids, so revaluing them hits exactly
        // the migrated set.
        enum Rewrite {
            Points(usize, PointId, PointId),
            PointLine(usize, PointId),
        }
        let mut rewrites = Vec::new();
        for (idx, d) in self.doc.dimensions.iter().enumerate() {
            match d.target {
                DimTarget::Points { a, b, .. } => {
                    let na = if a == corner {
                        Some(if b == far1 {
                            t1
                        } else if b == far2 {
                            t2
                        } else {
                            nearer_id(&self.doc, b, t1, t2)
                        })
                    } else {
                        None
                    };
                    let nb = if b == corner {
                        Some(if a == far1 {
                            t1
                        } else if a == far2 {
                            t2
                        } else {
                            nearer_id(&self.doc, a, t1, t2)
                        })
                    } else {
                        None
                    };
                    if na.is_some() || nb.is_some() {
                        rewrites.push(Rewrite::Points(idx, na.unwrap_or(a), nb.unwrap_or(b)));
                    }
                }
                DimTarget::PointLine { p, line } if p == corner => {
                    // The measured point rode the corner itself; take the
                    // nearer tangent to the line's far end.
                    let anchor = self
                        .doc
                        .segment(line)
                        .and_then(|s| self.doc.point(s.end))
                        .unwrap_or(Point2::new(0., 0.));
                    rewrites.push(Rewrite::PointLine(
                        idx,
                        nearer_pos(&self.doc, anchor, t1, t2),
                    ));
                }
                _ => {}
            }
        }
        for r in rewrites {
            match r {
                Rewrite::Points(idx, a, b) => {
                    if let Some(d) = self.doc.dimensions.get_mut(idx) {
                        if let DimTarget::Points { a: ra, b: rb, .. } = &mut d.target {
                            *ra = a;
                            *rb = b;
                        }
                    }
                }
                Rewrite::PointLine(idx, p) => {
                    if let Some(d) = self.doc.dimensions.get_mut(idx) {
                        if let DimTarget::PointLine { p: rp, .. } = &mut d.target {
                            *rp = p;
                        }
                    }
                }
            }
        }
        revalue_dims_for_point(&mut self.doc, t1);
        revalue_dims_for_point(&mut self.doc, t2);
        // 4. Splice the arc into adjacent fill loops (order-agnostic) so
        // the loop chains through the new topology; non-adjacent loops
        // keep the legacy corner substitution as fallback.
        let mut loops = Vec::new();
        for (fid, f) in self.doc.all_fills() {
            if f.segments.contains(&first)
                && f.segments.contains(&second)
                && !f.segments.contains(&arc)
            {
                loops.push(fid);
            }
        }
        for fid in loops {
            if let Some(f) = self.doc.fill_mut(fid) {
                let n = f.segments.len();
                let mut pos = None;
                for i in 0..n {
                    let (x, y) = (f.segments[i], f.segments[(i + 1) % n]);
                    if (x == first && y == second) || (x == second && y == first) {
                        pos = Some(i);
                        break;
                    }
                }
                if let Some(i) = pos {
                    f.segments.insert((i + 1) % (n + 1), arc);
                }
            }
        }
        // 5. Layer the new endpoints with the arc; consume the corner with
        // a non-cascading removal (everything referencing it was migrated
        // or dropped above — remove_point would eat the rewired edges).
        if let Some(layer) = self.doc.layers.first().map(|l| l.id) {
            self.doc.push_to_layer(layer, ElementRef::Point(t1));
            self.doc.push_to_layer(layer, ElementRef::Point(t2));
        }
        self.doc.remove_point_raw(corner);
        Some(dropped)
    }

    pub fn refresh_fillets(&mut self) {
        let mods = self.doc.modifiers.clone();
        for m in mods {
            let Some(g) = m.evaluate(&self.doc) else { continue; };
            for (id, p) in [(m.first_tangent, g.first_tangent), (m.second_tangent, g.second_tangent), (m.center, g.center), (m.control, g.control)] {
                if let Some(id) = id { self.doc.move_point(id, p); }
            }
        }
    }

    /// Exact geometric rebuild of one modifier's derived points. Silent
    /// (no toasts): the solver path and the drag gesture clamp first, so
    /// evaluation is expected to succeed here.
    fn rebuild_modifier(&mut self, index: usize) -> bool {
        let Some(m) = self.doc.modifiers.get(index).copied() else { return false };
        let Some(g) = m.evaluate(&self.doc) else { return false };
        if let Some(p) = m.first_tangent { self.doc.move_point(p, g.first_tangent); }
        if let Some(p) = m.second_tangent { self.doc.move_point(p, g.second_tangent); }
        if let Some(p) = m.center { self.doc.move_point(p, g.center); }
        if let Some(p) = m.control { self.doc.move_point(p, g.control); }
        true
    }

    /// Solves the modifier's component for a new radius in one continuous
    /// system: the trial is seeded at the exact new geometry (so
    /// radius-captured equations agree with the dimension from the
    /// start), then the radius dimension, tangency, and point-on-line
    /// equations settle the connected geometry jointly — existing dims
    /// fight honestly (the shape accommodates or the solve fails) and
    /// are never silently rewritten. History-free: callers own the
    /// snapshot (typed edits wrap once; drags rely on press/release).
    /// Returns true on a committable solve.
    fn solve_fillet_radius(&mut self, index: usize, radius: f64) -> bool {
        let Some(m) = self.doc.modifiers.get(index).copied() else { return false };
        let (Some(t1), Some(t2), Some(ctr), Some(ctl)) =
            (m.first_tangent, m.second_tangent, m.center, m.control)
        else {
            return false;
        };
        let (Some(sa), Some(sb)) = (self.doc.segment(m.first), self.doc.segment(m.second))
        else {
            return false;
        };
        let mut trial = self.doc.clone();
        trial.modifiers[index].radius = radius;
        // The radius dimension is optional (deleted dims drag dim-less):
        // when present it drives the solve, when absent the seeded exact
        // geometry plus tangency carry it.
        if let Some(dim) = trial.dimensions.iter_mut().find(|d| {
            matches!(d.target, DimTarget::Radius { seg } if seg == m.arc.unwrap_or(crate::core::ids::SegmentId::NONE))
        }) {
            dim.value = radius;
        }
        // Seed the trial at the exact new geometry first: radius-captured
        // equations (CirclePoint) read positions at build, so seeding makes
        // them agree with the dimension from the start. The solve then only
        // relaxes connected geometry jointly instead of fighting stale
        // positions.
        let trial_m = trial.modifiers[index];
        if let Some(g) = trial_m.evaluate(&trial) {
            for (id, p) in [
                (trial_m.first_tangent, g.first_tangent),
                (trial_m.second_tangent, g.second_tangent),
                (trial_m.center, g.center),
                (trial_m.control, g.control),
            ] {
                if let Some(id) = id {
                    trial.move_point(id, p);
                }
            }
        } else {
            return false;
        }
        // Free set: derived points + source endpoints, closed transitively.
        let seeds = component_points(
            &trial,
            vec![t1, t2, ctr, ctl, sa.start, sa.end, sb.start, sb.end],
        );
        let aux: Vec<(PointId, Point2)> = seeds
            .iter()
            .filter_map(|&pid| trial.point(pid).map(|p| (pid, p)))
            .collect();
        // The live corner stays hard-fixed so a radius edit reshapes
        // around it instead of relocating it. Subdivided corners are
        // already dead — nothing to pin, the anchors regularize.
        let pins: Vec<(PointId, Point2)> = trial
            .point(m.corner)
            .map(|point| (m.corner, point))
            .into_iter()
            .collect();
        let solver =
            crate::core::solver::Solver::build_pinned(&trial, &[], &aux, &pins, 2.0);
        let solution = solver.solve();
        if !solution.is_valid()
            || solution.max_angle_residual > 1e-5
            || solution.max_lin_residual > 1e-3
        {
            return false;
        }
        // Branch guard: a mirror-branch commit would flip the arc
        // inside-out (radius equations can't tell branches apart). The
        // geometric fallback rebuilds the true branch by construction.
        {
            let at = |id: PointId| {
                solution
                    .positions
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, p)| *p)
            };
            if let Some(g) = trial.modifiers[index].evaluate(&trial) {
                if let (Some(t1p), Some(t2p), Some(cp)) = (at(t1), at(t2), at(ctl)) {
                    if control_flipped(
                        t1p,
                        t2p,
                        cp,
                        g.first_tangent,
                        g.second_tangent,
                        g.control,
                    ) {
                        return false;
                    }
                }
            }
        }
        self.doc = trial;
        for (id, pos) in solution.positions {
            self.doc.move_point(id, pos);
        }
        // Exactness pass: snaps the derived points (and curve-source
        // corners the solver only approximates) onto the evaluated arc.
        self.refresh_fillets();
        true
    }

    /// Flat-touching dims (point-pair / point-line referencing a tangent
    /// point): the locks a radius change must respect, named for toasts.
    fn flat_lock_chips(&self, t1: PointId, t2: PointId) -> Vec<BlockChip> {
        let mut out = Vec::new();
        for d in &self.doc.dimensions {
            let label = match d.target {
                DimTarget::Points { a, b, mode } if a == t1 || b == t1 || a == t2 || b == t2 => {
                    let what = match mode {
                        DimMode::Aligned => "Distance",
                        DimMode::X => "Width",
                        DimMode::Y => "Height",
                    };
                    Some(format!("{} = {:.2}", what, d.value))
                }
                DimTarget::PointLine { p, .. } if p == t1 || p == t2 => {
                    Some(format!("Point-line = {:.2}", d.value))
                }
                _ => None,
            };
            if let Some(label) = label {
                if !out.iter().any(|c: &BlockChip| c.label == label) {
                    out.push(BlockChip { kind: LockKind::Dimension, label });
                }
            }
            if out.len() >= 3 {
                break;
            }
        }
        out
    }

    /// Applies a typed radius to a fillet arc, clamped to what the source
    /// edges fit. Solves jointly (existing dims fight honestly — the shape
    /// accommodates both or the edit is refused with an explanation, never
    /// silently rewritten). Without flat locks, a solver miss falls back
    /// to the exact geometric rebuild. Returns the applied radius plus the
    /// resolved radius-dim index (the caller writes values only there, so
    /// a dim deleted mid-edit can never corrupt an unrelated row), or None
    /// when the modifier itself is unlinkable (caller keeps legacy flow).
    pub fn update_fillet_radius(
        &mut self,
        arc: SegmentId,
        radius: f64,
    ) -> Option<(f64, Option<usize>)> {
        let Some((index, old)) = self.doc.modifiers.iter().enumerate().find_map(|(i,m)| (m.arc == Some(arc)).then_some((i,*m))) else { return None; };
        let max_r = old.max_radius(&self.doc).unwrap_or(f64::INFINITY);
        let clamped = radius.max(0.01).min(max_r);
        let dim_idx = self.doc.dimensions.iter().position(|d| {
            matches!(d.target, DimTarget::Radius { seg } if seg == arc)
        });
        self.history_begin();
        let applied = if self.solve_fillet_radius(index, clamped) {
            self.flush_pending_history();
            clamped
        } else {
            // No silent rewrites: with flat locks aboard, a miss means
            // genuine conflict — refuse loudly and change nothing.
            let (t1, t2) = (old.first_tangent, old.second_tangent);
            let locks = match (t1, t2) {
                (Some(a), Some(b)) => self.flat_lock_chips(a, b),
                _ => Vec::new(),
            };
            if !locks.is_empty() {
                self.gesture_snapshot = None;
                self.toast_requests.push(ToastRequest::error(
                    "Can't set radius",
                    format!(
                        "R{:.2} needs room the locked dims don't allow.",
                        clamped,
                    ),
                    "Delete or relax a dimension, then retry.",
                    locks,
                ));
                return Some((old.radius, dim_idx));
            }
            // No locks involved: exact geometric rebuild always lands.
            self.doc.modifiers[index].radius = clamped;
            if !self.rebuild_modifier(index) {
                self.gesture_snapshot = None;
                return Some((old.radius, dim_idx));
            }
            self.flush_pending_history();
            clamped
        };
        if clamped < radius {
            self.toast_requests.push(ToastRequest::info(
                "Radius clamped",
                format!("R{:.2} doesn't fit — applied R{:.2}.", radius, clamped),
                "Lengthen an edge for a bigger fillet.",
                Vec::new(),
            ));
        }
        Some((applied, dim_idx))
    }

    /// Modifier index whose center/tangent sits under the cursor, if any.
    /// Centers first (the explicit handle), then tangent points.
    pub fn fillet_handle_at(&self, at: Point2) -> Option<usize> {
        let tol = HANDLE_TOL_PX / self.camera.zoom.max(1e-6);
        self.doc.modifiers.iter().enumerate().find_map(|(i, m)| {
            let center_hit = m
                .center
                .and_then(|id| self.doc.point(id))
                .is_some_and(|c| pick::distance(c, at) <= tol);
            let tangent_hit = [m.first_tangent, m.second_tangent]
                .into_iter()
                .flatten()
                .any(|t| {
                    self.doc
                        .point(t)
                        .is_some_and(|p| pick::distance(p, at) <= tol)
                });
            (center_hit || tangent_hit).then_some(i)
        })
    }

    /// Routes a fillet grab (center, tangent point, or converted edge
    /// press) by lock state: committed dimension → corner-drag, otherwise
    /// radius resize. Shared so every entry point behaves identically.
    pub(crate) fn route_fillet_grab(&mut self, modifier_index: usize, down: Point2) -> bool {
        let m = self.doc.modifiers[modifier_index];
        let dim_info = m.arc.and_then(|arc| {
            self.doc
                .dimensions
                .iter()
                .enumerate()
                .find(|(_, d)| matches!(d.target, DimTarget::Radius { seg } if seg == arc))
                .map(|(idx, d)| (idx, d.value))
        });
        // A committed dimension owns the radius, so the handle can't
        // resize anymore — instead it drags the virtual corner, resizing
        // from the two adjacent edges at fixed radius (below). Only the
        // still-open input for that same dimension (the provisional
        // placement flow), or no dimension at all, keeps resize alive.
        if let Some((idx, _)) = dim_info {
            let provisional = self
                .dim_input
                .as_ref()
                .is_some_and(|input| input.existing == Some(idx));
            if !provisional {
                return self.begin_fillet_corner_drag(modifier_index, down);
            }
        }
        // Drag axis: outward along center→corner reversed — pulling the
        // dot away from the shape grows the fillet, pushing it in
        // shrinks it. (An absolute cursor distance would read ~0 at the
        // grab, and the previous inward axis scaled in reverse.)
        let center_pos = m.center.and_then(|id| self.doc.point(id));
        let corner_pos = self
            .doc
            .point(m.corner)
            .or_else(|| crate::core::fillet::line_corner(&self.doc, m.first, m.second));
        let (dx, dy) = match (center_pos, corner_pos) {
            (Some(c), Some(p)) => (c.x - p.x, c.y - p.y),
            // No axis and no committed dim: nothing sane to drive (a
            // corner-drag without its radius equation would fight the
            // refresh snap). Committed dims route to the corner drag.
            _ => {
                let has_dim = m.arc.is_some_and(|arc| {
                    self.doc.dimensions.iter().any(|d| {
                        matches!(d.target, DimTarget::Radius { seg } if seg == arc)
                    })
                });
                return has_dim && self.begin_fillet_corner_drag(modifier_index, down);
            }
        };
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        self.fillet_radius_drag = Some(FilletRadiusDrag {
            modifier_index,
            down,
            dir: (dx / l, dy / l),
            start_radius: m.radius,
        });
        if let Some(arc) = m.arc {
            self.selection = vec![ElementRef::Segment(arc)];
        }
        true
    }

    /// Begins a corner-drag: with a committed radius dimension the center
    /// handle stops resizing and instead drags the virtual corner — both
    /// tangent points chase the cursor while H/V, tangency, and the locked
    /// radius shape the answer (resize from the adjacent edges). Needs
    /// resolved tangent points; legacy unlinkable modifiers keep the
    /// locked toast instead.
    pub fn begin_fillet_corner_drag(&mut self, modifier_index: usize, down: Point2) -> bool {
        let Some(m) = self.doc.modifiers.get(modifier_index).copied() else {
            return false;
        };
        let (Some(t1), Some(t2)) = (m.first_tangent, m.second_tangent) else {
            return self.locked_radius_toast(modifier_index);
        };
        let (Some(p1), Some(p2)) = (self.doc.point(t1), self.doc.point(t2)) else {
            return self.locked_radius_toast(modifier_index);
        };
        self.fillet_corner_drag = Some(FilletCornerDrag {
            modifier_index,
            down,
            t1_start: p1,
            t2_start: p2,
        });
        if let Some(arc) = m.arc {
            self.selection = vec![ElementRef::Segment(arc)];
        }
        true
    }

    /// Explains a locked radius when even the corner-drag fallback can't
    /// run (unlinkable legacy modifier). Returns false for convenience.
    fn locked_radius_toast(&mut self, modifier_index: usize) -> bool {
        let value = self.doc.modifiers.get(modifier_index).copied().and_then(|m| {
            m.arc.and_then(|arc| {
                self.doc
                    .dimensions
                    .iter()
                    .find(|d| matches!(d.target, DimTarget::Radius { seg } if seg == arc))
                    .map(|d| d.value)
            })
        });
        if let Some(value) = value {
            self.toast_requests.push(ToastRequest::error(
                "Radius is locked",
                format!("R{:.2} is held by its dimension.", value),
                "Delete the dimension to drag freely.",
                vec![BlockChip {
                    kind: LockKind::Dimension,
                    label: format!("Dimension = {:.2}", value),
                }],
            ));
        }
        false
    }

    /// Drags the virtual corner: both tangent points chase the cursor in
    /// one trial solve (best-effort commit per frame, like all drags —
    /// infeasible cursors land nearest-feasible). Flat-touching dims are
    /// revalued after each commit so they track the resize instead of
    /// fighting it.
    pub fn update_fillet_corner_drag(&mut self, cur: Point2) -> bool {
        let Some(dragst) = self.fillet_corner_drag else {
            return false;
        };
        let index = dragst.modifier_index;
        let Some(m) = self.doc.modifiers.get(index).copied() else {
            return false;
        };
        let (Some(t1), Some(t2)) = (m.first_tangent, m.second_tangent) else {
            return false;
        };
        let trial = self.doc.clone();
        let seeds = component_points(&trial, vec![t1, t2]);
        let aux: Vec<(PointId, Point2)> = seeds
            .iter()
            .filter_map(|&pid| trial.point(pid).map(|p| (pid, p)))
            .collect();
        // Rigid translation of the tangent pair by the cursor delta.
        // Chasing one shared cursor point would demand a zero-length arc
        // (infeasible under a fixed radius); translating keeps every
        // frame near-feasible instead.
        let (dx, dy) = (cur.x - dragst.down.x, cur.y - dragst.down.y);
        let drag = vec![
            (
                t1,
                Point2::new(dragst.t1_start.x + dx, dragst.t1_start.y + dy),
            ),
            (
                t2,
                Point2::new(dragst.t2_start.x + dx, dragst.t2_start.y + dy),
            ),
        ];
        let pins: Vec<(PointId, Point2)> = Vec::new();
        let mut solver =
            crate::core::solver::Solver::build_pinned(&trial, &drag, &aux, &pins, 2.0);
        // Same contract as canvas drags: fillet internals are
        // refresh-owned per frame (H/V + radius hold the shape).
        solver.strip_fillet_equations(&trial);
        let solution = solver.solve();
        // Strict commit (same thresholds as every other drag): infeasible
        // cursors resist instead of slanting locked constraints.
        if !solution.constraints_satisfied() {
            return false;
        }
        // Branch guard per frame: never commit a flipped arc mid-drag —
        // hold last-valid instead (a later cursor position resolves it).
        {
            let at = |id: PointId| {
                solution
                    .positions
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, p)| *p)
            };
            if let Some(tm) = trial.modifiers.get(index).copied()
                && let Some(g) = tm.evaluate(&trial)
                && let (Some(t1p), Some(t2p)) = (at(t1), at(t2))
                && let Some(cc) = tm.control.and_then(|id| at(id))
                && control_flipped(
                    t1p,
                    t2p,
                    cc,
                    g.first_tangent,
                    g.second_tangent,
                    g.control,
                )
            {
                return false;
            }
        }
        for (id, pos) in solution.positions {
            self.doc.move_point(id, pos);
        }
        self.refresh_fillets();
        // No revalue: strict commits keep flat dims satisfied as-is, so
        // values stay exactly as the user set them.
        self.doc_gen += 1;
        true
    }

    /// Drags the grabbed fillet center: cursor displacement along the
    /// outward axis resizes relative to the grab radius, through the same
    /// joint solver as typed edits (flat dims fight honestly per frame —
    /// conflicting frames simply don't commit). No history here; the
    /// press/release pair owns the undo step.
    pub fn update_fillet_radius_drag(&mut self, cur: Point2) -> bool {
        let Some(drag) = self.fillet_radius_drag else { return false };
        let Some(m) = self.doc.modifiers.get(drag.modifier_index).copied() else {
            return false;
        };
        let max_r = m.max_radius(&self.doc).unwrap_or(f64::INFINITY);
        let delta = (cur.x - drag.down.x) * drag.dir.0 + (cur.y - drag.down.y) * drag.dir.1;
        let clamped = (drag.start_radius + delta).max(0.01).min(max_r);
        if (clamped - m.radius).abs() < 1e-9 {
            return false;
        }
        if !self.solve_fillet_radius(drag.modifier_index, clamped) {
            return false;
        }
        // An open value input shows the stored dimension live — sync its
        // baseline too, or Enter would commit the stale pre-drag number.
        // Typed text (buffer) is left alone: typing keeps display priority.
        // Guarded to a real dim so a fresh dim-less input never inherits
        // a drag it has nothing to do with.
        let dim_index = m.arc.and_then(|arc| {
            self.doc.dimensions.iter().position(|d| {
                matches!(d.target, DimTarget::Radius { seg } if seg == arc)
            })
        });
        if let (Some(idx), Some(input)) = (dim_index, self.dim_input.as_mut()) {
            if input.existing == Some(idx) {
                input.measured = clamped;
            }
        }
        true
    }

    /// Opens the value input for a modifier's radius dimension (Inspector
    /// R-row, loaded fillets included — the persisted arc id relinks).
    pub fn open_fillet_radius_input(&mut self, index: usize) -> bool {
        let Some(m) = self.doc.modifiers.get(index).copied() else { return false };
        let Some(arc) = m.arc else { return false };
        let Some(dim_index) = self.doc.dimensions.iter().position(|d| {
            matches!(d.target, DimTarget::Radius { seg } if seg == arc)
        }) else {
            return false;
        };
        self.selection = vec![ElementRef::Segment(arc)];
        self.begin_dim_edit(dim_index);
        true
    }

    /// Flips a modifier between the Inner and Outer wedge. Mirror of the
    /// evaluator's side support; tangent points ride the line extensions
    /// on Outer (the solver constrains infinite lines, so nothing else
    /// changes).
    pub fn cycle_fillet_side(&mut self, index: usize) -> bool {
        let Some(old) = self.doc.modifiers.get(index).copied() else { return false };
        self.history_begin();
        self.doc.modifiers[index].side = old.side.flip();
        if !self.rebuild_modifier(index) {
            self.doc.modifiers[index].side = old.side;
            self.gesture_snapshot = None;
            return false;
        }
        self.doc_gen += 1;
        true
    }

    /// Fillet-dim leader frame: center plus the tangent-point angles that
    /// bound the arc span. Powers the dedicated dim layout and its drag.
    pub(crate) fn fillet_dim_frame(&self, seg: SegmentId) -> Option<(Point2, f64, f64, f64)> {
        let m = self.doc.modifiers.iter().find(|m| m.arc == Some(seg))?;
        let (Some(t1), Some(t2)) = (
            m.first_tangent.and_then(|id| self.doc.point(id)),
            m.second_tangent.and_then(|id| self.doc.point(id)),
        ) else {
            return None;
        };
        let o = m.center.and_then(|id| self.doc.point(id))?;
        let r = ((t1.x - o.x).powi(2) + (t1.y - o.y).powi(2)).sqrt();
        if r < 1e-9 {
            return None;
        }
        Some((o, (t1.y - o.y).atan2(t1.x - o.x), (t2.y - o.y).atan2(t2.x - o.x), r))
    }

    /// Placement drag for a fillet radius dim: slide rides the arc span
    /// (clamped to the edge), offset is the free leader length.
    pub(crate) fn update_fillet_dim_placement(&mut self, seg: SegmentId, cur: Point2) -> bool {
        let Some((o, a1, a2, r)) = self.fillet_dim_frame(seg) else { return false };
        let ctrl = self
            .doc
            .modifiers
            .iter()
            .find(|m| m.arc == Some(seg))
            .and_then(|m| m.control)
            .and_then(|id| self.doc.point(id));
        let Some(ctrl) = ctrl else { return false };
        let a_ctrl = (ctrl.y - o.y).atan2(ctrl.x - o.x);
        let ang = (cur.y - o.y).atan2(cur.x - o.x);
        let slide = span_fraction(a1, a2, a_ctrl, ang);
        let anchor_ang = span_angle(a1, a2, a_ctrl, slide);
        let (dx, dy) = (anchor_ang.cos(), anchor_ang.sin());
        // Signed leader: positive rides outside the arc, negative dives
        // inside toward the center (clamped there) — one continuous
        // motion morphs the dim from outer to inner fillet layout.
        let offset = ((cur.x - o.x) * dx + (cur.y - o.y) * dy - r).max(-r);
        if let Some(dim) = self.doc.dimensions.iter_mut().find(|d| {
            matches!(d.target, DimTarget::Radius { seg: s } if s == seg)
        }) {
            dim.slide = slide;
            dim.offset = offset;
            true
        } else {
            false
        }
    }

    pub fn fillet_click(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        // Center grabs are handled before the tool match in canvas_down
        // (shared with Move); picks start here.
        let at=self.cursor_doc(cursor);
        let picker=Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        if let Some(point)=picker.point(at) {
            // Clicking a committed tangent point reopens its radius
            // input — never a duplicate fillet attempt at the joint.
            if let Some(index) = self.doc.modifiers.iter().position(|m| {
                Some(point) == m.first_tangent || Some(point) == m.second_tangent
            }) {
                self.fillet_picks.clear();
                self.open_fillet_radius_input(index);
                return true;
            }
            let touching: Vec<_>=self.doc.all_segments().filter(|(_,s)| [s.start,s.end].contains(&point)).map(|(id,_)|id).collect();
            if touching.len() >= 2 { self.fillet_picks=vec![touching[0],touching[1]]; return self.create_selected_fillet(point); }
        }
        let Some(sid) = picker.segment(at) else {
            if self.fillet_picks.len()==2 {
                let (a, b) = (self.fillet_picks[0], self.fillet_picks[1]);
                if let Some(corner) = shared_corner(&self.doc, a, b, Some(at)) {
                    return self.create_selected_fillet(corner);
                }
            }
            return false;
        };
        if let Some(i) = self.fillet_picks.iter().position(|&x| x == sid) { self.fillet_picks.remove(i); return true; }
        self.fillet_picks.push(sid);
        if self.fillet_picks.len() > 2 { self.fillet_picks.remove(0); }
        if self.fillet_picks.len() != 2 { return true; }
        let (a, b) = (self.fillet_picks[0], self.fillet_picks[1]);
        let Some(corner) = shared_corner(&self.doc, a, b, Some(at)) else { return true; };
        self.create_selected_fillet(corner)
    }

    fn create_selected_fillet(&mut self, corner: PointId) -> bool {
        let (Some(a), Some(b)) = (self.doc.segment(self.fillet_picks[0]), self.doc.segment(self.fillet_picks[1])) else { return true; };
        if ![a.start, a.end].contains(&corner) || ![b.start, b.end].contains(&corner) {
            return true;
        }
        // Already filleted here (either pick order): don't stack a
        // duplicate — reveal the existing one and reopen its radius
        // input instead. This is also what makes pressing the center dot
        // of a filleted corner safe: it can never double-create.
        let (p0, p1) = (self.fillet_picks[0], self.fillet_picks[1]);
        if let Some(index) = self.doc.modifiers.iter().position(|m| {
            m.corner == corner
                && ((m.first == p0 && m.second == p1) || (m.first == p1 && m.second == p0))
        }) {
            self.fillet_picks.clear();
            self.open_fillet_radius_input(index);
            return true;
        }
        // Fit check first: degenerate pairs fail loudly, oversize defaults
        // clamp down instead of emitting geometry past the far ends.
        let probe = Fillet { first: self.fillet_picks[0], second: self.fillet_picks[1], corner, radius: 1.0, side: FilletSide::Inner, arc: None, first_tangent: None, second_tangent: None, center: None, control: None };
        let Some(max_r) = probe.max_radius(&self.doc) else {
            self.toast_requests.push(ToastRequest::error(
                "Can't fillet these edges",
                "These segments don't meet at a clean corner.",
                "Pick two segments sharing one corner.",
                Vec::new(),
            ));
            self.fillet_picks.clear();
            return true;
        };
        if max_r < MIN_USABLE_RADIUS {
            self.toast_requests.push(ToastRequest::error(
                "Can't fillet these edges",
                "These edges are too short for any fillet.",
                "Lengthen an edge, then retry.",
                Vec::new(),
            ));
            self.fillet_picks.clear();
            return true;
        }
        let mut modifier = Fillet { first: self.fillet_picks[0], second: self.fillet_picks[1], corner, radius: DEFAULT_RADIUS.min(max_r), side: FilletSide::Inner, arc: None, first_tangent: None, second_tangent: None, center: None, control: None };
        let Some(g) = modifier.evaluate(&self.doc) else { return true; };
        self.history_begin();
        let p1=self.doc.add_point(g.first_tangent); let p2=self.doc.add_point(g.second_tangent); let po=self.doc.add_point(g.center); let pc=self.doc.add_point(g.control);
        let arc=self.doc.add_arc_segment(p1,pc,p2,po);
        modifier.arc=Some(arc); modifier.first_tangent=Some(p1); modifier.second_tangent=Some(p2); modifier.center=Some(po); modifier.control=Some(pc);
        // Topological subdivision (line-line only): the sources are
        // rewired to the tangent points and the corner is consumed, so
        // edges end at the fillet and dims measure flats. Curves skip
        // this and keep the legacy referenced corner.
        let dropped = self.subdivide_corner(
            modifier.first,
            modifier.second,
            corner,
            p1,
            p2,
            arc,
        );
        self.doc.add_modifier(modifier);
        self.doc.add_point_on_segment_constraint(p1, p1, modifier.first);
        self.doc.add_point_on_segment_constraint(p2, p2, modifier.second);
        self.doc.add_tangent_constraint(modifier.first, arc, p1);
        self.doc.add_tangent_constraint(modifier.second, arc, p2);
        if let Some(dropped) = dropped {
            if dropped > 0 {
                self.toast_requests.push(ToastRequest::info(
                    "Corner constraints cleaned up",
                    format!(
                        "{} constraint{} referenced the filleted corner and couldn't follow the split.",
                        dropped,
                        if dropped == 1 { "" } else { "s" },
                    ),
                    "Re-constrain the new tangent points if needed.",
                    Vec::new(),
                ));
            }
        }
        if let Some(layer)=self.doc.layers.first().map(|l|l.id) { self.doc.push_to_layer(layer, ElementRef::Segment(arc)); }
        let dim_index=self.doc.dimensions.len();
        self.doc.dimensions.push(crate::core::constraints::Dimension { target: DimTarget::Radius { seg: arc }, value: modifier.radius, offset: 18., slide: 0.5, sweep: 0. });
        self.selection = vec![ElementRef::Segment(arc)];
        self.fillet_picks.clear();
        self.dim_input=Some(crate::editor::DimInput { target: DimTarget::Radius { seg: arc }, offset:18., slide:0.5, measured:modifier.radius, buffer:String::new(), existing:Some(dim_index) });
        self.doc_gen += 1;
        true
    }

    pub fn update_fillet_preview(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        let at = self.cursor_doc(cursor);
        let Some(sid) = Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX).segment(at) else { return self.fillet_preview.take().is_some(); };
        let mut candidates = self.fillet_picks.clone();
        if !candidates.contains(&sid) { candidates.push(sid); }
        if candidates.len() != 2 { return self.fillet_preview.take().is_some(); }
        let Some(corner) = shared_corner(&self.doc, candidates[0], candidates[1], Some(at)) else { return self.fillet_preview.take().is_some(); };
        // Preview the creation radius (clamped like creation) so the
        // preview never promises a fillet that won't fit.
        let probe = Fillet { first: candidates[0], second: candidates[1], corner, radius: 1.0, side: FilletSide::Inner, arc: None, first_tangent: None, second_tangent: None, center: None, control: None };
        let radius = probe.max_radius(&self.doc).map(|m| DEFAULT_RADIUS.min(m)).unwrap_or(DEFAULT_RADIUS);
        let geometry = Fillet { first:candidates[0], second:candidates[1], corner, radius, side: FilletSide::Inner, arc:None, first_tangent:None, second_tangent:None, center:None, control:None }.evaluate(&self.doc);
        let next = geometry.map(|geometry| FilletPreview { first:candidates[0], second:candidates[1], geometry });
        let changed = self.fillet_preview != next; self.fillet_preview = next; changed
    }

    /// Heals subdivided sources back to the recovered corner: both tangent
    /// points return to the live line intersection, t2 merges into t1
    /// (which becomes the corner again), dims revalue to full spans, and
    /// the arc leaves fill loops. Returns false when the lines went
    /// parallel (caller keeps the trimmed endpoints instead of dangling
    /// them).
    fn heal_subdivided_fillet(&mut self, m: &Fillet) -> bool {
        let Some(corner_pos) =
            crate::core::fillet::line_corner(&self.doc, m.first, m.second)
        else {
            return false;
        };
        let (Some(t1), Some(t2)) = (m.first_tangent, m.second_tangent) else {
            return false;
        };
        if self.doc.point(t1).is_none() || self.doc.point(t2).is_none() {
            return false;
        }
        self.doc.move_point(t1, corner_pos);
        self.doc.move_point(t2, corner_pos);
        // t2 merges into t1: S2 reattaches, refs rewrite, drop dies.
        // (A stacked second fillet cornered exactly at t2 keeps a now-dead
        // corner id; line-line evaluation recovers it via intersection.)
        self.doc.merge_point(t1, t2);
        revalue_dims_for_point(&mut self.doc, t1);
        if let Some(arc) = m.arc {
            let loops: Vec<_> = self
                .doc
                .all_fills()
                .filter(|(_, f)| f.segments.contains(&arc))
                .map(|(fid, _)| fid)
                .collect();
            for fid in loops {
                if let Some(f) = self.doc.fill_mut(fid) {
                    f.segments.retain(|&s| s != arc);
                }
            }
        }
        true
    }

    /// Drops exactly the creation-added derived constraints for one
    /// modifier (point-on-segment twins + arc tangents, matched on ids —
    /// user rows with coincidentally related geometry never match).
    fn drop_derived_constraints(&mut self, m: &Fillet) {
        let Some(arc) = m.arc else { return };
        self.doc.constraints.retain(|c| {
            let derived_point_glue = c.kind == ConstraintKind::Coincident
                && c.tangent_segments.is_none()
                && ((Some(c.a) == m.first_tangent
                    && Some(c.b) == m.first_tangent
                    && c.point_on_segment == Some(m.first))
                    || (Some(c.a) == m.second_tangent
                        && Some(c.b) == m.second_tangent
                        && c.point_on_segment == Some(m.second)));
            let derived_tangent = c.kind == ConstraintKind::Tangent
                && (Some(c.a) == m.first_tangent || Some(c.a) == m.second_tangent)
                && c.tangent_segments.is_some_and(|(x, y)| {
                    (x == m.first && y == arc)
                        || (x == arc && y == m.first)
                        || (x == m.second && y == arc)
                        || (x == arc && y == m.second)
                });
            !(derived_point_glue || derived_tangent)
        });
    }

    pub fn remove_modifier(&mut self, index: usize) -> bool {
        if index >= self.doc.modifiers.len() { return false; }
        self.history_begin();
        if let Some(m)=self.doc.modifiers.get(index).copied() {
            // Subdivided topology (dead corner, line-line sources): heal
            // the edges back to the recovered corner instead of leaving
            // them trimmed mid-air. t1 survives as the healed corner.
            let subdivided = self.doc.point(m.corner).is_none()
                && self.doc.segment(m.first).is_some_and(|s| s.kind == SegmentKind::Line)
                && self.doc.segment(m.second).is_some_and(|s| s.kind == SegmentKind::Line);
            // Heal when possible; either way the trimmed tangent points
            // stay live edge endpoints afterwards (healed into one corner
            // or left trimmed but valid) — only center/control go.
            if subdivided {
                self.heal_subdivided_fillet(&m);
            }
            if let Some(arc)=m.arc { self.doc.dimensions.retain(|d| !matches!(d.target, DimTarget::Radius { seg } if seg==arc)); self.doc.remove_segment(arc); }
            self.drop_derived_constraints(&m);
            // Construction points go with surgical (non-cascading)
            // removal. Subdivided tangent points stay live edge endpoints
            // in all cases (healed into one corner, or left trimmed but
            // valid when the lines went parallel) — only center/control
            // go. Legacy tangent points were never edge endpoints.
            let drop_tangents = !subdivided;
            for p in [
                m.first_tangent.filter(|_| drop_tangents),
                m.second_tangent.filter(|_| drop_tangents),
                m.center,
                m.control,
            ]
            .into_iter()
            .flatten()
            {
                let _ = self.doc.remove_point_raw(p);
            }
        }
        self.doc.remove_modifier(index); self.doc_gen += 1; true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::constraints::{ConstraintKind, DimMode, DimTarget, Dimension};
    use crate::core::geometry::Point2;
    use crate::editor::Editor;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn point_by_pos(ed: &Editor, x: f64, y: f64) -> crate::core::ids::PointId {
        ed.doc
            .all_points()
            .find(|(_, p)| approx(p.x, x) && approx(p.y, y))
            .map(|(id, _)| id)
            .expect("point not found")
    }

    fn segment_by_ends(
        ed: &Editor,
        ax: f64,
        ay: f64,
        bx: f64,
        by: f64,
    ) -> crate::core::ids::SegmentId {
        ed.doc
            .all_segments()
            .find(|(_, s)| {
                let (Some(a), Some(b)) = (ed.doc.point(s.start), ed.doc.point(s.end)) else {
                    return false;
                };
                (approx(a.x, ax) && approx(a.y, ay) && approx(b.x, bx) && approx(b.y, by))
                    || (approx(a.x, bx) && approx(a.y, by) && approx(b.x, ax) && approx(b.y, ay))
            })
            .map(|(id, _)| id)
            .expect("segment not found")
    }

    /// Full topological round-trip on a rectangle corner: subdivide
    /// (rewire + migrate + revalue + splice + consume), then heal on
    /// delete. Headless-safe: no rendering context needed.
    #[test]
    fn subdivide_and_heal_rectangle_corner() {
        let mut ed = Editor::new();
        ed.create_rectangle(1, Point2::new(0., 0.), Point2::new(100., 100.));
        let tl = point_by_pos(&ed, 0., 0.);
        let tr = point_by_pos(&ed, 100., 0.);
        let br = point_by_pos(&ed, 100., 100.);
        let top = segment_by_ends(&ed, 0., 0., 100., 0.);
        let right = segment_by_ends(&ed, 100., 0., 100., 100.);
        // A driving width dim on the top edge (full span for now).
        ed.doc.dimensions.push(Dimension {
            target: DimTarget::Points {
                a: tl,
                b: tr,
                mode: DimMode::Aligned,
            },
            value: 100.,
            offset: 18.,
            slide: 0.,
            sweep: 0.,
        });
        ed.fillet_picks = vec![top, right];
        assert!(ed.create_selected_fillet(tr));
        assert_eq!(ed.doc.modifiers.len(), 1);
        let m = ed.doc.modifiers[0];
        assert!((m.radius - 24.).abs() < 1e-9);
        let (t1, t2) = (m.first_tangent.unwrap(), m.second_tangent.unwrap());
        // Tangent points land one radius along each edge.
        let (p1, p2) = (ed.doc.point(t1).unwrap(), ed.doc.point(t2).unwrap());
        assert!(approx(p1.x, 76.) && approx(p1.y, 0.));
        assert!(approx(p2.x, 100.) && approx(p2.y, 24.));
        // Sources rewired; corner consumed.
        assert_eq!(ed.doc.segment(top).unwrap().end, t1);
        assert_eq!(ed.doc.segment(right).unwrap().start, t2);
        assert!(ed.doc.point(tr).is_none());
        // H/V migrated edge-affine; width dim revalued to the flat.
        assert!(ed.doc.constraints.iter().any(|c| {
            c.kind == ConstraintKind::Horizontal
                && ((c.a == tl && c.b == t1) || (c.a == t1 && c.b == tl))
        }));
        assert!(ed.doc.constraints.iter().any(|c| {
            c.kind == ConstraintKind::Vertical
                && ((c.a == t2 && c.b == br) || (c.a == br && c.b == t2))
        }));
        let width = ed
            .doc
            .dimensions
            .iter()
            .find(|d| {
                matches!(
                    d.target,
                    crate::core::constraints::DimTarget::Points { .. }
                )
            })
            .unwrap();
        assert!(approx(width.value, 76.));
        // Arc spliced into the fill loop.
        let arc = m.arc.unwrap();
        assert!(ed
            .doc
            .all_fills()
            .any(|(_, f)| f.segments.contains(&arc)));
        // Re-picking the subdivided pair finds no shared corner.
        ed.fillet_picks = vec![top, right];
        assert!(shared_corner(&ed.doc, top, right, None).is_none());
        // Delete heals: edges rejoin at one live corner, width back full.
        assert!(ed.remove_modifier(0));
        assert!(ed.doc.modifiers.is_empty());
        assert!(ed.doc.segment(arc).is_none());
        let top_s = ed.doc.segment(top).unwrap();
        let right_s = ed.doc.segment(right).unwrap();
        assert_eq!(top_s.end, right_s.start);
        let healed = ed.doc.point(top_s.end).unwrap();
        assert!(approx(healed.x, 100.) && approx(healed.y, 0.));
        let width = ed
            .doc
            .dimensions
            .iter()
            .find(|d| {
                matches!(
                    d.target,
                    crate::core::constraints::DimTarget::Points { .. }
                )
            })
            .unwrap();
        assert!(approx(width.value, 100.));
        // Radius dim went with the feature.
        assert!(ed.doc.dimensions.iter().all(|d| !matches!(
            d.target,
            crate::core::constraints::DimTarget::Radius { seg } if seg == arc
        )));
    }
}
