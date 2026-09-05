use super::constraints::{ConstraintKind, DimTarget};
use super::document::Document;
use super::geometry::Point2;
use super::ids::PointId;
use std::collections::HashMap;

// Real constraint solver: damped least-squares (Levenberg-Marquardt) over
// FREE point positions.
//
// Dragged points are eliminated from the system entirely — their cursor
// targets ARE their positions, exactly, by construction. The remaining
// free points are solved with well-conditioned weights, so every live
// drag frame is one small dense solve.
//
// Weighting scheme over free points:
//   - constraint equations (H/V/coincident/locked dims) dominate,
//   - a small soft anchor pulls each free point toward its pre-solve
//     position, which kills nullspace drift on under-constrained systems
//     (a rectangle has 8 DOF but only 4 constraint equations) by always
//     picking the minimal-motion solution.

/// Outcome of a solve pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolveStatus {
    Converged,
    MaxIterations,
}

pub struct Solution {
    pub status: SolveStatus,
    pub positions: Vec<(PointId, Point2)>,
    /// Largest unsatisfied LINEAR residual (doc units) at the solution.
    pub max_lin_residual: f64,
    /// Largest unsatisfied ANGULAR residual (radians) at the solution.
    pub max_angle_residual: f64,
}

const ANCHOR_WEIGHT: f64 = 1.0;
const DIM_WEIGHT: f64 = 1e6;
const EQ_WEIGHT: f64 = 1e6;
const DRAG_WEIGHT: f64 = 1e3;
// Implicit arc followers, per role (see build): left-behind endpoints stay
// (stretch pivots), the center is middle ground, and the bend follows an
// endpoint drag loosely so the shape survives instead of collapsing flat
// and flipping sides.
const ARC_ANCHOR_FACTOR: f64 = 8.0;
const ENDPOINT_ANCHOR_FACTOR: f64 = 10.0;
const CENTER_ANCHOR_FACTOR: f64 = 7.0;
const CTRL_FOLLOW_FACTOR: f64 = 2.5;
// Line endpoints freed so a tangent/parallel line can rotate instead of
// forcing the arc center to do all the work. Weak: rotation stays easy.
const TANGENT_ANCHOR_FACTOR: f64 = 4.0;
const MAX_ITER: usize = 60;
const TOL: f64 = 1e-10;
const LAMBDA_INIT: f64 = 1e-3;
// Per-iteration step cap (doc units). Halved from the old 100-unit cap
// that let one LM step teleport arc centers across the canvas.
const STEP_CAP: f64 = 50.0;

// One equation over point positions.
#[derive(Clone, Copy, Debug)]
enum Eq {
    // Residual: a.y - b.y
    Horizontal { a: usize, b: usize },
    // Residual: a.x - b.x
    Vertical { a: usize, b: usize },
    // Residual: |ab| - target
    Distance { a: usize, b: usize, target: f64 },
    // Residual: (b.x - a.x) - target (signed target captured at build)
    DistanceX { a: usize, b: usize, target: f64 },
    // Residual: (b.y - a.y) - target (signed target captured at build)
    DistanceY { a: usize, b: usize, target: f64 },
    // Residual: signed distance of p from line (l1->l2) - target
    PointLineDist { p: usize, l1: usize, l2: usize, target: f64 },
    // Residual: signed distance of b1 from line (a1->a2) - target
    LineDist { a1: usize, a2: usize, b1: usize, target: f64 },
    // Residual: signed angle between (a2-a1) and (b2-b1) - target (radians)
    Angle { a1: usize, a2: usize, b1: usize, b2: usize, target: f64 },
    // Residual: circumradius of the triangle (s,e,c) - target (doc units).
    // Expresses an arc radius constraint through its three defining points.
    ArcRadius { s: usize, e: usize, c: usize, target: f64 },
    EqualRadius { o: usize, a: usize, b: usize },
    // Minimum-bend barrier for live drags: keeps |bend height| above a small
    // floor so micro-drags can't push the arc through flat (the instant it
    // goes flat, the sweep representation flips sides and the whole arc
    // strobes). Memory-free — it resists crossing from whichever side the
    // iterate is on, so unlike a stored-side hinge it can never lock in a
    // flip that already happened. Deliberate large drags can still invert.
    ArcBend { s: usize, e: usize, c: usize },
    Tangent { l1: usize, l2: usize, o: usize, p: usize },
    CirclePoint { p: usize, o: usize, radius: f64 },
    Parallel { a1: usize, a2: usize, b1: usize, b2: usize },
}

// A residual evaluated at one iterate: value plus sparse gradient over
// FREE variable indices.
struct Residual {
    value: f64,
    // (free variable index, d value / d variable)
    grad: Vec<(usize, f64)>,
    weight: f64,
}

pub struct Solver {
    // All points involved in the system, indexed by slot. Slots that are
    // neither dragged nor auxiliary are FIXED at their document positions.
    slots: Vec<PointId>,
    // Point -> slot lookup (for post-build hard pins).
    index: HashMap<PointId, usize>,
    // Slot -> true when eliminated (a drag target).
    fixed: Vec<bool>,
    // Positions for fixed slots.
    fixed_pos: Vec<Point2>,
    start: Vec<Point2>,
    eqs: Vec<Eq>,
    // Subset of eqs that touches at least one free variable. Kept separate
    // so diagnostics can still report the full document constraint state.
    active_eqs: Vec<Eq>,
    // Dragged slots and cursor targets.
    drag: Vec<(usize, Point2)>,
    // Auxiliary slots: free followers with an anchor toward their position.
    // The f64 is a multiplier over `anchor_weight` (1.0 = normal follower,
    // ARC_ANCHOR_FACTOR = implicitly-freed arc points that must stay put).
    aux: Vec<(usize, Point2, f64)>,
    // Map slot index -> free variable index (None when fixed).
    free_of: Vec<Option<usize>>,
    n_free: usize,
    // Soft-anchor weight for free points (strong for dimension applies).
    anchor_weight: f64,
}

