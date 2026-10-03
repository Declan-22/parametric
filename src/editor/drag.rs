use crate::core::constraints::{ConstraintKind, DimTarget};
use crate::core::document::Document;
use crate::core::geometry::Point2;
use crate::core::ids::{PointId, SegmentId};

use super::Editor;
use super::pick;
use super::snapping::{self, SnapGuide};
use super::tools;

// Live drag system: cursor targets in, solved positions out.

// An active drag: `points` are the dragged slots (gesture-start positions;
// they chase cursor targets), `aux` are follower points freed ONLY in
// point-resize mode — they carry strong anchors so they slide along their
// constrained axes without letting the whole object translate. Everything
// else involved stays hard-fixed, which lets the solver PROJECT OUT illegal
// motion components (an edge drag can never slide its unselected opposite
// side).
pub(crate) struct DragState {
    pub points: Vec<(PointId, Point2)>,
    pub aux: Vec<(PointId, Point2)>,
    pub start_cursor: Point2,
    // Arc body grab: the whole arc translates rigidly (see plan_arc_drag).
    // None for every other gesture (solver path).
    pub arc_body_scale: Option<SegmentId>,
    // Adaptive chase scale (1.0 = full target): shrinks on rejected frames
    // so the point keeps creeping toward feasibility instead of freezing;
    // recovers on accepts. Never-drop drags.
    pub chase: f64,
}

/// Kinematic arc drag outcome: exact targets plus points to hard-pin for
/// the follow-up solve, plus an optional exact center refit to apply first.
struct ArcKin {
    targets: Vec<(PointId, Point2)>,
    pins: Vec<(PointId, Point2)>,
    premove: Option<(PointId, Point2)>,
}

impl Editor {
    // Live constraint-solve drag: cursor targets go in, solved positions
    // come out — geometry satisfies H/V/dimension constraints continuously
    // while dragging. Called from `canvas_drag` in `mod.rs`.
    pub(crate) fn solve_drag(&mut self, shift: bool) -> bool {
        let Some(drag) = self.dragging.as_ref() else {
            return false;
        };
        let p = self.cursor_doc(self.last_cursor.unwrap());
        let delta = Point2::new(p.x - drag.start_cursor.x, p.y - drag.start_cursor.y);
        if delta.x == 0. && delta.y == 0. {
            return false;
        }

        // Snap exclusion, two flavors:
        //  - exclude_pts: everything belonging to the dragged system
        //    (transitively connected) plus points co-located with a drag
        //    start. Endpoint/midpoint targets from this set are DEAD — you
        //    never relocate onto your own geometry.
        //  - exclude_segs: only the actually-dragged segments. Edge-span
        //    ALIGNMENTS from the rest of the component remain live, so a
        //    fully-connected drawing still snaps to axis alignments.
        let mut exclude_pts: Vec<PointId> = drag.points.iter().map(|(id, _)| *id).collect();
        let mut exclude_segs: Vec<crate::core::ids::SegmentId> = Vec::new();
        for &(pid, _) in &drag.aux {
            if !exclude_pts.contains(&pid) {
                exclude_pts.push(pid);
            }
        }
        let selected_pt_ids = self.doc.selection_points(&self.selection);
        for pid in &selected_pt_ids {
            if !exclude_pts.contains(pid) {
                exclude_pts.push(*pid);
            }
        }
        // Segments with BOTH ends in the dragged set are the ones being
        // manipulated; their spans are dead.
        for (sid, s) in self.doc.all_segments() {
            if exclude_pts.contains(&s.start)
                && exclude_pts.contains(&s.end)
                && (drag.points.iter().chain(drag.aux.iter()).any(|&(id, _)| id == s.start))
                && (selected_pt_ids.contains(&s.start) || selected_pt_ids.contains(&s.end))
            {
                exclude_segs.push(sid);
            }
        }
        // Transitive closure along segments: every point REACHABLE from the
        // dragged system is part of the dragged object(s). A partially
        // selected rectangle must never snap back onto its own unselected
        // far corner — NOTHING ever snaps to its own geometry.
        let mut i = 0;
        while i < exclude_pts.len() {
            let pid = exclude_pts[i];
            for (_, s) in self.doc.all_segments() {
                let other = if s.start == pid {
                    Some(s.end)
                } else if s.end == pid {
                    Some(s.start)
                } else {
                    None
                };
                if let Some(o) = other
                    && !exclude_pts.contains(&o)
                    && self.doc.point(o).is_some()
                {
                    exclude_pts.push(o);
                }
            }
            i += 1;
        }
        // Arc defining points form ONE snap-unit: the ctrl point is not a
        // segment endpoint (the closure above never reaches it), so dragging
        // the bend would otherwise snap to its own arc's endpoints/midpoint.
        // If any of the three is excluded, all three are.
        let mut arc_closure = true;
        while arc_closure {
            arc_closure = false;
            for (_, s) in self.doc.all_segments() {
                if s.kind != crate::core::document::SegmentKind::Arc {
                    continue;
                }
                let Some(ctrl) = s.ctrl else { continue };
                let defs = [s.start, s.end, ctrl];
                let any_in = defs.iter().any(|d| exclude_pts.contains(d));
                let any_out = defs.iter().any(|d| !exclude_pts.contains(d));
                if any_in && any_out {
                    for d in defs {
                        if !exclude_pts.contains(&d) {
                            exclude_pts.push(d);
                        }
                    }
                    arc_closure = true;
                }
            }
        }
        // Points CO-LOCATED with a dragged point's start (e.g. a partner
        // whose coincident constraint was just deleted) must not be snap
        // targets either — otherwise the point can never be pulled away.
        let tol = self.snap_tol_doc();
        let starts: Vec<Point2> = drag.points.iter().map(|&(_, s)| s).collect();
        for (pid, p) in self.doc.all_points() {
            if !exclude_pts.contains(&pid)
                && starts.iter().any(|s| pick::distance(*s, p) <= tol)
            {
                exclude_pts.push(pid);
            }
        }

        // Single-endpoint drags of STANDALONE lines only snap when the
        // endpoint truly lands on another point — a passing axis alignment
        // must not yank one axis (the slight 90-degree snap).
        // Line endpoints get the FULL snap vocabulary — axis alignment to
        // distant points/edges included. Other single-point resizes stay
        // fluid (endpoints only); multi-point moves get everything too.
        let bare_line_endpoint =
            drag.points.len() == 1 && self.pid_on_bare_line(drag.points[0].0);
        let endpoints_only = drag.points.len() == 1 && !bare_line_endpoint;

        // Per-axis consensus voting: every dragged point proposes its own
        // snap corrections; each axis adopts the most-agreed proposal and
        // applies it RIGIDLY to all points. Grid and object snaps work
        // TOGETHER: objects keep priority, the drawn grid's intersections
        // fill the axes objects leave free. The adopted proposal's guides
        // are kept so the connection lines render during the drag — and
        // they record WHICH point locked, so the badge/stub can be
        // re-anchored at its FINAL (post-solver) position.
        let mut proposals: Vec<(PointId, f64, f64, Vec<SnapGuide>)> = Vec::new();
        if !self.alt_down && (self.snap_to_objects || self.snap_to_grid) {
            for &(pid, start) in &drag.points {
                // Fillet derived points never vote: their positions are
                // re-derived every frame, so their snap proposals would
                // yank the whole rigid consensus in jumps.
                let derived = self.doc.modifiers.iter().any(|m| {
                    Some(pid) == m.first_tangent
                        || Some(pid) == m.second_tangent
                        || Some(pid) == m.center
                        || Some(pid) == m.control
                });
                if derived {
                    continue;
                }
                let target = Point2::new(start.x + delta.x, start.y + delta.y);
                let (adj, guides) = snapping::best(
                    &self.doc,
                    self.snap_tol_doc(),
                    target,
                    &exclude_pts,
                    &exclude_segs,
                    endpoints_only,
                    false,
                    self.snap_visible(),
                    self.grid_step(),
                    self.camera.zoom,
                );
                if adj.x != 0. || adj.y != 0. {
                    proposals.push((pid, adj.x, adj.y, guides));
                }
            }
        }
        let consensus = |props: &[f64]| -> Option<f64> {
            props.iter().copied().min_by(|a, b| {
                let sa: f64 = props.iter().map(|p| (p - a).abs()).sum();
                let sb: f64 = props.iter().map(|p| (p - b).abs()).sum();
                sa.total_cmp(&sb)
            })
        };
        let xs: Vec<f64> = proposals.iter().map(|p| p.1).collect();
        let ys: Vec<f64> = proposals.iter().map(|p| p.2).collect();
        let sx = consensus(&xs).unwrap_or(0.);
        let sy = consensus(&ys).unwrap_or(0.);
        let matched = if sx == 0. && sy == 0. {
            None
        } else {
            proposals
                .iter()
                .find(|(_, x, y, _)| {
                    (sx == 0. || (x - sx).abs() < 1e-9) && (sy == 0. || (y - sy).abs() < 1e-9)
                })
                .cloned()
        };
        let locked_pid = matched.as_ref().map(|(pid, _, _, _)| *pid);
        let snap_guides: Vec<SnapGuide> = matched.map(|(_, _, _, g)| g).unwrap_or_default();
        if self.snap_guides != snap_guides {
            self.snap_guides = snap_guides;
        }
        let snapped_delta = Point2::new(delta.x + sx, delta.y + sy);

        let mut targets: Vec<(PointId, Point2)> = drag
            .points
            .iter()
            .map(|&(pid, start)| {
                (pid, Point2::new(start.x + snapped_delta.x, start.y + snapped_delta.y))
            })
            .collect();

        // Never-drop drags: clamp each target to a per-frame chase radius
        // around the point's CURRENT position. A cursor that sprints past
        // feasibility used to hard-freeze the point (hold-last-valid every
        // frame) until re-click; now the point chases every frame and only
        // genuinely locked directions resist. Shift adjustments below can
        // still override (arc kinematics are exact-feasible by design).
        // The adaptive `chase` scale (see reject/accept below) then shrinks
        // the clamped target toward feasibility whenever frames reject.
        const CHASE_RADIUS: f64 = 150.0;
        let chase = drag.chase;
        for (pid, target) in targets.iter_mut() {
            if let Some(cur) = self.doc.point(*pid) {
                let dx = target.x - cur.x;
                let dy = target.y - cur.y;
                let d = (dx * dx + dy * dy).sqrt();
                if d > CHASE_RADIUS {
                    let s = CHASE_RADIUS / d;
                    target.x = cur.x + dx * s;
                    target.y = cur.y + dy * s;
                }
                if chase < 1.0 {
                    target.x = cur.x + (target.x - cur.x) * chase;
                    target.y = cur.y + (target.y - cur.y) * chase;
                }
            }
        }

        // Shift on a single-endpoint drag: ARC points get arc-specific
        // constraints (rotate on circle / sweep snap); everything else snaps
        // the direction to 45-degree steps around the segment's other
        // endpoint.
        if shift && targets.len() == 1 {
            let (pid, target) = targets[0];
            if !self.arc_shift_target(pid, target, &drag.points, &mut targets[0].1) {
                snap_target_direction(&self.doc, &mut targets);
            }
        }

        // ANGLE LOCK: lines participating in an angle dimension with a
        // dragged point get their remaining defining points freed as
        // soft-anchored followers — the angle equation (1e6 weight) then
        // rotates the connected geometry to preserve the angle instead of
        // letting the drag shear it by degrees. Constraints are constraints.
        let mut aux_all: Vec<(PointId, Point2)> = drag.aux.clone();
        let dragged: Vec<PointId> = drag.points.iter().map(|&(id, _)| id).collect();
        for d in &self.doc.dimensions {
            let DimTarget::Angle { a, b } = &d.target else {
                continue;
            };
            for sid in [*a, *b] {
                let Some(seg) = self.doc.segment(sid) else { continue };
                let touches_drag = dragged.iter().any(|&p| p == seg.start || p == seg.end);
                if !touches_drag {
                    continue;
                }
                for pid in [seg.start, seg.end] {
                    if dragged.contains(&pid) || self.doc.point(pid).is_none() {
                        continue;
                    }
                    let pos = self.doc.point(pid).unwrap();
                    if !aux_all.iter().any(|&(id, _)| id == pid) {
                        aux_all.push((pid, pos));
                    }
                }
            }
        }

        // A line-edge drag can include the shared tangent point in its drag
        // set. That point belongs to the arc too, but the gesture intent is
        // still “move the line”, not “reshape the arc”. Pin the complete arc
        // in this case; the tangent solver then rotates only the line around
        // the fixed contact and preserves its length.
        let mut tangent_arc_pins: Vec<(PointId, Point2)> = Vec::new();
        if drag.arc_body_scale.is_none() {
            for constraint in &self.doc.constraints {
                if constraint.kind != ConstraintKind::Tangent {
                    continue;
                }
                let Some((first_id, second_id)) = constraint.tangent_segments else { continue };
                let (Some(first), Some(second)) = (self.doc.segment(first_id), self.doc.segment(second_id)) else { continue };
                let (line, curve, curve_id) = match (first.kind, second.kind) {
                    (crate::core::document::SegmentKind::Line, crate::core::document::SegmentKind::Arc) => (first, second, second_id),
                    (crate::core::document::SegmentKind::Arc, crate::core::document::SegmentKind::Line) => (second, first, first_id),
                    _ => continue,
                };
                // Fillet contacts slide by definition (tangent points ride
                // their edges as the radius changes) — pinning the arc
                // here would freeze edge-stretch and group drags touching
                // filleted geometry. The solver + refresh keep it exact.
                if self.doc.modifiers.iter().any(|m| m.arc == Some(curve_id)) {
                    continue;
                }
                let Some(ctrl) = curve.ctrl else { continue };
                let arc_ids = [curve.start, curve.end, ctrl, curve.center.unwrap_or(curve.start)];
                let line_ids = [line.start, line.end];
                let arc_dragged = arc_ids[..3].iter().any(|id| dragged.contains(id));
                let line_dragged = line_ids.iter().any(|id| dragged.contains(id));
                let far_line_dragged = line_ids.iter().any(|id| dragged.contains(id) && *id != constraint.a);
                if !arc_dragged || !line_dragged || !far_line_dragged {
                    continue;
                }
                for pid in arc_ids {
                    if let Some(pos) = self.doc.point(pid)
                        && !tangent_arc_pins.iter().any(|(id, _)| *id == pid)
                    {
                        tangent_arc_pins.push((pid, pos));
                    }
                }
            }
        }

        // Transitive follower closure: every point reachable from the
        // drag set through constraints (pairs, tangent owners,
        // point-on-segment edges) and arc construction slots follows
        // softly. The single-hop ring above leaves two-hops-away points
        // (opposite rectangle corners, fillet tangent contacts) hard-pinned
        // at canvas positions, freezing corner drags and making edge
        // drags fight. Soft anchors still regularize (minimal motion);
        // constraints dominate. Dragged and hard-pinned ids are never
        // double-entered as followers.
        {
            let mut seen: Vec<PointId> = dragged
                .iter()
                .copied()
                .chain(aux_all.iter().map(|&(id, _)| id))
                .collect();
            let mut stack = seen.clone();
            while let Some(pid) = stack.pop() {
                // Constraint relations.
                for c in &self.doc.constraints {
                    if c.a == pid || c.b == pid {
                        for q in [c.a, c.b] {
                            if !seen.contains(&q) && self.doc.point(q).is_some() {
                                seen.push(q);
                                stack.push(q);
                            }
                        }
                    }
                    if let Some(segment_id) = c.point_on_segment
                        && let Some(seg) = self.doc.segment(segment_id)
                    {
                        let touches = seg.start == pid
                            || seg.end == pid
                            || seg.ctrl == Some(pid)
                            || seg.center == Some(pid);
                        if touches {
                            for q in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center]
                                .into_iter()
                                .flatten()
                                .chain([c.a, c.b])
                            {
                                if !seen.contains(&q) && self.doc.point(q).is_some() {
                                    seen.push(q);
                                    stack.push(q);
                                }
                            }
                        }
                    }
                    if c.kind == ConstraintKind::Tangent
                        || c.kind == ConstraintKind::Parallel
                        || c.kind == ConstraintKind::Perpendicular
                    {
                        if let Some((first, second)) = c.tangent_segments
                            && let (Some(a_seg), Some(b_seg)) =
                                (self.doc.segment(first), self.doc.segment(second))
                        {
                            let ends = [
                                a_seg.start,
                                a_seg.end,
                                b_seg.start,
                                b_seg.end,
                            ];
                            if ends.contains(&pid)
                                || c.a == pid
                                || c.b == pid
                                || a_seg.ctrl == Some(pid)
                                || a_seg.center == Some(pid)
                                || b_seg.ctrl == Some(pid)
                                || b_seg.center == Some(pid)
                            {
                                for q in ends
                                    .into_iter()
                                    .chain([c.a, c.b])
                                    .chain(
                                        [a_seg.ctrl, a_seg.center, b_seg.ctrl, b_seg.center]
                                            .into_iter()
                                            .flatten(),
                                    )
                                {
                                    if !seen.contains(&q) && self.doc.point(q).is_some() {
                                        seen.push(q);
                                        stack.push(q);
                                    }
                                }
                            }
                        }
                    }
                }
                // Arc construction slots: an arc's four defining points
                // move as one follower set.
                for (_, s) in self.doc.all_segments() {
                    if s.start == pid
                        || s.end == pid
                        || s.ctrl == Some(pid)
                        || s.center == Some(pid)
                    {
                        for q in [Some(s.start), Some(s.end), s.ctrl, s.center]
                            .into_iter()
                            .flatten()
                        {
                            if !seen.contains(&q) && self.doc.point(q).is_some() {
                                seen.push(q);
                                stack.push(q);
                            }
                        }
                    }
                }
            }
            for pid in seen {
                if dragged.contains(&pid)
                    || aux_all.iter().any(|&(id, _)| id == pid)
                    || tangent_arc_pins.iter().any(|(id, _)| *id == pid)
                {
                    continue;
                }
                if let Some(pos) = self.doc.point(pid) {
                    aux_all.push((pid, pos));
                }
            }
        }