impl Solver {
    /// Builds the system from the document. `drag` maps dragged points to
    /// cursor targets (they chase the cursor with dominant weight). `aux`
    /// lists follower points with their gesture-start positions (soft,
    /// strong-anchor). Every other point referenced by a constraint is
    /// HARD-FIXED at its current document position, so constraints can
    /// reject illegal components of the drag instead of transmitting them.
    pub fn build(
        doc: &Document,
        drag: &[(PointId, Point2)],
        aux: &[(PointId, Point2)],
    ) -> Solver {
        let mut slots: Vec<PointId> = Vec::new();
        let mut index: HashMap<PointId, usize> = HashMap::new();

        let mut slot_of = |pid: PointId| -> Option<usize> {
            if doc.point(pid).is_none() {
                return None;
            }
            Some(match index.get(&pid) {
                Some(&i) => i,
                None => {
                    let i = slots.len();
                    slots.push(pid);
                    index.insert(pid, i);
                    i
                }
            })
        };

        let mut eqs = Vec::new();
        for c in &doc.constraints {
            let (Some(a), Some(b)) = (slot_of(c.a), slot_of(c.b)) else {
                continue;
            };
            match c.kind {
                ConstraintKind::Horizontal => eqs.push(Eq::Horizontal { a, b }),
                ConstraintKind::Vertical => eqs.push(Eq::Vertical { a, b }),
                ConstraintKind::Coincident => {
                    eqs.push(Eq::Horizontal { a, b });
                    eqs.push(Eq::Vertical { a, b });
                    if let Some(sid) = c.point_on_segment
                        && let Some(seg) = doc.segment(sid)
                        && let (Some(l1), Some(l2)) = (slot_of(seg.start), slot_of(seg.end))
                    {
                        eqs.push(Eq::PointLineDist { p: b, l1, l2, target: 0.0 });
                    }
                }
                ConstraintKind::Tangent => {
                    let inferred = || {
                        let mut line = None; let mut arc = None;
                        for (sid, s) in doc.all_segments() {
                            if s.start != c.a && s.end != c.a { continue; }
                            if s.kind == crate::core::document::SegmentKind::Line { line = Some(sid); }
                            if s.kind == crate::core::document::SegmentKind::Arc { arc = Some(sid); }
                        }
                        line.zip(arc)
                    };
                    let Some((line_id, arc_id)) = c.tangent_segments.or_else(inferred) else { continue };
                    let (Some(line), Some(arc)) = (doc.segment(line_id), doc.segment(arc_id)) else { continue };
                    let (Some(l1), Some(l2), Some(o), Some(p)) = (
                        slot_of(line.start), slot_of(line.end), arc.center.and_then(|id| slot_of(id)), slot_of(c.a)) else { continue };
                    eqs.push(Eq::Tangent { l1, l2, o, p });
                    if let (Some(center_id), Some(contact)) = (arc.center, doc.point(c.a))
                        && let Some(center) = doc.point(center_id)
                    {
                        let radius = ((contact.x - center.x).powi(2) + (contact.y - center.y).powi(2)).sqrt();
                        if radius > 1e-9 {
                            eqs.push(Eq::CirclePoint { p, o, radius });
                        }
                    }
                }
                ConstraintKind::Parallel => {
                    let Some((first, second)) = c.tangent_segments else { continue };
                    let (Some(a_seg), Some(b_seg)) = (doc.segment(first), doc.segment(second)) else { continue };
                    let (Some(a1), Some(a2), Some(b1), Some(b2)) = (
                        slot_of(a_seg.start), slot_of(a_seg.end), slot_of(b_seg.start), slot_of(b_seg.end)) else { continue };
                    eqs.push(Eq::Parallel { a1, a2, b1, b2 });
                }
            }
        }
        // Arc centers are part of the constraint graph. These equations keep
        // the stored center equidistant from all three arc-defining points.
        // The bend barrier is drag-only (live drags pass non-empty `drag`;
        // dimension/constraint applies pass empty): a shallow-but-valid
        // radius dimension must never fight it.
        let live_drag = !drag.is_empty();
        for (_, seg) in doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Arc { continue; }
            let (Some(o), Some(a), Some(b), Some(c)) = (
                seg.center.and_then(|id| slot_of(id)), slot_of(seg.start),
                slot_of(seg.end), seg.ctrl.and_then(|id| slot_of(id))) else { continue };
            eqs.push(Eq::EqualRadius { o, a, b });
            eqs.push(Eq::EqualRadius { o, a, b: c });
            if live_drag {
                eqs.push(Eq::ArcBend { s: a, e: b, c });
            }
        }

        for d in &doc.dimensions {
            match &d.target {
                DimTarget::Points { a, b, mode } => {
                    // Current geometry first: axis modes capture their sign
                    // from it, and slot_of shadows the ids below.
                    let endpoints = (doc.point(*a), doc.point(*b));
                    let (Some(a), Some(b)) = (slot_of(*a), slot_of(*b)) else {
                        continue;
                    };
                    match mode {
                        crate::core::constraints::DimMode::Aligned => {
                            eqs.push(Eq::Distance { a, b, target: d.value });
                        }
                        axis => {
                            // Sign captured from current geometry so the
                            // dimension keeps the placed orientation.
                            let (Some(ap), Some(bp)) = endpoints else {
                                continue;
                            };
                            let (cur, grad_axis) = match axis {
                                crate::core::constraints::DimMode::X => (bp.x - ap.x, 0usize),
                                _ => (bp.y - ap.y, 1usize),
                            };
                            let target = cur.signum() * d.value.abs();
                            eqs.push(if grad_axis == 0 {
                                Eq::DistanceX { a, b, target }
                            } else {
                                Eq::DistanceY { a, b, target }
                            });
                        }
                    }
                }
                DimTarget::PointLine { p, line } => {
                    let Some(seg) = doc.segment(*line) else { continue };
                    let (Some(pp), Some(l1), Some(l2)) =
                        (slot_of(*p), slot_of(seg.start), slot_of(seg.end))
                    else {
                        continue;
                    };
                    // Preserve whichever side of the line the point is on.
                    let (Some(ppp), Some(l1p), Some(l2p)) = (
                        doc.point(*p),
                        doc.point(seg.start),
                        doc.point(seg.end),
                    ) else {
                        continue;
                    };
                    let dx = l2p.x - l1p.x;
                    let dy = l2p.y - l1p.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let signed =
                        (ppp.x - l1p.x) * (-dy / l) + (ppp.y - l1p.y) * (dx / l);
                    eqs.push(Eq::PointLineDist {
                        p: pp,
                        l1,
                        l2,
                        target: signed.signum() * d.value,
                    });
                }
                DimTarget::Lines { a, b } => {
                    let Some(sega) = doc.segment(*a) else { continue };
                    let Some(segb) = doc.segment(*b) else { continue };
                    let (Some(a1), Some(a2)) = (slot_of(sega.start), slot_of(sega.end)) else {
                        continue;
                    };
                    let (Some(b1), _) = (slot_of(segb.start), slot_of(segb.end)) else {
                        continue;
                    };
                    let (Some(a1p), Some(a2p), Some(b1p)) = (
                        doc.point(sega.start),
                        doc.point(sega.end),
                        doc.point(segb.start),
                    ) else {
                        continue;
                    };
                    let dx = a2p.x - a1p.x;
                    let dy = a2p.y - a1p.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let signed = (b1p.x - a1p.x) * (-dy / l) + (b1p.y - a1p.y) * (dx / l);
                    eqs.push(Eq::LineDist { a1, a2, b1, target: signed.signum() * d.value });
                }
                DimTarget::Radius { seg } => {
                    // Radius constraint: circumradius of the arc's defining
                    // triangle equals the placed value.
                    let Some(seg_d) = doc.segment(*seg) else { continue };
                    let (Some(s), Some(e), Some(o)) = (slot_of(seg_d.start), slot_of(seg_d.end), seg_d.center.and_then(|id| slot_of(id))) else {
                        continue;
                    };
                    let Some(cc) = seg_d.ctrl.and_then(|id| slot_of(id)) else {
                        continue;
                    };
                    eqs.push(Eq::Distance { a: o, b: s, target: d.value.abs() });
                    eqs.push(Eq::Distance { a: o, b: e, target: d.value.abs() });
                    eqs.push(Eq::Distance { a: o, b: cc, target: d.value.abs() });
                }
                DimTarget::Angle { a, b } => {
                    let Some(sega) = doc.segment(*a) else { continue };
                    let Some(segb) = doc.segment(*b) else { continue };
                    let (Some(a1), Some(a2)) = (slot_of(sega.start), slot_of(sega.end)) else {
                        continue;
                    };
                    let (Some(b1), Some(b2)) = (slot_of(segb.start), slot_of(segb.end)) else {
                        continue;
                    };
                    let (Some(a1p), Some(a2p)) = (doc.point(sega.start), doc.point(sega.end)) else {
                        continue;
                    };
                    let (Some(b1p), Some(b2p)) = (doc.point(segb.start), doc.point(segb.end)) else {
                        continue;
                    };
                    // ORIENT the slot pairs along the vertex rays — the same
                    // convention dim_angle_geometry draws with. A segment's
                    // stored start->end may point AWAY from the corner (edge
                    // emitted bottom-to-top on a top-left corner), which
                    // would flip the enforced angle by 180 degrees and
                    // wrestle the geometry away from the drawn arc.
                    let (mut a1, mut a2, mut b1, mut b2) = (a1, a2, b1, b2);
                    if let Some(v) = line_intersection(a1p, a2p, b1p, b2p) {
                        let orient = |p0: Point2, p1: Point2, v: Point2| -> (usize, usize) {
                            // Ray from v toward the segment's midpoint.
                            let mid = Point2::new((p0.x + p1.x) / 2., (p0.y + p1.y) / 2.);
                            let ray = (mid.x - v.x, mid.y - v.y);
                            let dir = (p1.x - p0.x, p1.y - p0.y);
                            if ray.0 * dir.0 + ray.1 * dir.1 < 0. {
                                // slot indices flip with the points: p0's slot
                                // was passed in first, so swap order.
                                (1, 0)
                            } else {
                                (0, 1)
                            }
                        };
                        let (da, db) = (orient(a1p, a2p, v), orient(b1p, b2p, v));
                        if da == (1, 0) {
                            std::mem::swap(&mut a1, &mut a2);
                        }
                        if db == (1, 0) {
                            std::mem::swap(&mut b1, &mut b2);
                        }
                    }
                    // The placed SIGNED sweep (degrees) is the target — the
                    // rotation from ray A to ray B in the placed direction,
                    // exactly what the arc draws. Label, arc and constraint
                    // can never disagree.
                    eqs.push(Eq::Angle {
                        a1,
                        a2,
                        b1,
                        b2,
                        target: d.sweep.to_radians(),
                    });
                }
            }
        }