        // Kinematic arc drags bypass the least-squares hunt entirely:
        // endpoints slide on the circle, the bend refits it, the body
        // scales about the center. Everything else takes the solver path.
        let arc_plan = if tangent_arc_pins.is_empty() {
            self.plan_arc_drag(drag, &targets)
        } else {
            // The line owns this gesture; do not let arc endpoint kinematics
            // reinterpret it as an arc reshape.
            None
        };
        let solver = match arc_plan {
            Some(kin) => {
                if let Some((pid, pos)) = kin.premove {
                    self.doc.move_point(pid, pos);
                }
                // Kinematic arc positions are authoritative. Feed only the
                // non-arc drag targets into the solver; otherwise LM gets a
                // second chance to reinterpret a rigid arc move as a branch
                // change. Tangent followers remain free through aux_all and
                // are solved against the now-fixed arc.
                let kin_ids: std::collections::HashSet<PointId> =
                    kin.targets.iter().map(|&(id, _)| id).collect();
                for &(pid, pos) in &kin.targets {
                    self.doc.move_point(pid, pos);
                }
                let external: Vec<(PointId, Point2)> = targets
                    .iter()
                    .copied()
                    .filter(|(id, _)| !kin_ids.contains(id))
                    .collect();
                let mut pins = kin.pins;
                pins.extend(kin.targets.iter().copied());
                // 1.0 = the default live-drag anchor weight.
                crate::core::solver::Solver::build_pinned(
                    &self.doc,
                    &external,
                    &aux_all,
                    &pins,
                    1.0,
                )
            }
            None if tangent_arc_pins.is_empty() => {
                crate::core::solver::Solver::build(&self.doc, &targets, &aux_all)
            }
            None => crate::core::solver::Solver::build_pinned(
                &self.doc,
                &targets,
                &aux_all,
                &tangent_arc_pins,
                1.0,
            ),
        };
        // Live drags never solve fillet internals (refresh re-seats them
        // exactly after every commit); the stiff derived equations would
        // otherwise freeze drags touching filleted geometry.
        let mut solver = solver;
        solver.strip_fillet_equations(&self.doc);
        solver.strip_curve_length();
        let solution = solver.solve();
        // A live drag may request an impossible step, but applying a partial
        // LM iterate would visibly break a locked constraint. Shrink the
        // chase instead of freezing: the next frame tries closer to the
        // current position, so feasible micro-steps still get through and
        // only truly locked directions hold.
        if !solution.constraints_satisfied() {
            self.snap_guides.clear();
            if let Some(d) = self.dragging.as_mut() {
                d.chase = (d.chase * 0.5).max(1.0 / 32.0);
            }
            return true;
        }
        let mut moved: std::collections::HashSet<PointId> = std::collections::HashSet::new();
        for (id, pos) in solution.positions {
            moved.insert(id);
            self.doc.move_point(id, pos);
        }
        // Clay: re-derive managed bezier handles post-solve (free handles
        // only; constrained ones are skipped inside).
        self.derive_handles();
        self.enforce_arc_coincident_joints(&dragged);
        self.enforce_tangencies();
        // Arc consistency is enforced by solver equations; no post-solve
        // mutation is allowed to overwrite other constraints.
        // Re-anchor the connection guides at the locked point's FINAL
        // position — the solver may project it off the raw proposal, and a
        // badge/stub that trails the cursor instead of sitting on the point
        // is worse than no feedback at all. Endpoint features DIRECTLY
        // connected to the locked point by existing geometry also get their
        // stub suppressed: the shape's own edge already draws that
        // connection.
        if let Some(pid) = locked_pid
            && let Some(p) = self.doc.point(pid)
        {
            self.snap_guides = self
                .snap_guides
                .iter()
                .map(|g| {
                    let mut g = *g;
                    g.to = p;
                    if g.kind == snapping::SnapKind::Endpoint
                        && self.point_directly_linked(pid, g.from)
                    {
                        g.linked = true;
                    }
                    g
                })
                .collect();
        }
        // Feasible frame: relax the chase back toward full targets.
        if let Some(d) = self.dragging.as_mut() {
            d.chase = (d.chase * 2.0).min(1.0);
        }
        true
    }

    /// Coincident arc joints are topological attachments, not merely a
    /// numerical preference. Kinematic arc drags pin the arc points, so a
    /// separate line endpoint must be copied onto the moved arc point after
    /// the follower solve or it can visibly detach for a frame.
    fn enforce_arc_coincident_joints(&mut self, dragged: &[PointId]) {
        let dragged: std::collections::HashSet<PointId> = dragged.iter().copied().collect();
        let touched_arcs: Vec<[PointId; 3]> = self
            .doc
            .all_segments()
            .filter(|(_, s)| s.kind == crate::core::document::SegmentKind::Arc)
            .filter_map(|(_, s)| {
                let ctrl = s.ctrl?;
                let defs = [s.start, s.end, ctrl];
                defs.iter().any(|id| dragged.contains(id)).then_some(defs)
            })
            .collect();
        if touched_arcs.is_empty() {
            return;
        }
        for c in self.doc.constraints.clone() {
            if c.kind != ConstraintKind::Coincident || c.point_on_segment.is_some() {
                continue;
            }
            let arc_point = if touched_arcs.iter().any(|defs| defs.contains(&c.a)) {
                c.a
            } else if touched_arcs.iter().any(|defs| defs.contains(&c.b)) {
                c.b
            } else {
                continue;
            };
            let other = if arc_point == c.a { c.b } else { c.a };
            let Some(pos) = self.doc.point(arc_point) else { continue };
            if self.doc.point(other).is_some() {
                self.doc.move_point(other, pos);
            }
        }
    }

    /// True when a document segment directly connects `pid` to a point at
    /// `feature` (same position) — the two snapping pieces are already
    /// joined by visible geometry.
    fn point_directly_linked(&self, pid: PointId, feature: Point2) -> bool {
        for (_, s) in self.doc.all_segments() {
            let other = if s.start == pid {
                Some(s.end)
            } else if s.end == pid {
                Some(s.start)
            } else {
                None
            };
            if let Some(o) = other
                && let Some(q) = self.doc.point(o)
                && pick::distance(q, feature) < 1e-6
            {
                return true;
            }
        }
        false
    }

    /// Kinematic arc drag plan: exact geometry instead of the least-squares
    /// hunt, so an arc can never flip unless the cursor commands it.
    ///  - endpoint (start/end): slides along the existing circle (center +
    ///    radius frozen, other points pinned), clamped away from the other
    ///    endpoint and the bend so the sweep can never invert;
    ///  - ctrl (on-curve point): moves freely with a bend-height floor and
    ///    the center refit exactly through the pinned endpoints;
    ///  - body (DragState::arc_body_scale): rigid translation of the arc;
    ///  - center: rigid translate, already exact — legacy solver path.
    /// Pins are hard-fixed for the follow-up solve so tangent lines rotate
    /// around the frozen arc instead of throwing it around. None = legacy
    /// solver path (multi-arc drags, H/V locks, non-radius dimensions).
    fn plan_arc_drag(&self, drag: &DragState, targets: &[(PointId, Point2)]) -> Option<ArcKin> {
        use crate::core::document::SegmentKind;
        const TAU: f64 = std::f64::consts::TAU;
        const PI: f64 = std::f64::consts::PI;
        let wrap = |mut d: f64| {
            d = (d + PI).rem_euclid(TAU);
            d - PI
        };

        let drag_ids: Vec<PointId> = drag.points.iter().map(|&(id, _)| id).collect();
        // Exactly one arc touched, else legacy (e.g. shared vertices).
        // Fillet arcs never lead kinematically: they are derived
        // geometry (refresh re-seats them every frame), so a kinematic
        // plan would fight the refresh and warp.
        let mut arc_sid = None;
        for (sid, s) in self.doc.all_segments() {
            if s.kind != SegmentKind::Arc {
                continue;
            }
            if self.doc.modifiers.iter().any(|m| m.arc == Some(sid)) {
                continue;
            }
            let mut defs = vec![s.start, s.end];
            if let Some(c) = s.ctrl {
                defs.push(c);
            }
            if let Some(c) = s.center {
                defs.push(c);
            }
            if drag_ids.iter().any(|id| defs.contains(id)) {
                if arc_sid.is_some() {
                    return None;
                }
                arc_sid = Some(sid);
            }
        }
        let sid = arc_sid?;
        let seg = self.doc.segment(sid)?;
        let ctrl = seg.ctrl?;
        let center_id = seg.center?;
        let (Some(a), Some(b), Some(c), Some(_o)) = (
            self.doc.point(seg.start),
            self.doc.point(seg.end),
            self.doc.point(ctrl),
            self.doc.point(center_id),
        ) else {
            return None;
        };
        let Some((cc, r)) = crate::editor::arc::circumcircle(a, b, c) else {
            return None;
        };
        if !(r > 1e-9) {
            return None;
        }
        let chord = pick::distance(a, b);
        if !(chord > 1e-9) {
            return None;
        }
        let arc_ids = [seg.start, seg.end, ctrl, center_id];
        // H/V locks on arc points need the solver. Legacy path.
        for con in &self.doc.constraints {
            match con.kind {
                ConstraintKind::Horizontal | ConstraintKind::Vertical
                    if arc_ids.contains(&con.a) || arc_ids.contains(&con.b) =>
                {
                    return None;
                }
                _ => {}
            }
        }
        let radius_locked = self.doc.dimensions.iter().any(|d| {
            matches!(
                d.target,
                crate::core::constraints::DimTarget::Radius { seg: s } if s == sid
            )
        });
        // Any non-radius dimension touching the arc needs the solver.
        for d in &self.doc.dimensions {
            match &d.target {
                crate::core::constraints::DimTarget::Radius { seg: s } if *s == sid => {}
                crate::core::constraints::DimTarget::Points { a: da, b: db, .. }
                    if arc_ids.contains(da) || arc_ids.contains(db) =>
                {
                    return None;
                }
                crate::core::constraints::DimTarget::PointLine { p, .. }
                    if arc_ids.contains(p) =>
                {
                    return None;
                }
                _ => {}
            }
        }

        let target_of = |pid: PointId| -> Option<Point2> {
            targets.iter().find(|(id, _)| *id == pid).map(|&(_, t)| t)
        };
        let in_drag = |pid: PointId| drag_ids.contains(&pid);
        let partners: Vec<PointId> = drag_ids
            .iter()
            .copied()
            .filter(|id| *id != seg.start && *id != seg.end && *id != ctrl && *id != center_id)
            .collect();

        // Body: uniform scale about the frozen (healed) center. A locked
        // radius can't scale — legacy translate preserves it exactly.
        if drag.arc_body_scale == Some(sid) {
            let Some(ct) = target_of(ctrl) else {
                return None;
            };
            // An edge grab is a move gesture, not a radius-edit gesture.
            // The old implementation scaled around the center, which made a
            // tangent-connected arc resize/spin while the cursor was merely
            // translating across its edge.
            let delta = Point2::new(ct.x - c.x, ct.y - c.y);
            let mut out = Vec::with_capacity(4);
            for (pid, s) in [(seg.start, a), (seg.end, b), (ctrl, c), (center_id, cc)] {
                out.push((pid, Point2::new(s.x + delta.x, s.y + delta.y)));
            }
            return Some(ArcKin {
                targets: out,
                pins: Vec::new(),
                premove: None,
            });
        }

        let ctrl_d = in_drag(ctrl);
        let start_d = in_drag(seg.start);
        let end_d = in_drag(seg.end);

        // Dragging the center is a rigid translation. It must never enter the
        // general arc solve: the solver can satisfy tangent/radius equations
        // by changing the sweep branch even though every arc point is being
        // asked to move by the same delta.
        if in_drag(center_id) {
            let Some(t0) = target_of(center_id) else { return None };
            let delta = Point2::new(t0.x - cc.x, t0.y - cc.y);
            return Some(ArcKin {
                targets: vec![
                    (seg.start, Point2::new(a.x + delta.x, a.y + delta.y)),
                    (seg.end, Point2::new(b.x + delta.x, b.y + delta.y)),
                    (ctrl, Point2::new(c.x + delta.x, c.y + delta.y)),
                    (center_id, t0),
                ],
                pins: Vec::new(),
                premove: None,
            });
        }

        // Endpoint: slide along the existing circle. Do not reserve a hidden
        // exclusion zone around the other endpoint or construction marker:
        // arcs are allowed to become very small, nearly closed spans.
        // If external segment endpoints (like a line or bezier) are being dragged,
        // bypass kinematic arc drag so the solver solves the multi-point drag.
        if (start_d ^ end_d) && !ctrl_d {
            let e_pid = if start_d { seg.start } else { seg.end };
            let Some(t0) = target_of(e_pid) else {
                return None;
            };
            let ang = |p: Point2| (p.y - cc.y).atan2(p.x - cc.x);
            let e_cur = if start_d { a } else { b };
            let th_prev = ang(e_cur);
            let dth = wrap(ang(t0) - th_prev).clamp(-0.2, 0.2);
            let th = th_prev + dth;

            // Preserve the current signed sweep. Recomputing the sweep from
            // the hidden construction point each frame lets an endpoint
            // crossing the atan2 seam turn a small arc into its 360-degree
            // complement. Move that internal marker to the midpoint of the
            // same directed sweep instead.
            let a0 = ang(a);
            let b0 = ang(b);
            let c0 = ang(c);
            let forward = (b0 - a0).rem_euclid(std::f64::consts::TAU);
            let c_forward = (c0 - a0).rem_euclid(std::f64::consts::TAU);
            let current_sweep = if c_forward <= forward {
                forward
            } else {
                forward - std::f64::consts::TAU
            };
            let positive_sweep = current_sweep >= 0.0;
            let (new_start, new_end) = if start_d {
                (th, b0)
            } else {
                (a0, th)
            };
            let sweep = if positive_sweep {
                wrap(new_end - new_start).rem_euclid(std::f64::consts::TAU)
            } else {
                -wrap(new_start - new_end).rem_euclid(std::f64::consts::TAU)
            };
            let sweep = if sweep.abs() < 1e-6 {
                if positive_sweep { 1e-6 } else { -1e-6 }
            } else {
                sweep
            };
            let ctrl_angle = new_start + sweep * 0.5;
            let ctrl_target = Point2::new(
                cc.x + r * ctrl_angle.cos(),
                cc.y + r * ctrl_angle.sin(),
            );
            let t = Point2::new(cc.x + r * th.cos(), cc.y + r * th.sin());
            let pins = vec![(seg.start, a), (seg.end, b), (center_id, cc)];
            let pins = pins.into_iter().filter(|(id, _)| *id != e_pid).collect();
            let mut out = Vec::with_capacity(targets.len());
            for &(pid, _) in targets {
                if [seg.start, seg.end, ctrl, center_id].contains(&pid) {
                    let original = if pid == seg.start { a }
                        else if pid == seg.end { b }
                        else if pid == ctrl { ctrl_target }
                        else { cc };
                    out.push((pid, if pid == e_pid { t } else { original }));
                }
            }
            if !out.iter().any(|(pid, _)| *pid == ctrl) {
                out.push((ctrl, ctrl_target));
            }
            return Some(ArcKin {
                targets: out,
                pins,
                premove: Some((center_id, cc)),
            });
        }

        // Bend handle: free move with a bend-height floor; the center is
        // refit exactly through the pinned endpoints. A locked radius can't
        // refit — legacy path (which preserves it).
        if ctrl_d && !start_d && !end_d {
            if radius_locked || !partners.is_empty() {
                return None;
            }
            let Some(t0) = target_of(ctrl) else {
                return None;
            };
            let ux = (b.x - a.x) / chord;
            let uy = (b.y - a.y) / chord;
            let (nx, ny) = (-uy, ux);
            let h_cur = (c.x - a.x) * nx + (c.y - a.y) * ny;
            let mut h = (t0.x - a.x) * nx + (t0.y - a.y) * ny;
            let floor = (0.025 * chord).clamp(1.0, 8.0);
            if h.abs() < floor {
                let s = if h_cur >= 0. { 1.0 } else { -1.0 };
                h = s * floor;
            }
            // Bend slides freely along the chord direction; only the height
            // off the chord is floored.
            let t_along = (t0.x - a.x) * ux + (t0.y - a.y) * uy;
            let t_pt = Point2::new(a.x + ux * t_along + nx * h, a.y + uy * t_along + ny * h);
            let Some((cc2, _)) = crate::editor::arc::circumcircle(a, b, t_pt) else {
                return None;
            };
            let mut out = Vec::with_capacity(targets.len());
            for &(pid, _) in targets {
                out.push((pid, t_pt));
            }
            return Some(ArcKin {
                targets: out,
                pins: vec![(seg.start, a), (seg.end, b), (center_id, cc2)],
                premove: Some((center_id, cc2)),
            });
        }

        None
    }
    /// SHIFT constraint for single-point drags of ARC defining points:
    ///  - start/end point: ROTATE ON CIRCLE — the target is projected
    ///    radially onto the arc's original circle (the one at drag start),
    ///    so the endpoint spins around the circle instead of warping it;
    ///  - ctrl point: sweep snaps to 90-degree steps (perfect quarter /
    ///    half / three-quarter arc).
    /// Returns false when pid belongs to no arc (caller falls back to the
    /// generic 45-degree direction snap).
    fn arc_shift_target(
        &self,
        pid: PointId,
        target: Point2,
        drag_points: &[(PointId, Point2)],
        out: &mut Point2,
    ) -> bool {
        for (sid, s) in self.doc.all_segments() {
            if s.kind != crate::core::document::SegmentKind::Arc {
                continue;
            }
            // Fillet arcs never spin kinematically (derived geometry).
            if self.doc.modifiers.iter().any(|m| m.arc == Some(sid)) {
                continue;
            }
            let Some(ctrl_id) = s.ctrl else { continue };
            let is_end = s.start == pid || s.end == pid;
            let is_ctrl = ctrl_id == pid;
            if !is_end && !is_ctrl {
                continue;
            }
            let (Some(pa), Some(pb), Some(pc)) =
                (self.doc.point(s.start), self.doc.point(s.end), self.doc.point(ctrl_id))
            else {
                return true;
            };
            // Defining triangle at DRAG START: the dragged point's original
            // position plus the two unmoved points — that circle is the one
            // worth preserving.
            let pos = |id: PointId, cur: Point2| {
                drag_points
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map(|(_, start)| *start)
                    .unwrap_or(cur)
            };
            let (a, b, c) = (pos(s.start, pa), pos(s.end, pb), pos(ctrl_id, pc));
            if is_end {
                if let Some((o, r)) = crate::editor::arc::circumcircle(a, b, c) {
                    let dx = target.x - o.x;
                    let dy = target.y - o.y;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d > 1e-9 {
                        *out = Point2::new(o.x + dx / d * r, o.y + dy / d * r);
                    }
                }
            } else if let Some(snapped) = crate::editor::arc::snap_sweep(a, b, target) {
                *out = snapped;
            }
            return true;
        }
        false
    }

    /// Keeps every arc consistent after a drag. Two regimes:
    ///  - center MOVED by the solver (dragged directly or towed via a
    ///    coincident constraint): the arc translates rigidly so its
    ///    circumcenter lands on the center's new position — constraints on
    ///    the center are honored.
    ///  - center UNMOVED: it follows the geometry (recomputed circumcenter),
    ///    EXCEPT when the center itself is coincident-constrained — then the
    ///    defining points are projected back onto the circle around the
    ///    pinned center instead.
    fn resolve_arcs_after_drag(&mut self, moved: &std::collections::HashSet<PointId>) {
        let arcs: Vec<(crate::core::ids::SegmentId, crate::core::document::Segment)> = self
            .doc
            .all_segments()
            .filter(|(_, s)| s.kind == crate::core::document::SegmentKind::Arc)
            .map(|(id, s)| (id, s))
            .collect();
        for (seg_id, seg) in arcs {
            let (Some(center_id), Some(ctrl_id)) = (seg.center, seg.ctrl) else {
                continue;
            };
            let (Some(mut a), Some(mut b), Some(mut c)) = (
                self.doc.point(seg.start),
                self.doc.point(seg.end),
                self.doc.point(ctrl_id),
            ) else {
                continue;
            };
            let Some(old_o) = crate::editor::arc::circumcircle(a, b, c).map(|(o, _)| o) else {
                continue;
            };
            let Some(center_now) = self.doc.point(center_id) else {
                continue;
            };
            let center_constrained = self.doc.constraints.iter().any(|c| {
                c.kind == ConstraintKind::Coincident
                    && (c.a == center_id || c.b == center_id)
            }) || self.doc.dimensions.iter().any(|d| {
                matches!(d.target, DimTarget::Radius { seg: sid } if sid == seg_id)
            });
            if moved.contains(&center_id) {
                // Center is authoritative: rigid-translate the defining
                // points that the solver did NOT position.
                let d = Point2::new(center_now.x - old_o.x, center_now.y - old_o.y);
                for (pid, pos) in [
                    (seg.start, a),
                    (seg.end, b),
                    (ctrl_id, c),
                ] {
                    if !moved.contains(&pid) {
                        self.doc
                            .move_point(pid, Point2::new(pos.x + d.x, pos.y + d.y));
                    }
                }
                continue;
            }
            if center_constrained {
                // The solver positioned ALL defining points: its circumcircle
                // is authoritative — re-projecting onto a circle around the
                // pinned center would clobber the solve (radius dims set to
                // 30 landing back at the old 35). Trust the solve; the
                // pinned center only anchors when points were moved by hand.
                let locked_radius = self.doc.dimensions.iter().find_map(|d| match d.target {
                    DimTarget::Radius { seg: s } if s == seg_id => Some(d.value),
                    _ => None,
                });
                if locked_radius.is_none() && moved.contains(&seg.start) && moved.contains(&seg.end) && moved.contains(&ctrl_id) {
                    continue;
                }
                // Center pinned by a constraint: keep all defining points on
                // the circle around it. Radius anchors to whichever defining
                // point the user did NOT move.
                let radius = if let Some(radius) = locked_radius {
                    radius.abs()
                } else if !moved.contains(&seg.start) {
                    pick::distance(center_now, a)
                } else if !moved.contains(&seg.end) {
                    pick::distance(center_now, b)
                } else if !moved.contains(&ctrl_id) {
                    pick::distance(center_now, c)
                } else {
                    pick::distance(center_now, a)
                };
                if radius < 1e-6 {
                    continue;
                }
                for (pid, pos) in [(seg.start, a), (seg.end, b), (ctrl_id, c)] {
                    let dx = pos.x - center_now.x;
                    let dy = pos.y - center_now.y;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < 1e-9 {
                        continue;
                    }
                    self.doc.move_point(
                        pid,
                        Point2::new(
                            center_now.x + dx / d * radius,
                            center_now.y + dy / d * radius,
                        ),
                    );
                }
                continue;
            }
            // Free center: follow the geometry. The defining points may have
            // moved during the solve, so the old circumcenter is stale and
            // would leave the center handle detached from the visible arc.
            if let Some((new_o, _)) = crate::editor::arc::circumcircle(a, b, c) {
                self.doc.move_point(center_id, new_o);
            }
        }
    }

    /// True when pid is an endpoint of a standalone stroked line (line
    /// tool output, not part of any fill).
    fn pid_on_bare_line(&self, pid: PointId) -> bool {
        self.doc.all_segments().any(|(sid, s)| {
            (s.start == pid || s.end == pid)
                && s.kind == crate::core::document::SegmentKind::Line
                && s.stroke_width > 0.
                && !self.doc.all_fills().any(|(_, f)| f.segments.contains(&sid))
        })
    }
}