        // Constraint-less geometry (rulers, free lines) must still enter
        // the system when dragged, or their targets get dropped.
        for &(pid, _) in drag {
            let _ = slot_of(pid);
        }
        for &(pid, _) in aux {
            let _ = slot_of(pid);
        }

        let mut fixed = vec![true; slots.len()];
        let mut fixed_pos = vec![Point2::new(0., 0.); slots.len()];
        for (i, &pid) in slots.iter().enumerate() {
            fixed_pos[i] = doc.point(pid).unwrap();
        }

        let start: Vec<Point2> = fixed_pos.clone();

        let mut free_of = vec![None; slots.len()];
        let mut n_free = 0;

        let mut drag_idx: Vec<(usize, Point2)> = Vec::new();
        for &(pid, t) in drag {
            if let Some(&i) = index.get(&pid) {
                drag_idx.push((i, t));
                if free_of[i].is_none() {
                    free_of[i] = Some(n_free);
                    n_free += 1;
                    fixed[i] = false;
                }
            }
        }

        let mut aux_idx: Vec<(usize, Point2, f64)> = Vec::new();
        for &(pid, anchor) in aux {
            if let Some(&i) = index.get(&pid) {
                if free_of[i].is_none() {
                    free_of[i] = Some(n_free);
                    n_free += 1;
                    fixed[i] = false;
                }
                // Refresh to current position if already free (e.g. an arc
                // point that is both a ring follower and arc-implicit): the
                // anchor must track the live geometry, not gesture start.
                if let Some(entry) = aux_idx.iter_mut().find(|(s, _, _)| *s == i) {
                    entry.1 = fixed_pos[i];
                } else {
                    aux_idx.push((i, anchor, 1.0));
                }
            }
        }