/// Snap a single drag target's direction to 45-degree steps around the
/// other endpoint of its owning segment (shift-resize). Standalone lines
/// get angle-only snapping — no inch-mark length quantization.
fn snap_target_direction(doc: &Document, targets: &mut [(PointId, Point2)]) {
    let (pid, target) = targets[0];
    let mut anchor: Option<Point2> = None;
    let mut bare_line = false;
    for (sid, s) in doc.all_segments() {
        let other = if s.start == pid {
            Some(s.end)
        } else if s.end == pid {
            Some(s.start)
        } else {
            None
        };
        if let Some(o) = other
            && let Some(pos) = doc.point(o)
        {
            anchor = Some(pos);
            bare_line = s.kind == crate::core::document::SegmentKind::Line
                && s.stroke_width > 0.
                && !doc.all_fills().any(|(_, f)| f.segments.contains(&sid));
            break;
        }
    }
    let Some(anchor) = anchor else { return };
    let dx = target.x - anchor.x;
    let dy = target.y - anchor.y;
    if dx == 0. && dy == 0. {
        return;
    }
    let (_, snapped_b) = tools::snap_direction(anchor, target);
        targets[0].1 = if bare_line {
            tools::snap_angle(anchor, target)
        } else {
            snapped_b
        };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_cursor_chases_instead_of_freezing() {
        // Never-drop drags: a cursor that sprints past feasibility must
        // move the point every frame (bounded chase), never hold-last-valid
        // until re-click.
        let mut ed = Editor::new();
        ed.snap_to_grid = false;
        ed.snap_to_objects = false;
        ed.alt_down = true; // skip snap proposals entirely
        let start = Point2::new(0., 0.);
        let b = ed.doc.add_point(start);
        ed.dragging = Some(DragState {
            points: vec![(b, start)],
            aux: Vec::new(),
            start_cursor: start,
            arc_body_scale: None,
            chase: 1.0,
        });
        ed.last_cursor = Some(gpui::point(gpui::px(1000.), gpui::px(0.)));
        assert!(ed.solve_drag(false));
        assert!(ed.dragging.is_some(), "grab survives the frame");
        let p = ed.doc.point(b).unwrap();
        let moved = (p.x - start.x).hypot(p.y - start.y);
        assert!(moved > 1.0, "point chased, moved={moved}");
        assert!(
            moved <= 150.0 + 1e-6,
            "chase bounded per frame, moved={moved}"
        );
    }

    #[test]
    fn rejected_frames_shrink_chase_and_hold() {
        // Genuinely stuck (conflicting locks): hold position, halve the
        // chase per rejected frame, never drop the grab.
        use crate::core::constraints::{DimMode, DimTarget, Dimension};
        let mut ed = Editor::new();
        ed.snap_to_grid = false;
        ed.snap_to_objects = false;
        ed.alt_down = true;
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(100., 0.));
        for value in [100., 200.] {
            ed.doc.dimensions.push(Dimension {
                target: DimTarget::Points { a, b, mode: DimMode::Aligned },
                value,
                offset: 0.,
                slide: 0.,
                sweep: 0.,
            });
        }
        ed.dragging = Some(DragState {
            points: vec![(b, Point2::new(100., 0.))],
            aux: Vec::new(),
            start_cursor: Point2::new(100., 0.),
            arc_body_scale: None,
            chase: 1.0,
        });
        ed.last_cursor = Some(gpui::point(gpui::px(500.), gpui::px(0.)));
        assert!(ed.solve_drag(false));
        assert!(ed.dragging.is_some(), "grab survives reject");
        let p = ed.doc.point(b).unwrap();
        assert!(
            (p.x - 100.).abs() < 1e-6 && p.y.abs() < 1e-6,
            "truly stuck holds position"
        );
        let chase = ed.dragging.as_ref().unwrap().chase;
        assert!((chase - 0.5).abs() < 1e-9, "chase halved, got={chase}");
        assert!(ed.solve_drag(false));
        let chase = ed.dragging.as_ref().unwrap().chase;
        assert!((chase - 0.25).abs() < 1e-9, "chase keeps decaying, got={chase}");
    }
}