        // An arc is one solver component during a drag. Free all defining
        // points and its center together when any one is dragged, anchored
        // at the CURRENT position. Roles get different strengths: a left-
        // behind endpoint should stay (stretch pivot), the bend follows an
        // endpoint drag loosely so the shape survives instead of collapsing
        // flat and flipping, and the center is in the middle.
        for (_, seg) in doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Arc { continue; }
            let mut ids = vec![seg.start, seg.end];
            if let Some(id) = seg.ctrl { ids.push(id); }
            if let Some(id) = seg.center { ids.push(id); }
            if !drag.iter().any(|(id, _)| ids.contains(id)) { continue; }
            let dragged_center = seg.center.is_some_and(|id| drag.iter().any(|(d, _)| *d == id));
            let dragged_ctrl = seg.ctrl.is_some_and(|id| drag.iter().any(|(d, _)| *d == id));
            let dragged_endpoint =
                drag.iter().any(|(d, _)| *d == seg.start || *d == seg.end);
            for pid in ids {
                // Per-role anchor strength (see comment above).
                let factor = if Some(pid) == seg.center {
                    CENTER_ANCHOR_FACTOR
                } else if Some(pid) == seg.ctrl {
                    if dragged_endpoint { CTRL_FOLLOW_FACTOR } else { ARC_ANCHOR_FACTOR }
                } else if dragged_ctrl || dragged_center {
                    ARC_ANCHOR_FACTOR
                } else {
                    // A left-behind endpoint while its partner is dragged.
                    ENDPOINT_ANCHOR_FACTOR
                };
                let Some(&i) = index.get(&pid) else { continue };
                if free_of[i].is_none() {
                    free_of[i] = Some(n_free); n_free += 1; fixed[i] = false;
                    aux_idx.push((i, fixed_pos[i], factor));
                } else if let Some(entry) = aux_idx.iter_mut().find(|(s, _, _)| *s == i) {
                    // Already a follower (e.g. dragged directly): keep its
                    // target but promote the anchor strength.
                    entry.2 = entry.2.max(factor);
                }
            }
        }

        // Tangent/parallel lines must be able to ROTATE when their contact
        // is dragged. Their constraints store a==b==contact, so the generic
        // ring-follower logic never frees the line's far endpoint — it stays
        // hard-fixed and the solver has to satisfy tangency by throwing the
        // arc center around (the bounce). Free those far endpoints here as
        // weak followers anchored at their current positions.
        for c in &doc.constraints {
            let (ids, touched) = match c.kind {
                crate::core::constraints::ConstraintKind::Tangent => {
                    let Some((line_id, arc_id)) = c.tangent_segments else { continue };
                    let (Some(line), Some(arc)) = (doc.segment(line_id), doc.segment(arc_id)) else { continue };
                    let mut v = vec![line.start, line.end, c.a, c.b];
                    v.push(arc.start);
                    v.push(arc.end);
                    if let Some(id) = arc.ctrl { v.push(id); }
                    if let Some(id) = arc.center { v.push(id); }
                    let touched = drag.iter().any(|(id, _)| v.contains(id));
                    (vec![line.start, line.end], touched)
                }
                crate::core::constraints::ConstraintKind::Parallel => {
                    let Some((first, second)) = c.tangent_segments else { continue };
                    let (Some(a_seg), Some(b_seg)) = (doc.segment(first), doc.segment(second)) else { continue };
                    let v = vec![a_seg.start, a_seg.end, b_seg.start, b_seg.end];
                    let touched = drag.iter().any(|(id, _)| v.contains(id));
                    (v, touched)
                }
                _ => continue,
            };
            if !touched { continue; }
            for pid in ids {
                let Some(&i) = index.get(&pid) else { continue };
                if drag.iter().any(|(id, _)| *id == pid) { continue; }
                if free_of[i].is_none() {
                    free_of[i] = Some(n_free); n_free += 1; fixed[i] = false;
                    aux_idx.push((i, fixed_pos[i], TANGENT_ANCHOR_FACTOR));
                }
            }
        }

        let active_eqs = eqs
            .iter()
            .copied()
            .filter(|&eq| {
                let slots = match eq {
                    Eq::Horizontal { a, b }
                    | Eq::Vertical { a, b }
                    | Eq::Distance { a, b, .. }
                    | Eq::DistanceX { a, b, .. }
                    | Eq::DistanceY { a, b, .. } => [a, b, usize::MAX, usize::MAX],
                    Eq::PointLineDist { p, l1, l2, .. } => [p, l1, l2, usize::MAX],
                    Eq::LineDist { a1, a2, b1, .. } => [a1, a2, b1, usize::MAX],
                    Eq::Angle { a1, a2, b1, b2, .. }
                    | Eq::Parallel { a1, a2, b1, b2 } => [a1, a2, b1, b2],
                    Eq::ArcRadius { s, e, c, .. }
                    | Eq::ArcBend { s, e, c } => [s, e, c, usize::MAX],
                    Eq::EqualRadius { o, a, b } => [o, a, b, usize::MAX],
                    Eq::Tangent { l1, l2, o, p } => [l1, l2, o, p],
                    Eq::CirclePoint { p, o, .. } => [p, o, usize::MAX, usize::MAX],
                };
                slots.iter().any(|&slot| {
                    slot != usize::MAX && free_of[slot].is_some()
                })
            })
            .collect();

        Solver {
            slots,
            index,
            fixed,
            fixed_pos,
            start,
            eqs,
            active_eqs,
            drag: drag_idx,
            aux: aux_idx,
            free_of,
            n_free,
            anchor_weight: ANCHOR_WEIGHT,
        }
    }

    /// Convenience: no auxiliary followers.
    pub fn build_simple(doc: &Document, drag: &[(PointId, Point2)]) -> Solver {
        Self::build(doc, drag, &[])
    }

    /// Variant with a STRONG soft-anchor: used by dimension application,
    /// where the freed component must deform minimally to satisfy the new
    /// equation instead of drifting wherever the solve is easiest.
    pub fn build_with_anchor(
        doc: &Document,
        drag: &[(PointId, Point2)],
        aux: &[(PointId, Point2)],
        anchor_weight: f64,
    ) -> Solver {
        let mut solver = Self::build(doc, drag, aux);
        solver.anchor_weight = anchor_weight;
        solver
    }

    /// Like `build_with_anchor`, with additional points HARD-FIXED at the
    /// given positions (pulled out of the free set entirely). Used by
    /// dimension application to anchor the reshape at the geometry's
    /// top-left: the pinned extremity stays exactly put while the rest of
    /// the component shrinks/grows toward it, instead of everything
    /// converging symmetrically.
    pub fn build_pinned(
        doc: &Document,
        drag: &[(PointId, Point2)],
        aux: &[(PointId, Point2)],
        pins: &[(PointId, Point2)],
        anchor_weight: f64,
    ) -> Solver {
        let mut solver = Self::build(doc, drag, aux);
        for (pid, pos) in pins {
            if let Some(&i) = solver.index.get(pid) {
                solver.fixed[i] = true;
                solver.fixed_pos[i] = *pos;
                solver.free_of[i] = None;
            }
        }
        // Compact the free-variable indices: `x` iterates free slots in
        // slot order, so variable v must equal the slot's rank among the
        // still-free slots.
        let mut next = 0;
        for f in solver.free_of.iter_mut() {
            if f.is_some() {
                *f = Some(next);
                next += 1;
            }
        }
        solver.n_free = next;
        solver.active_eqs = solver
            .eqs
            .iter()
            .copied()
            .filter(|&eq| solver.eq_touches_free(eq))
            .collect();
        solver.anchor_weight = anchor_weight;
        solver
    }

    pub fn is_empty(&self) -> bool {
        self.n_free == 0 && self.eqs.is_empty()
    }

    fn has_work(&self) -> bool {
        self.n_free > 0
    }

    /// Current position of a slot: cursor target when eliminated, else the
    /// free iterate.
    fn pos(&self, s: usize, x: &[Point2]) -> Point2 {
        match self.free_of[s] {
            Some(v) => x[v],
            None => self.fixed_pos[s],
        }

    }

    /// Returns whether an equation can influence the current free iterate.
    /// The document may contain many unrelated constraints; evaluating their
    /// residuals during a local drag only burns CPU because every variable in
    /// their gradient is fixed.
    fn eq_touches_free(&self, eq: Eq) -> bool {
        let slots: [usize; 4] = match eq {
            Eq::Horizontal { a, b }
            | Eq::Vertical { a, b }
            | Eq::Distance { a, b, .. }
            | Eq::DistanceX { a, b, .. }
            | Eq::DistanceY { a, b, .. } => [a, b, usize::MAX, usize::MAX],
            Eq::PointLineDist { p, l1, l2, .. } => [p, l1, l2, usize::MAX],
            Eq::LineDist { a1, a2, b1, .. } => [a1, a2, b1, usize::MAX],
            Eq::Angle { a1, a2, b1, b2, .. } => [a1, a2, b1, b2],
            Eq::ArcRadius { s, e, c, .. } => [s, e, c, usize::MAX],
            Eq::EqualRadius { o, a, b } => [o, a, b, usize::MAX],
            Eq::ArcBend { s, e, c } => [s, e, c, usize::MAX],
            Eq::Tangent { l1, l2, o, p } => [l1, l2, o, p],
            Eq::CirclePoint { p, o, .. } => [p, o, usize::MAX, usize::MAX],
            Eq::Parallel { a1, a2, b1, b2 } => [a1, a2, b1, b2],
        };
        slots
            .iter()
            .any(|&slot| slot != usize::MAX && self.free_of[slot].is_some())
    }

    /// Evaluates all weighted residuals (geometric + soft anchors).
    /// Variables are SCALARS: free slot v owns variables (2v, 2v+1).
    fn residuals_into(&self, x: &[Point2], out: &mut Vec<Residual>) {
        out.clear();
        out.reserve(self.active_eqs.len() * 2 + self.n_free * 2);
        for eq in &self.active_eqs {
            match *eq {
                Eq::Horizontal { a, b } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2 + 1, 1.0));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2 + 1, -1.0));
                    }
                    out.push(Residual { value: pa.y - pb.y, grad, weight: EQ_WEIGHT });
                }
                Eq::Vertical { a, b } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2, 1.0));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2, -1.0));
                    }
                    out.push(Residual { value: pa.x - pb.x, grad, weight: EQ_WEIGHT });
                }
                Eq::Distance { a, b, target } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let dx = pb.x - pa.x;
                    let dy = pb.y - pa.y;
                    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let ux = dx / len;
                    let uy = dy / len;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        // d|ab|/da = -(unit ab)
                        grad.push((v * 2, -ux));
                        grad.push((v * 2 + 1, -uy));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2, ux));
                        grad.push((v * 2 + 1, uy));
                    }
                    out.push(Residual { value: len - target, grad, weight: DIM_WEIGHT });
                }
                Eq::DistanceX { a, b, target } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2, -1.0));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2, 1.0));
                    }
                    out.push(Residual { value: pb.x - pa.x - target, grad, weight: DIM_WEIGHT });
                }
                Eq::DistanceY { a, b, target } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2 + 1, -1.0));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2 + 1, 1.0));
                    }
                    out.push(Residual { value: pb.y - pa.y - target, grad, weight: DIM_WEIGHT });
                }
                Eq::PointLineDist { p, l1, l2, target } => {
                    let (pp, pl1, pl2) = (self.pos(p, x), self.pos(l1, x), self.pos(l2, x));
                    let dx = pl2.x - pl1.x;
                    let dy = pl2.y - pl1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let nx = -dy / l;
                    let ny = dx / l;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[p] {
                        grad.push((v * 2, nx));
                        grad.push((v * 2 + 1, ny));
                    }
                    if let Some(v) = self.free_of[l1] {
                        grad.push((v * 2, -nx));
                        grad.push((v * 2 + 1, -ny));
                    }
                    // l2 only tilts the normal (second order) — no gradient.
                    out.push(Residual {
                        value: (pp.x - pl1.x) * nx + (pp.y - pl1.y) * ny - target,
                        grad,
                        weight: DIM_WEIGHT,
                    });
                }
                Eq::LineDist { a1, a2, b1, target } => {
                    let (pa1, pa2, pb1) =
                        (self.pos(a1, x), self.pos(a2, x), self.pos(b1, x));
                    let dx = pa2.x - pa1.x;
                    let dy = pa2.y - pa1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let nx = -dy / l;
                    let ny = dx / l;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a1] {
                        grad.push((v * 2, -nx));
                        grad.push((v * 2 + 1, -ny));
                    }
                    if let Some(v) = self.free_of[b1] {
                        grad.push((v * 2, nx));
                        grad.push((v * 2 + 1, ny));
                    }
                    out.push(Residual {
                        value: (pb1.x - pa1.x) * nx + (pb1.y - pa1.y) * ny - target,
                        grad,
                        weight: DIM_WEIGHT,
                    });
                }
                Eq::Angle { a1, a2, b1, b2, target } => {
                    let (pa1, pa2, pb1, pb2) = (
                        self.pos(a1, x),
                        self.pos(a2, x),
                        self.pos(b1, x),
                        self.pos(b2, x),
                    );
                    let ax = pa2.x - pa1.x;
                    let ay = pa2.y - pa1.y;
                    let bx = pb2.x - pb1.x;
                    let by = pb2.y - pb1.y;
                    let c = ax * by - ay * bx;
                    let d = ax * bx + ay * by;
                    let denom = (c * c + d * d).max(1e-9);
                    // dTheta/dA and dTheta/dB of atan2(cross, dot).
                    let gax = (by * d - c * bx) / denom;
                    let gay = (-bx * d - c * by) / denom;
                    let gbx = (-ay * d - c * ax) / denom;
                    let gby = (ax * d - c * ay) / denom;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a1] {
                        grad.push((v * 2, -gax));
                        grad.push((v * 2 + 1, -gay));
                    }
                    if let Some(v) = self.free_of[a2] {
                        grad.push((v * 2, gax));
                        grad.push((v * 2 + 1, gay));
                    }
                    if let Some(v) = self.free_of[b1] {
                        grad.push((v * 2, -gbx));
                        grad.push((v * 2 + 1, -gby));
                    }
                    if let Some(v) = self.free_of[b2] {
                        grad.push((v * 2, gbx));
                        grad.push((v * 2 + 1, gby));
                    }
                    out.push(Residual {
                        // Wrapped into (-PI, PI]: correct for ordinary AND
                        // reflex (|sweep| > 180 deg) targets.
                        value: {
                            let mut value = c.atan2(d) - target;
                            while value > std::f64::consts::PI {
                                value -= std::f64::consts::TAU;
                            }
                            while value <= -std::f64::consts::PI {
                                value += std::f64::consts::TAU;
                            }
                            value
                        },
                        grad,
                        weight: DIM_WEIGHT,
                    });
                }
                Eq::EqualRadius { o, a, b } => {
                    // Normalized: |pa-po| - |pb-po| in doc units. The old
                    // squared-distance form scaled with radius^2, so a large
                    // arc dominated the system and micro-drags slingshot the
                    // center.
                    let (po, pa, pb) = (self.pos(o, x), self.pos(a, x), self.pos(b, x));
                    let dax = pa.x - po.x;
                    let day = pa.y - po.y;
                    let dbx = pb.x - po.x;
                    let dby = pb.y - po.y;
                    let da = (dax * dax + day * day).sqrt().max(1e-9);
                    let db = (dbx * dbx + dby * dby).sqrt().max(1e-9);
                    let (uax, uay) = (dax / da, day / da);
                    let (ubx, uby) = (dbx / db, dby / db);
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[o] {
                        grad.push((v * 2, ubx - uax));
                        grad.push((v * 2 + 1, uby - uay));
                    }
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2, uax));
                        grad.push((v * 2 + 1, uay));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2, -ubx));
                        grad.push((v * 2 + 1, -uby));
                    }
                    out.push(Residual { value: da - db, grad, weight: EQ_WEIGHT });
                }
                Eq::ArcBend { s, e, c } => {
                    // Minimum-bend barrier in doc units: keeps the bend
                    // height off the chord above a small floor. Zero when
                    // satisfied; pushing back toward the current side when
                    // violated (gradient follows sign(h), so no stored side
                    // is needed and a flip can never get locked in).
                    let (ps, pe, pc) = (self.pos(s, x), self.pos(e, x), self.pos(c, x));
                    let ux = pe.x - ps.x;
                    let uy = pe.y - ps.y;
                    let vx = pc.x - ps.x;
                    let vy = pc.y - ps.y;
                    let chord = (ux * ux + uy * uy).sqrt().max(1e-9);
                    let h = (ux * vy - uy * vx) / chord;
                    let min_h = (0.025 * chord).clamp(1.0, 8.0);
                    let h_abs = h.abs();
                    let mut grad = Vec::new();
                    if h_abs < min_h {
                        // d(h_abs) = sign(h) * dh; residual = min_h - h_abs.
                        let sgn = if h >= 0. { 1.0 } else { -1.0 };
                        if let Some(v) = self.free_of[s] {
                            grad.push((v * 2, -sgn * (-vy + uy) / chord));
                            grad.push((v * 2 + 1, -sgn * (-ux + vx) / chord));
                        }
                        if let Some(v) = self.free_of[e] {
                            grad.push((v * 2, -sgn * vy / chord));
                            grad.push((v * 2 + 1, -sgn * -vx / chord));
                        }
                        if let Some(v) = self.free_of[c] {
                            grad.push((v * 2, -sgn * -uy / chord));
                            grad.push((v * 2 + 1, -sgn * ux / chord));
                        }
                    }
                    out.push(Residual {
                        value: (min_h - h_abs).max(0.),
                        grad,
                        weight: EQ_WEIGHT,
                    });
                }
                Eq::Tangent { l1, l2, o, p } => {
                    // Transverse distance in doc units: dot/mean_len, 0 when
                    // perpendicular. Raw dot scaled with length*radius so
                    // long lines yanked arcs on micro-drags; pure cosine was
                    // too soft next to distance constraints. Mean-length
                    // scaling keeps units consistent with the rest.
                    let (a, b, center, contact) = (self.pos(l1, x), self.pos(l2, x), self.pos(o, x), self.pos(p, x));
                    let vx = b.x - a.x; let vy = b.y - a.y;
                    let rx = contact.x - center.x; let ry = contact.y - center.y;
                    let lv = (vx * vx + vy * vy).sqrt().max(1e-9);
                    let lr = (rx * rx + ry * ry).sqrt().max(1e-9);
                    let m = ((lv + lr) / 2.).max(1e-9);
                    let dot = vx * rx + vy * ry;
                    let value = dot / m;
                    let dvx = rx / m - dot * vx / (2. * lv * m * m);
                    let dvy = ry / m - dot * vy / (2. * lv * m * m);
                    let drx = vx / m - dot * rx / (2. * lr * m * m);
                    let dry = vy / m - dot * ry / (2. * lr * m * m);
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[l1] { grad.push((v * 2, -dvx)); grad.push((v * 2 + 1, -dvy)); }
                    if let Some(v) = self.free_of[l2] { grad.push((v * 2, dvx)); grad.push((v * 2 + 1, dvy)); }
                    if let Some(v) = self.free_of[o] { grad.push((v * 2, -drx)); grad.push((v * 2 + 1, -dry)); }
                    if let Some(v) = self.free_of[p] { grad.push((v * 2, drx)); grad.push((v * 2 + 1, dry)); }
                    out.push(Residual { value, grad, weight: EQ_WEIGHT });
                }
                Eq::CirclePoint { p, o, radius } => {
                    let (point, center) = (self.pos(p, x), self.pos(o, x));
                    let dx = point.x - center.x;
                    let dy = point.y - center.y;
                    let length = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[p] {
                        grad.push((v * 2, dx / length));
                        grad.push((v * 2 + 1, dy / length));
                    }
                    if let Some(v) = self.free_of[o] {
                        grad.push((v * 2, -dx / length));
                        grad.push((v * 2 + 1, -dy / length));
                    }
                    out.push(Residual { value: length - radius, grad, weight: EQ_WEIGHT });
                }
                Eq::Parallel { a1, a2, b1, b2 } => {
                    // Transverse distance in doc units: cross/mean_len.
                    let (a, b, c, d) = (self.pos(a1, x), self.pos(a2, x), self.pos(b1, x), self.pos(b2, x));
                    let ux = b.x - a.x; let uy = b.y - a.y;
                    let vx = d.x - c.x; let vy = d.y - c.y;
                    let lu = (ux * ux + uy * uy).sqrt().max(1e-9);
                    let lv = (vx * vx + vy * vy).sqrt().max(1e-9);
                    let m = ((lu + lv) / 2.).max(1e-9);
                    let cross = ux * vy - uy * vx;
                    let value = cross / m;
                    let dux = vy / m - cross * ux / (2. * lu * m * m);
                    let duy = -vx / m - cross * uy / (2. * lu * m * m);
                    let dvx = -uy / m - cross * vx / (2. * lv * m * m);
                    let dvy = ux / m - cross * vy / (2. * lv * m * m);
                    let mut grad = Vec::new();
                    if let Some(i) = self.free_of[a1] { grad.push((i * 2, -dux)); grad.push((i * 2 + 1, -duy)); }
                    if let Some(i) = self.free_of[a2] { grad.push((i * 2, dux)); grad.push((i * 2 + 1, duy)); }
                    if let Some(i) = self.free_of[b1] { grad.push((i * 2, -dvx)); grad.push((i * 2 + 1, -dvy)); }
                    if let Some(i) = self.free_of[b2] { grad.push((i * 2, dvx)); grad.push((i * 2 + 1, dvy)); }
                    out.push(Residual { value, grad, weight: EQ_WEIGHT });
                }
                Eq::ArcRadius { s, e, c, target } => {
                    let (ps, pe, pc) = (self.pos(s, x), self.pos(e, x), self.pos(c, x));
                    // Chord geometry: half-length m, bend height h (signed
                    // perpendicular of the bend off the chord). Circumradius
                    // R = (m^2 + h^2) / (2h).
                    let wx = pe.x - ps.x;
                    let wy = pe.y - ps.y;
                    let l = (wx * wx + wy * wy).sqrt().max(1e-9);
                    let ux = wx / l;
                    let uy = wy / l;
                    let m = l / 2.;
                    // SIGNED bend height off the chord (either side is a
                    // valid arc); R uses |h| — clamping the sign away made
                    // half of all arcs unsatisfiable.
                    let h = (pc.x - ps.x) * (-uy) + (pc.y - ps.y) * ux;
                    let h_abs = h.abs().max(1e-6);
                    let value = (m * m + h_abs * h_abs) / (2. * h_abs) - target;
                    // dR/dh carries the sign of h (R is symmetric in +/-h),
                    // dR/dm = m / |h| — the /h variant had the WRONG sign
                    // for negative-h arcs, pushing chord length the wrong
                    // way so radius dims never reached their target.
                    let dr_dh = h.signum() * (h_abs * h_abs - m * m) / (2. * h_abs * h_abs);
                    let dr_dm = m / h_abs;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[c] {
                        // dh/dc = perp(u)
                        grad.push((v * 2, -uy * dr_dh));
                        grad.push((v * 2 + 1, ux * dr_dh));
                    }
                    if let Some(v) = self.free_of[s] {
                        // dh/ds = -perp(u); dm/ds = -u/2
                        grad.push((v * 2, uy * dr_dh - ux / 2. * dr_dm));
                        grad.push((v * 2 + 1, -ux * dr_dh - uy / 2. * dr_dm));
                    }
                    if let Some(v) = self.free_of[e] {
                        // dh/de = 0 (first order); dm/de = +u/2
                        grad.push((v * 2, ux / 2. * dr_dm));
                        grad.push((v * 2 + 1, uy / 2. * dr_dm));
                    }
                    out.push(Residual { value, grad, weight: DIM_WEIGHT });
                }
            }
        }
        // Drag targets dominate; auxiliary followers anchor strongly to
        // their gesture-start positions.
        for &(i, t) in &self.drag {
            if let Some(v) = self.free_of[i] {
                out.push(Residual {
                    value: x[v].x - t.x,
                    grad: vec![(v * 2, 1.0)],
                    weight: DRAG_WEIGHT,
                });
                out.push(Residual {
                    value: x[v].y - t.y,
                    grad: vec![(v * 2 + 1, 1.0)],
                    weight: DRAG_WEIGHT,
                });
            }
        }
        for &(i, anchor, factor) in &self.aux {
            if let Some(v) = self.free_of[i] {
                let w = self.anchor_weight * factor;
                out.push(Residual {
                    value: x[v].x - anchor.x,
                    grad: vec![(v * 2, 1.0)],
                    weight: w,
                });
                out.push(Residual {
                    value: x[v].y - anchor.y,
                    grad: vec![(v * 2 + 1, 1.0)],
                    weight: w,
                });
            }
        }
    }

    fn cost(residuals: &[Residual]) -> f64 {
        residuals.iter().map(|r| r.weight * r.value * r.value).sum()
    }

    /// Runs LM from current geometry. Returns new positions for all slots
    /// plus a status and the largest unsatisfied equation residuals
    /// (linear in doc units, angular in radians) — large values mean the
    /// system is over-constrained / infeasible.
    pub fn solve(&self) -> Solution {
        if !self.has_work() {
            return Solution {
                status: SolveStatus::Converged,
                positions: Vec::new(),
                max_lin_residual: 0.,
                max_angle_residual: 0.,
            };
        }

        let n = self.n_free * 2;
        // Free iterate in slot order.
        let mut x: Vec<Point2> =
            (0..self.slots.len()).filter(|&s| self.free_of[s].is_some()).map(|s| self.start[s]).collect();

        let mut lambda = LAMBDA_INIT;
        let mut status = SolveStatus::MaxIterations;
        let residual_capacity = self.active_eqs.len() * 2 + self.n_free * 2;
        let mut residuals = Vec::with_capacity(residual_capacity);
        let mut trial_residuals = Vec::with_capacity(residual_capacity);
        self.residuals_into(&x, &mut residuals);
        let init_cost = Self::cost(&residuals);
        let mut cost = init_cost;
        // Reuse the normal-equation and trial buffers across LM iterations.
        // These are hot-path scratch values, not solver state: keeping them
        // here avoids allocating several large vectors for every iteration
        // of every live drag.
        let mut jtj = vec![0.0; n * n];
        let mut jtr = vec![0.0; n];
        let mut dx = vec![0.0; n];
        let mut trial = vec![Point2::new(0., 0.); self.n_free];

        for _iter in 0..MAX_ITER {
            if cost < TOL {
                status = SolveStatus::Converged;
                break;
            }

            jtj.fill(0.0);
            jtr.fill(0.0);
            self.residuals_into(&x, &mut residuals);
            for r in &residuals {
                let w = r.weight;
                for &(vi, gi) in &r.grad {
                    for &(vj, gj) in &r.grad {
                        jtj[vi * n + vj] += w * gi * gj;
                    }
                    jtr[vi] -= w * gi * r.value;
                }
            }
            for i in 0..n {
                jtj[i * n + i] += lambda + 1e-12;
            }

            if !gauss_solve_in_place(&mut jtj, n, &mut jtr, &mut dx) {
                lambda *= 10.0;
                if lambda > 1e8 {
                    break;
                }
                continue;
            }

            for (i, (p, d)) in x.iter().zip(dx.chunks_exact(2)).enumerate() {
                trial[i] = Point2::new(
                    p.x + d[0].clamp(-STEP_CAP, STEP_CAP),
                    p.y + d[1].clamp(-STEP_CAP, STEP_CAP),
                )
                .clamped();
            }
            self.residuals_into(&trial, &mut trial_residuals);
            let new_cost = Self::cost(&trial_residuals);
            if new_cost < cost {
                x.copy_from_slice(&trial);
                cost = new_cost;
                lambda = (lambda * 0.5).max(1e-9);
            } else {
                lambda *= 4.0;
                if lambda > 1e8 {
                    break;
                }
            }
        }

        // Convergence: tight solve, or any improvement over the start (a
        // live drag re-solves next frame; partial progress is still applied).
        status = if cost < TOL || cost < init_cost {
            SolveStatus::Converged
        } else {
            SolveStatus::MaxIterations
        };

        let positions = self
            .slots
            .iter()
            .enumerate()
            .map(|(s, &id)| {
                if let Some(v) = self.free_of[s] {
                    (id, x[v])
                } else {
                    (id, self.fixed_pos[s])
                }
            })
            .collect();
        let (max_lin_residual, max_angle_residual) = self.eq_residual_max(&x);
        Solution {
            status,
            positions,
            max_lin_residual,
            max_angle_residual,
        }
    }

    /// Largest unsatisfied equation residual at the iterate `x`, split into
    /// linear (doc units) and angular (radians) families.
    fn eq_residual_max(&self, x: &[Point2]) -> (f64, f64) {
        let mut lin = 0.0f64;
        let mut ang = 0.0f64;
        for eq in &self.eqs {
            let v = match *eq {
                Eq::Horizontal { a, b } => (self.pos(a, x).y - self.pos(b, x).y).abs(),
                Eq::Vertical { a, b } => (self.pos(a, x).x - self.pos(b, x).x).abs(),
                Eq::Distance { a, b, target } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let dx = pb.x - pa.x;
                    let dy = pb.y - pa.y;
                    ((dx * dx + dy * dy).sqrt() - target).abs()
                }
                Eq::DistanceX { a, b, target } => (self.pos(b, x).x - self.pos(a, x).x - target).abs(),
                Eq::DistanceY { a, b, target } => (self.pos(b, x).y - self.pos(a, x).y - target).abs(),
                Eq::PointLineDist { p, l1, l2, target } => {
                    let (pp, pl1, pl2) = (self.pos(p, x), self.pos(l1, x), self.pos(l2, x));
                    let dx = pl2.x - pl1.x;
                    let dy = pl2.y - pl1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    (((pp.x - pl1.x) * (-dy / l) + (pp.y - pl1.y) * (dx / l)) - target).abs()
                }
                Eq::LineDist { a1, a2, b1, target } => {
                    let (pa1, pa2, pb1) =
                        (self.pos(a1, x), self.pos(a2, x), self.pos(b1, x));
                    let dx = pa2.x - pa1.x;
                    let dy = pa2.y - pa1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    (((pb1.x - pa1.x) * (-dy / l) + (pb1.y - pa1.y) * (dx / l)) - target).abs()
                }
                Eq::Angle { a1, a2, b1, b2, target } => {
                    let mut v = signed_angle(
                        self.pos(a1, x),
                        self.pos(a2, x),
                        self.pos(b1, x),
                        self.pos(b2, x),
                    ) - target;
                    // Wrap into (-PI, PI]: correct for reflex sweeps too.
                    while v > std::f64::consts::PI {
                        v -= std::f64::consts::TAU;
                    }
                    while v <= -std::f64::consts::PI {
                        v += std::f64::consts::TAU;
                    }
                    ang = ang.max(v.abs());
                    continue;
                }
                Eq::EqualRadius { o, a, b } => {
                    let (po, pa, pb) = (self.pos(o, x), self.pos(a, x), self.pos(b, x));
                    let da = ((pa.x - po.x).powi(2) + (pa.y - po.y).powi(2)).sqrt();
                    let db = ((pb.x - po.x).powi(2) + (pb.y - po.y).powi(2)).sqrt();
                    lin = lin.max((da - db).abs());
                    continue;
                }
                Eq::ArcBend { s, e, c } => {
                    let (ps, pe, pc) = (self.pos(s, x), self.pos(e, x), self.pos(c, x));
                    let ux = pe.x - ps.x;
                    let uy = pe.y - ps.y;
                    let chord = (ux * ux + uy * uy).sqrt().max(1e-9);
                    let h = (ux * (pc.y - ps.y) - uy * (pc.x - ps.x)) / chord;
                    let min_h = (0.025 * chord).clamp(1.0, 8.0);
                    lin = lin.max((min_h - h.abs()).max(0.));
                    continue;
                }
                Eq::Tangent { l1, l2, o, p } => {
                    let (a, b, center, contact) = (self.pos(l1, x), self.pos(l2, x), self.pos(o, x), self.pos(p, x));
                    let vx = b.x - a.x;
                    let vy = b.y - a.y;
                    let rx = contact.x - center.x;
                    let ry = contact.y - center.y;
                    let lv = (vx * vx + vy * vy).sqrt().max(1e-9);
                    let lr = (rx * rx + ry * ry).sqrt().max(1e-9);
                    let m = ((lv + lr) / 2.).max(1e-9);
                    let v = (vx * rx + vy * ry).abs() / m;
                    lin = lin.max(v);
                    continue;
                }
                Eq::CirclePoint { p, o, radius } => {
                    let point = self.pos(p, x);
                    let center = self.pos(o, x);
                    lin = lin.max((((point.x - center.x).powi(2) + (point.y - center.y).powi(2)).sqrt() - radius.abs()).abs());
                    continue;
                }
                Eq::Parallel { a1, a2, b1, b2 } => {
                    let (a, b, c, d) = (self.pos(a1, x), self.pos(a2, x), self.pos(b1, x), self.pos(b2, x));
                    let ux = b.x - a.x;
                    let uy = b.y - a.y;
                    let vx = d.x - c.x;
                    let vy = d.y - c.y;
                    let lu = (ux * ux + uy * uy).sqrt().max(1e-9);
                    let lv = (vx * vx + vy * vy).sqrt().max(1e-9);
                    let m = ((lu + lv) / 2.).max(1e-9);
                    let v = (ux * vy - uy * vx).abs() / m;
                    lin = lin.max(v);
                    continue;
                }
                Eq::ArcRadius { s, e, c, target } => {
                    let (ps, pe, pc) = (self.pos(s, x), self.pos(e, x), self.pos(c, x));
                    let wx = pe.x - ps.x;
                    let wy = pe.y - ps.y;
                    let l = (wx * wx + wy * wy).sqrt().max(1e-9);
                    let ux = wx / l;
                    let uy = wy / l;
                    let m = l / 2.;
                    let h = (pc.x - ps.x) * (-uy) + (pc.y - ps.y) * ux;
                    let h_abs = h.abs().max(1e-6);
                    let v = ((m * m + h_abs * h_abs) / (2. * h_abs) - target).abs();
                    lin = lin.max(v);
                    continue;
                }
            };
            lin = lin.max(v);
        }
        (lin, ang)
    }
}

/// Intersection of two infinite lines (p0->p1) and (q0->q1). Used to find
/// the vertex for vertex-oriented angle targets.
fn line_intersection(p0: Point2, p1: Point2, q0: Point2, q1: Point2) -> Option<Point2> {
    let d1 = (p1.x - p0.x, p1.y - p0.y);
    let d2 = (q1.x - q0.x, q1.y - q0.y);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = ((q0.x - p0.x) * d2.1 - (q0.y - p0.y) * d2.0) / denom;
    Some(Point2::new(p0.x + d1.0 * t, p0.y + d1.1 * t))
}

/// Signed angle (radians) between the directions a1->a2 and b1->b2.
fn signed_angle(a1: Point2, a2: Point2, b1: Point2, b2: Point2) -> f64 {
    let ax = a2.x - a1.x;
    let ay = a2.y - a1.y;
    let bx = b2.x - b1.x;
    let by = b2.y - b1.y;
    (ax * by - ay * bx).atan2(ax * bx + ay * by)
}

/// Gaussian elimination with partial pivoting; None when singular.
fn gauss_solve_in_place(a: &mut [f64], n: usize, b: &mut [f64], out: &mut [f64]) -> bool {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    debug_assert_eq!(out.len(), n);
    for col in 0..n {
        let pivot = (col..n).max_by(|r1, r2| {
            a[*r1 * n + col]
                .abs()
                .partial_cmp(&a[*r2 * n + col].abs())
                .unwrap()
        });
        let Some(pivot) = pivot else { return false };
        if a[pivot * n + col].abs() < 1e-14 {
            return false;
        }
        if pivot != col {
            for j in col..n {
                a.swap(col * n + j, pivot * n + j);
            }
            b.swap(col, pivot);
        }
        let inv = 1.0 / a[col * n + col];
        for row in col + 1..n {
            let factor = a[row * n + col] * inv;
            if factor != 0.0 {
                for j in col..n {
                    a[row * n + j] -= factor * a[col * n + j];
                }
                b[row] -= factor * b[col];
            }
        }
    }
    for row in (0..n).rev() {
        let mut sum = b[row];
        for col in row + 1..n {
            sum -= a[row * n + col] * out[col];
        }
        out[row] = sum / a[row * n + row];
    }
    true
}
