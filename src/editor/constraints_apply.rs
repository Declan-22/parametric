use crate::core::constraints::{ConstraintKind, DimTarget, ElementRef};
use crate::core::geometry::Point2;
use crate::core::ids::PointId;

use super::Editor;
use super::HANDLE_TOL_PX;
use super::Tool;
use super::equivalent_angle_vertex;
use super::overconstraint;
use super::pick;

// Constraint + dimension apply path: menu/tool creation, trial solves, exact passes.
impl Editor {
    pub fn apply_constraint_from_menu(&mut self, kind: ConstraintKind) -> bool {
        match kind {
            ConstraintKind::Horizontal | ConstraintKind::Vertical => {
                // Shared candidates (same function the menu gates on).
                let Some((pa, pb, sid)) =
                    Self::hv_candidates(&self.doc, &self.selection)
                else {
                    return false;
                };
                let (Some(a), Some(b)) = (self.doc.point(pa), self.doc.point(pb)) else {
                    return false;
                };
                // Infer purely from geometry: the dominant axis wins. (The
                // menu passes Horizontal as a selector, never a decision.)
                let k = if (a.y - b.y).abs() <= (a.x - b.x).abs() {
                    ConstraintKind::Horizontal
                } else {
                    ConstraintKind::Vertical
                };
                // Toggle: remove if present, else add.
                if let Some(pos) = self.doc.constraints.iter().position(|c| {
                    (c.kind == ConstraintKind::Horizontal
                        || c.kind == ConstraintKind::Vertical)
                        && ((c.a == pa && c.b == pb) || (c.a == pb && c.b == pa))
                }) {
                    self.doc.constraints.remove(pos);
                    self.doc_gen += 1;
                    return true;
                }
                self.history_begin();
                self.doc.add_constraint(k, pa, pb);
                let els = match sid {
                    Some(sid) => vec![ElementRef::Segment(sid)],
                    None => vec![ElementRef::Point(pa), ElementRef::Point(pb)],
                };
                let ok = self.solve_constraint_now(&els);
                if ok {
                    self.flush_pending_history();
                } else {
                    self.gesture_snapshot = None;
                    let (summary, hint) = overconstraint::hv_detail(
                        &self.doc,
                        pa,
                        pb,
                        k == ConstraintKind::Horizontal,
                    );
                    self.toast_requests.push(overconstraint::explain_constraint(
                        &self.doc, k, pa, pb, summary, hint,
                    ));
                }
                ok
            }
            ConstraintKind::Coincident => {
                // Shared candidates (same function the menu gates on).
                let Some((a, b)) =
                    Self::coincident_candidates(&self.doc, &self.selection)
                else {
                    return false;
                };
                self.history_begin();
                self.doc.add_constraint(ConstraintKind::Coincident, a, b);
                let ok = self.solve_constraint_now(&[ElementRef::Point(a), ElementRef::Point(b)]);
                if ok {
                    self.flush_pending_history();
                } else {
                    self.gesture_snapshot = None;
                    let (summary, hint) = overconstraint::coincident_detail(&self.doc, a, b);
                    self.toast_requests.push(overconstraint::explain_constraint(
                        &self.doc,
                        ConstraintKind::Coincident,
                        a,
                        b,
                        summary,
                        hint,
                    ));
                }
                ok
            }
            ConstraintKind::Tangent => {
                // Shared candidates (same function the menu gates on),
                // re-checked for lock feasibility: a shown row always has
                // valid inputs, and locked geometry can never reach here.
                let Some((line, other, contact, collinear)) =
                    Self::tangent_candidate(&self.doc, &self.selection)
                else {
                    return false;
                };
                if !Self::tangent_feasible(&self.doc, line, other, contact) {
                    return false;
                }
                self.history_begin();
                if collinear {
                    // Line + line collinearity, decomposed into Parallel +
                    // a Coincident joint (parallel lines sharing an
                    // endpoint are one line).
                    let (a, b) = (line, other);
                    let (Some(sa), Some(sb)) =
                        (self.doc.segment(a), self.doc.segment(b))
                    else {
                        self.gesture_snapshot = None;
                        return false;
                    };
                    // Nearest cross endpoint pair becomes the joint.
                    let mut best: Option<(f64, PointId, PointId)> = None;
                    for lid in [sa.start, sa.end] {
                        let Some(lp) = self.doc.point(lid) else {
                            continue;
                        };
                        for cid in [sb.start, sb.end] {
                            let Some(cp) = self.doc.point(cid) else {
                                continue;
                            };
                            let d = pick::distance(lp, cp);
                            if best.map_or(true, |(bd, _, _)| d < bd) {
                                best = Some((d, lid, cid));
                            }
                        }
                    }
                    let Some((_, ja, jb)) = best else {
                        self.gesture_snapshot = None;
                        return false;
                    };
                    self.make_line_parallel(b, a);
                    self.doc.add_parallel_constraint(a, b);
                    if ja != jb {
                        self.doc.add_constraint(ConstraintKind::Coincident, ja, jb);
                    }
                    let ok = self.solve_constraint_now(&[
                        ElementRef::Segment(a),
                        ElementRef::Segment(b),
                    ]);
                    if ok {
                        self.flush_pending_history();
                    } else {
                        self.gesture_snapshot = None;
                        let (summary, hint) =
                            overconstraint::line_pair_detail(&self.doc, a, b, true);
                        if let Some(sa) = self.doc.segment(a) {
                            self.toast_requests.push(overconstraint::explain_constraint(
                                &self.doc,
                                ConstraintKind::Tangent,
                                sa.start,
                                sa.end,
                                summary,
                                hint,
                            ));
                        }
                    }
                    return ok;
                }
                self.doc.add_tangent_constraint(line, other, contact);
                let ok = self.solve_constraint_now(&[
                    ElementRef::Segment(line),
                    ElementRef::Segment(other),
                ]);
                if ok {
                    self.flush_pending_history();
                } else {
                    self.gesture_snapshot = None;
                    if let Some(lseg) = self.doc.segment(line) {
                        self.toast_requests.push(overconstraint::explain_constraint(
                            &self.doc,
                            ConstraintKind::Tangent,
                            lseg.start,
                            contact,
                            "The line can't swing tangent without breaking a lock.".to_string(),
                            "Relax the H/V lock or dimension first.".to_string(),
                        ));
                    }
                }
                ok
            }
            ConstraintKind::Parallel | ConstraintKind::Perpendicular => {
                // Shared candidates + lock feasibility (same as the menu).
                let Some((a, b)) = Self::line_pair(&self.doc, &self.selection) else {
                    return false;
                };
                if !Self::line_pair_feasible(
                    &self.doc,
                    a,
                    b,
                    kind == ConstraintKind::Parallel,
                ) {
                    return false;
                }
                self.history_begin();
                if kind == ConstraintKind::Parallel {
                    self.make_line_parallel(b, a);
                    self.doc.add_parallel_constraint(a, b);
                } else {
                    self.doc.add_perpendicular_constraint(a, b);
                }
                let ok =
                    self.solve_constraint_now(&[ElementRef::Segment(a), ElementRef::Segment(b)]);
                if ok {
                    self.flush_pending_history();
                } else {
                    self.gesture_snapshot = None;
                    let (summary, hint) = overconstraint::line_pair_detail(
                        &self.doc,
                        a,
                        b,
                        kind == ConstraintKind::Parallel,
                    );
                    if let Some(sa) = self.doc.segment(a) {
                        self.toast_requests.push(overconstraint::explain_constraint(
                            &self.doc,
                            kind,
                            sa.start,
                            sa.end,
                            summary,
                            hint,
                        ));
                    }
                }
                ok
            }
        }
    }

    pub(crate) fn try_apply_dimension(&mut self, dim: crate::core::constraints::Dimension) -> bool {
        let value = dim.value;
        let mut trial = self.doc.clone();
        trial.dimensions.push(dim);
        self.solve_and_apply(trial, dim.target, value)
    }

    /// Re-solves after an EDITED dimension value on the stored dimension
    /// `idx` (already mutated in place).
    pub(crate) fn reapply_dimension(&mut self, idx: usize) -> bool {
        let trial = self.doc.clone();
        let target = trial.dimensions[idx].target;
        let value = trial.dimensions[idx].value;
        self.solve_and_apply(trial, target, value)
    }

    /// Runs the trial solve for `trial` (which already carries the dimension
    /// under test). On success the solved document replaces the live one;
    /// on failure an explanatory toast is queued and nothing changes.
    fn solve_and_apply(
        &mut self,
        mut trial: crate::core::document::Document,
        target: crate::core::constraints::DimTarget,
        attempted: f64,
    ) -> bool {
        // Free set: everything transitively connected to the dimension's
        // geometry through segments and constraints — soft-anchored at
        // their current positions so the deformation is minimal.
        let mut seeds: Vec<PointId> = match target {
            crate::core::constraints::DimTarget::Points { a, b, .. } => vec![a, b],
            crate::core::constraints::DimTarget::PointLine { p, line } => {
                let mut v = vec![p];
                if let Some(seg) = trial.segment(line) {
                    v.push(seg.start);
                    v.push(seg.end);
                }
                v
            }
            crate::core::constraints::DimTarget::Lines { a, b }
            | crate::core::constraints::DimTarget::Angle { a, b }
            | crate::core::constraints::DimTarget::EdgeMid { a, b, .. } => {
                let mut v = Vec::new();
                for sid in [a, b] {
                    if let Some(seg) = trial.segment(sid) {
                        v.push(seg.start);
                        v.push(seg.end);
                    }
                }
                v
            }
            crate::core::constraints::DimTarget::Radius { seg }
            | crate::core::constraints::DimTarget::CurveLength { seg } => {
                let mut v = Vec::new();
                if let Some(seg) = trial.segment(seg) {
                    v.push(seg.start);
                    v.push(seg.end);
                    if let Some(c) = seg.ctrl {
                        v.push(c);
                    }
                    if let Some(c) = seg.center {
                        v.push(c);
                    }
                }
                v
            }
        };
        // Transitive closure over segments + constraint pairs.
        let mut i = 0;
        while i < seeds.len() {
            let pid = seeds[i];
            for (_, s) in trial.all_segments() {
                let other = if s.start == pid {
                    Some(s.end)
                } else if s.end == pid {
                    Some(s.start)
                } else {
                    None
                };
                if let Some(o) = other && !seeds.contains(&o) {
                    seeds.push(o);
                }
            }
            for c in &trial.constraints {
                let other = if c.a == pid {
                    Some(c.b)
                } else if c.b == pid {
                    Some(c.a)
                } else {
                    None
                };
                if let Some(o) = other && !seeds.contains(&o) {
                    seeds.push(o);
                }
            }
            i += 1;
        }
        let aux: Vec<(PointId, Point2)> = seeds
            .iter()
            .filter_map(|&pid| trial.point(pid).map(|p| (pid, p)))
            .collect();
        // TOP-LEFT ANCHOR: the reshape pivots around the component's
        // topmost-then-leftmost point, which stays HARD-FIXED. A rectangle
        // height edit (100 -> 50) keeps the top edge where it is and pulls
        // the bottom edge up the full 50, instead of both edges converging
        // 25 apiece. "Everything shifts toward the top-left."
        // ANGLE PIVOT: for connected edges, pin their shared vertex rather
        // than an unrelated top-left point. The angle should rotate/deform
        // around its actual corner; pinning elsewhere can force the solver
        // into a poor branch and report a valid 90-degree lock as infeasible.
        let angle_vertex = match target {
            crate::core::constraints::DimTarget::Angle { a, b } => {
                let first = trial.segment(a);
                let second = trial.segment(b);
                first.and_then(|first| {
                    second.and_then(|second| {
                        [first.start, first.end]
                            .into_iter()
                            .find(|&pid| equivalent_angle_vertex(&trial, pid, second.start)
                                || equivalent_angle_vertex(&trial, pid, second.end))
                    })
                })
            }
            _ => None,
        };
        let pins: Vec<(PointId, Point2)> = angle_vertex
            .and_then(|pid| trial.point(pid).map(|point| (pid, point)))
            .map(|pin| vec![pin])
            .unwrap_or_else(|| {
                aux.iter()
                    .copied()
                    .min_by(|a, b| {
                        (a.1.y, a.1.x)
                            .partial_cmp(&(b.1.y, b.1.x))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .into_iter()
                    .collect()
            });
        // STRONG-but-soft anchors: the freed component deforms minimally.
        // The weight ratio vs DIM_WEIGHT (1e6) sets the equilibrium
        // residual: sqrt(2/1e6) * displacement — tiny at 2.0, but ~10% of
        // displacement at 1e4, which false-triggered the over-constrained
        // modal on ordinary rectangle dimensions.
        let solver =
            crate::core::solver::Solver::build_pinned(&trial, &[], &aux, &pins, 2.0);
        let solution = solver.solve();
        let direct_distance = matches!(target,
            crate::core::constraints::DimTarget::Points { mode: crate::core::constraints::DimMode::Aligned, .. });
        if !solution.is_valid()
            || solution.max_angle_residual > 1e-5
            || (!direct_distance && solution.max_lin_residual > 1e-3)
        {
            self.toast_requests
                .push(overconstraint::explain_dimension(&self.doc, target, attempted));
            return false;
        }
        self.history_begin();
        self.doc = trial;
        let mut moved: std::collections::HashSet<PointId> = std::collections::HashSet::new();
        for (id, pos) in solution.positions {
            moved.insert(id);
            self.doc.move_point(id, pos);
        }
        // Arc consistency is part of the solve graph now.
        if direct_distance {
            self.enforce_point_distance_exact(target);
        }
        // The numerical radius equation is intentionally backed by an exact
        // geometric pass.  A circumradius has a very shallow gradient near a
        // semicircle, so least-squares can leave a visible 0.1px residue and
        // let the stored center drift.  Scale the defining points about the
        // arc center once the solve has chosen the deformation, then write
        // the center back to the exact circumcenter.
        if let crate::core::constraints::DimTarget::Radius { seg } = target {
            self.enforce_arc_radius_exact(seg);
            // The exact radius pass intentionally runs after the numerical
            // solve. Re-project tangent followers after it so changing an
            // arc radius cannot leave its connected line off the tangent.
            self.enforce_tangencies();
        }
        if let crate::core::constraints::DimTarget::CurveLength { seg } = target {
            self.enforce_curve_length_exact(seg);
        }
        self.flush_pending_history();
        true
    }

    pub(crate) fn is_constraint_tool(&self) -> bool {
        matches!(
            self.tool,
            Tool::ConstraintHorizontalVertical
                | Tool::ConstraintTangent
                | Tool::ConstraintCoincident
                | Tool::ConstraintParallel
                | Tool::ConstraintPerpendicular
        )
    }

    pub(crate) fn constraint_tool_click(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        let p = self.cursor_doc(cursor);
        let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        // At a shared endpoint the generic picker intentionally prefers a
        // point. Constraint tools need the underlying edge when the cursor
        // is on that edge, otherwise a line endpoint can hide the arc (or
        // vice versa) and tangent receives the same element twice.
        let hit = if matches!(self.tool, Tool::ConstraintTangent | Tool::ConstraintHorizontalVertical | Tool::ConstraintPerpendicular) {
            let edge = picker.segment(p).map(ElementRef::Segment);
            if edge.as_ref().is_some_and(|element| self.constraint_picks.contains(element)) {
                picker.element(p)
            } else {
                edge.or_else(|| picker.element(p))
            }
        } else {
            picker.element(p)
        }.and_then(|hit| self.normalize_constraint_hit(hit));
        let Some(hit) = hit else { return true };
        if !self.constraint_picks.contains(&hit) {
            self.constraint_picks.push(hit);
            self.selection = self.constraint_picks.clone();
            if self.tool == Tool::ConstraintCoincident {
                if let Some(point) = self.constraint_point_near(hit, p) {
                    self.constraint_point_picks.push(point);
                }
            }
        }

        match self.tool {
            Tool::ConstraintHorizontalVertical => {
                if let ElementRef::Segment(sid) = hit
                    && let Some(seg) = self.doc.segment(sid)
                    && seg.kind == crate::core::document::SegmentKind::Line
                {
                    let (Some(a), Some(b)) = (self.doc.point(seg.start), self.doc.point(seg.end)) else { return true };
                    let kind = if (a.y - b.y).abs() <= (a.x - b.x).abs() {
                        ConstraintKind::Horizontal
                    } else {
                        ConstraintKind::Vertical
                    };
                    self.doc.add_constraint(kind, seg.start, seg.end);
                    self.solve_constraint_now(&[ElementRef::Segment(sid)]);
                    self.constraint_picks.clear();
                    self.selection = vec![ElementRef::Segment(sid)];
                }
            }
            Tool::ConstraintCoincident => {
                if self.constraint_picks.len() == 2
                    && self.constraint_point_picks.len() >= 1
                    && let ElementRef::Segment(sid) = hit
                    && let Some(seg) = self.doc.segment(sid)
                    && seg.kind == crate::core::document::SegmentKind::Line
                {
                    let selected = self.constraint_point_picks[0];
                    let Some((a, b)) = self.doc.segment_geom(sid) else { return true };
                    let dx = b.x - a.x;
                    let dy = b.y - a.y;
                    let denom = dx * dx + dy * dy;
                    let t = if denom > 1e-12 {
                        ((p.x - a.x) * dx + (p.y - a.y) * dy) / denom
                    } else { 0. };
                    let edge_point = Point2::new(a.x + dx * t.clamp(0., 1.), a.y + dy * t.clamp(0., 1.));
                    let created = self.doc.add_point(edge_point);
                    if let Some(layer) = self.doc.layers.iter_mut().find(|l| l.elements.contains(&ElementRef::Segment(sid))) {
                        layer.elements.push(ElementRef::Point(created));
                    }
                    self.doc.add_point_on_segment_constraint(selected, created, sid);
                    self.solve_constraint_now(&[
                        ElementRef::Point(selected), ElementRef::Point(created), ElementRef::Segment(sid),
                    ]);
                    self.selection.push(ElementRef::Point(created));
                    self.constraint_picks.clear();
                    self.constraint_point_picks.clear();
                    self.update_dim_geom();
                    return true;
                }
                if self.constraint_picks.len() >= 2 {
                    if let [a, b] = self.constraint_point_picks.as_slice() {
                        self.doc.add_constraint(ConstraintKind::Coincident, *a, *b);
                        self.solve_constraint_now(&[ElementRef::Point(*a), ElementRef::Point(*b)]);
                    }
                    self.selection = self.constraint_picks.clone();
                    self.constraint_picks.clear();
                    self.constraint_point_picks.clear();
                }
            }
            Tool::ConstraintTangent => {
                let line = self.constraint_picks.iter().find_map(|e| e.as_segment()).and_then(|sid| {
                    self.doc.segment(sid).filter(|s| s.kind == crate::core::document::SegmentKind::Line).map(|_| sid)
                });
                let curve = self.constraint_picks.iter().find_map(|e| e.as_segment()).and_then(|sid| {
                    self.doc.segment(sid).filter(|s| matches!(s.kind, crate::core::document::SegmentKind::Arc | crate::core::document::SegmentKind::Bezier)).map(|_| sid)
                });
                // Curve-to-curve G1 is also a valid tangent relation. The
                // menu path and solver use the same candidate, so selecting
                // two Bezier spans no longer silently produces no action.
                if line.is_none() && self.constraint_picks.len() == 2 {
                    if let Some((first, second, contact, _)) = Self::tangent_candidate(&self.doc, &self.constraint_picks) {
                        self.doc.add_tangent_constraint(first, second, contact);
                        self.solve_constraint_now(&[ElementRef::Segment(first), ElementRef::Segment(second)]);
                        self.selection = self.constraint_picks.clone();
                        self.constraint_picks.clear();
                        self.update_dim_geom();
                        return true;
                    }
                }
                if let (Some(line), Some(curve)) = (line, curve) {
                    let point = self.tangent_contact_point(line, curve, p);
                    if let Some((point, contact)) = point {
                        // Put the selected line endpoint exactly on the
                        // selected arc contact, then place the other endpoint
                        // along the exact local tangent. Moving only one end
                        // leaves the stored tangent equation referring to a
                        // point that is not on the line, so the constraint
                        // appears to do nothing on the next solve.
                        if let (Some(ls), Some(le)) = (
                            self.doc.segment(line).map(|s| s.start),
                            self.doc.segment(line).map(|s| s.end),
                        ) {
                            if let (Some(center), Some(lp), Some(rp)) = (
                                self.doc.segment(curve).and_then(|s| s.center).and_then(|id| self.doc.point(id)),
                                self.doc.point(point), self.doc.point(if point == ls { le } else { ls }),
                            ) {
                                let radius = Point2::new(contact.x - center.x, contact.y - center.y);
                                let rl = (radius.x * radius.x + radius.y * radius.y).sqrt().max(1e-9);
                                let mut tangent = Point2::new(-radius.y / rl, radius.x / rl);
                                let old = Point2::new(rp.x - lp.x, rp.y - lp.y);
                                if tangent.x * old.x + tangent.y * old.y < 0. {
                                    tangent = Point2::new(-tangent.x, -tangent.y);
                                }
                                let length = (old.x * old.x + old.y * old.y).sqrt().max(1e-9);
                                self.doc.move_point(point, contact);
                                self.doc.move_point(if point == ls { le } else { ls }, Point2::new(
                                    contact.x + tangent.x * length,
                                    contact.y + tangent.y * length,
                                ));
                            } else {
                                self.doc.move_point(point, contact);
                            }
                        } else {
                            self.doc.move_point(point, contact);
                        }
                        self.doc.add_tangent_constraint(line, curve, point);
                        self.solve_constraint_now(&[ElementRef::Segment(line), ElementRef::Segment(curve)]);
                    }
                    self.selection = self.constraint_picks.clone();
                    self.constraint_picks.clear();
                }
            }
            Tool::ConstraintParallel | Tool::ConstraintPerpendicular => {
                if let [ElementRef::Segment(first), ElementRef::Segment(second)] = self.constraint_picks.as_slice()
                    && let (Some(a), Some(b)) = (self.doc.segment(*first), self.doc.segment(*second))
                {
                    let fixed = [b.start, b.end].iter().all(|&pid| {
                        self.doc.constraints.iter().filter(|c| c.a == pid || c.b == pid).count() >= 2
                            || self.doc.dimensions.iter().any(|d| match d.target {
                                DimTarget::Points { a, b, .. } => a == pid || b == pid,
                                DimTarget::PointLine { p, .. } => p == pid,
                                _ => false,
                            })
                    });
                    let (foundation, moving) = if fixed { (*second, *first) } else { (*first, *second) };
                    self.make_line_parallel(moving, foundation);
                    if self.tool == Tool::ConstraintParallel {
                        self.doc.add_parallel_constraint(foundation, moving);
                    } else {
                        self.doc.add_perpendicular_constraint(foundation, moving);
                    }
                    self.solve_constraint_now(&[ElementRef::Segment(foundation), ElementRef::Segment(moving)]);
                    self.selection = self.constraint_picks.clone();
                    self.constraint_picks.clear();
                }
            }
            _ => {}
        }
        self.update_dim_geom();
        true
    }

    pub(crate) fn solve_constraint_now(&mut self, elements: &[ElementRef]) -> bool {
        let mut ids = Vec::new();
        for &element in elements {
            for id in self.doc.element_points(element) {
                if !ids.contains(&id) { ids.push(id); }
            }
        }
        // Constraint creation must project the complete connected component,
        // not just the two endpoints that were clicked. Otherwise a new
        // horizontal/vertical/parallel relation on a chained drawing has no
        // movable path and is discarded as if it were over-constrained.
        let mut cursor = 0;
        while cursor < ids.len() {
            let pid = ids[cursor];
            for (_, seg) in self.doc.all_segments() {
                if seg.start != pid && seg.end != pid && seg.ctrl != Some(pid) && seg.center != Some(pid) {
                    continue;
                }
                for linked in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center].into_iter().flatten() {
                    if self.doc.point(linked).is_some() && !ids.contains(&linked) {
                        ids.push(linked);
                    }
                }
            }
            for constraint in &self.doc.constraints {
                let mut linked = constraint.a == pid || constraint.b == pid;
                if let Some(segment_id) = constraint.point_on_segment
                    && let Some(seg) = self.doc.segment(segment_id)
                {
                    linked |= seg.start == pid || seg.end == pid || seg.ctrl == Some(pid) || seg.center == Some(pid);
                    if linked {
                        for point in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center].into_iter().flatten() {
                            if self.doc.point(point).is_some() && !ids.contains(&point) {
                                ids.push(point);
                            }
                        }
                    }
                }
                if linked {
                    for point in [constraint.a, constraint.b] {
                        if self.doc.point(point).is_some() && !ids.contains(&point) {
                            ids.push(point);
                        }
                    }
                }
            }
            cursor += 1;
        }
        let aux: Vec<_> = ids
            .iter()
            .filter_map(|&id| self.doc.point(id).map(|p| (id, p)))
            .collect();
        let solver = crate::core::solver::Solver::build_with_anchor(
            &self.doc,
            &[],
            &aux,
            1.0,
        );
        let solution = solver.solve();
        if solution.constraints_satisfied() {
            for (id, pos) in solution.positions {
                self.doc.move_point(id, pos);
            }
            self.enforce_tangencies();
            self.doc_gen += 1;
            true
        } else {
            // Constraint creation is transactional. The caller has just
            // appended the candidate relation; if the complete component
            // cannot satisfy it, remove that candidate instead of leaving a
            // latent broken constraint in the document.
            self.doc.constraints.pop();
            false
        }
    }

    fn make_line_parallel(&mut self, moving: crate::core::ids::SegmentId, foundation: crate::core::ids::SegmentId) {
        let (Some(m), Some(f)) = (self.doc.segment(moving), self.doc.segment(foundation)) else { return };
        let (Some(ma), Some(mb), Some(fa), Some(fb)) = (self.doc.point(m.start), self.doc.point(m.end), self.doc.point(f.start), self.doc.point(f.end)) else { return };
        let (dx, dy) = (fb.x - fa.x, fb.y - fa.y);
        let fl = (dx * dx + dy * dy).sqrt();
        let ml = ((mb.x - ma.x).powi(2) + (mb.y - ma.y).powi(2)).sqrt();
        if fl < 1e-9 || ml < 1e-9 { return; }
        let mut ux = dx / fl;
        let mut uy = dy / fl;
        if ux * (mb.x - ma.x) + uy * (mb.y - ma.y) < 0. { ux = -ux; uy = -uy; }
        let mid = Point2::new((ma.x + mb.x) / 2., (ma.y + mb.y) / 2.);
        self.doc.move_point(m.start, Point2::new(mid.x - ux * ml / 2., mid.y - uy * ml / 2.));
        self.doc.move_point(m.end, Point2::new(mid.x + ux * ml / 2., mid.y + uy * ml / 2.));
    }

    fn constraint_point_near(&self, el: ElementRef, near: Point2) -> Option<PointId> {
        let points = self.doc.element_points(el);
        points.into_iter().min_by(|a, b| {
            let da = self.doc.point(*a).map(|p| pick::distance(p, near)).unwrap_or(f64::MAX);
            let db = self.doc.point(*b).map(|p| pick::distance(p, near)).unwrap_or(f64::MAX);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    fn tangent_contact_point(&self, line: crate::core::ids::SegmentId, arc: crate::core::ids::SegmentId, cursor: Point2) -> Option<(PointId, Point2)> {
        let l = self.doc.segment(line)?;
        let a = self.doc.segment(arc)?;
        if a.kind == crate::core::document::SegmentKind::Bezier {
            let curve_ends = [a.start, a.end];
            let line_point = [l.start, l.end].into_iter().min_by(|x, y| {
                let dx = |id: PointId| self.doc.point(id).map(|p| pick::distance(p, cursor)).unwrap_or(f64::MAX);
                dx(*x).partial_cmp(&dx(*y)).unwrap_or(std::cmp::Ordering::Equal)
            })?;
            let lp = self.doc.point(line_point)?;
            let curve_point = curve_ends.into_iter().min_by(|x, y| {
                let dx = |id: PointId| self.doc.point(id).map(|p| pick::distance(p, lp)).unwrap_or(f64::MAX);
                dx(*x).partial_cmp(&dx(*y)).unwrap_or(std::cmp::Ordering::Equal)
            })?;
            return Some((line_point, self.doc.point(curve_point)?));
        }
        let (Some(sa), Some(sb), Some(ctrl)) = (
            self.doc.point(a.start), self.doc.point(a.end), a.ctrl.and_then(|id| self.doc.point(id))
        ) else { return None };
        let (center, radius) = crate::editor::arc::circumcircle(sa, sb, ctrl)?;
        let dx = cursor.x - center.x;
        let dy = cursor.y - center.y;
        let length = (dx * dx + dy * dy).sqrt().max(1e-9);
        let contact = Point2::new(center.x + dx * radius / length, center.y + dy * radius / length);
        let point = [l.start, l.end].into_iter().min_by(|x, y| {
            let dx = |id: PointId| self.doc.point(id).map(|p| pick::distance(p, contact)).unwrap_or(f64::MAX);
            dx(*x).partial_cmp(&dx(*y)).unwrap_or(std::cmp::Ordering::Equal)
        })?;
        Some((point, contact))
    }

    fn enforce_point_distance_exact(&mut self, target: crate::core::constraints::DimTarget) {
        let crate::core::constraints::DimTarget::Points { a, b, .. } = target else { return; };
        let (Some(pa), Some(pb)) = (self.doc.point(a), self.doc.point(b)) else { return; };
        let dx = pb.x - pa.x; let dy = pb.y - pa.y;
        let len = (dx * dx + dy * dy).sqrt();
        let (ux, uy) = if len > 1e-9 { (dx / len, dy / len) } else { (1.0, 0.0) };
        let value = self.doc.dimensions.iter().rev().find_map(|d| match d.target {
            crate::core::constraints::DimTarget::Points { a: da, b: db, mode: crate::core::constraints::DimMode::Aligned }
                if da == a && db == b => Some(d.value.abs()),
            _ => None,
        }).unwrap_or(len);
        self.doc.move_point(b, crate::core::geometry::Point2::new(pa.x + ux * value, pa.y + uy * value));
    }

    fn enforce_arc_radius_exact(&mut self, sid: crate::core::ids::SegmentId) {
        let Some(seg) = self.doc.segment(sid) else { return };
        let (Some(aid), Some(bid), Some(cid), Some(oid)) =
            (Some(seg.start), Some(seg.end), seg.ctrl, seg.center) else { return };
        let (Some(a), Some(b), Some(c), Some(center)) =
            (self.doc.point(aid), self.doc.point(bid), self.doc.point(cid), self.doc.point(oid))
        else { return };
        let Some(dim) = self.doc.dimensions.iter().find(|d|
            matches!(d.target, crate::core::constraints::DimTarget::Radius { seg: s } if s == sid))
        else { return };
        let Some((circumcenter, radius)) = crate::editor::arc::circumcircle(a, b, c) else { return };
        if radius <= 1e-9 || dim.value <= 0. { return; }
        let scale = dim.value / radius;
        for (id, p) in [(aid, a), (bid, b), (cid, c)] {
            self.doc.move_point(id, Point2::new(
                circumcenter.x + (p.x - circumcenter.x) * scale,
                circumcenter.y + (p.y - circumcenter.y) * scale,
            ));
        }
        self.doc.move_point(oid, circumcenter);
        // Recompute from the scaled points so the center is exact even when
        // the original solver residual was large.
        if let (Some(a), Some(b), Some(c)) =
            (self.doc.point(aid), self.doc.point(bid), self.doc.point(cid))
            && let Some((o, _)) = crate::editor::arc::circumcircle(a, b, c)
        {
            self.doc.move_point(oid, o);
        }
        let _ = center;
    }

    /// Exact arc-length enforcement for CurveLength dims (BEZIER ONLY):
    /// uniform scale of end + handles about the start point so the curve's
    /// total length matches the placed value (handles ride proportionally,
    /// preserving shape).
    fn enforce_curve_length_exact(&mut self, sid: crate::core::ids::SegmentId) {
        let Some(seg) = self.doc.segment(sid) else { return };
        if seg.kind != crate::core::document::SegmentKind::Bezier {
            return;
        }
        let Some(dim) = self.doc.dimensions.iter().find(|d| {
            matches!(d.target, crate::core::constraints::DimTarget::CurveLength { seg: s } if s == sid)
        }).copied() else { return };
        if dim.value <= 1e-9 {
            return;
        }
        let (h1, h2) = seg.bezier_handles();
        let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
            self.doc.point(seg.start),
            h1.and_then(|id| self.doc.point(id)),
            h2.and_then(|id| self.doc.point(id)),
            self.doc.point(seg.end),
        ) else {
            return;
        };
        let cur = crate::editor::bezier::arc_length(p0, c1, c2, p1);
        if cur < 1e-9 {
            return;
        }
        let s = dim.value.abs() / cur;
        if !s.is_finite() || (s - 1.).abs() < 1e-9 {
            return;
        }
        for id in [seg.end, h1.unwrap_or(seg.end), h2.unwrap_or(seg.end)] {
            if let Some(p) = self.doc.point(id) {
                self.doc.move_point(
                    id,
                    Point2::new(p0.x + (p.x - p0.x) * s, p0.y + (p.y - p0.y) * s),
                );
            }
        }
    }

    /// Drag-commit repair for curve-length dims: live drags solve with the
    /// length equation stripped, so re-establish violated ones exactly here.
    /// Satisfied dims are untouched. Runs before history promotion so the
    /// repair joins the gesture's undo step.
    pub(crate) fn enforce_dragged_curve_lengths(&mut self) {
        let segs: Vec<crate::core::ids::SegmentId> = self
            .doc
            .dimensions
            .iter()
            .filter_map(|d| match d.target {
                crate::core::constraints::DimTarget::CurveLength { seg } => Some(seg),
                _ => None,
            })
            .collect();
        for seg in segs {
            let Some(d) = self.doc.segment(seg) else { continue };
            let (h1, h2) = d.bezier_handles();
            let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                self.doc.point(d.start),
                h1.and_then(|id| self.doc.point(id)),
                h2.and_then(|id| self.doc.point(id)),
                self.doc.point(d.end),
            ) else {
                continue;
            };
            let value = self
                .doc
                .dimensions
                .iter()
                .find(|x| {
                    matches!(x.target, crate::core::constraints::DimTarget::CurveLength { seg: s } if s == seg)
                })
                .map(|x| x.value.abs())
                .unwrap_or(0.);
            if value <= 1e-9 {
                continue;
            }
            let cur = crate::editor::bezier::arc_length(p0, c1, c2, p1);
            if (cur - value).abs() > 1e-3 {
                self.enforce_curve_length_exact(seg);
            }
        }
    }

    /// Projects the free end of each tangent line onto the exact tangent at
    /// the stored arc contact. This keeps the relationship exact after
    /// radius edits and ordinary point drags without asking the solver to
    /// optimize a poorly conditioned circle/line equation.
    pub(crate) fn enforce_tangencies(&mut self) {
        let constraints = self.doc.constraints.clone();
        for c in constraints {
            if c.kind != ConstraintKind::Tangent { continue; }
            let (line_id, arc_id) = c.tangent_segments.unwrap_or_else(|| {
                let mut line = None;
                let mut arc = None;
                for (sid, s) in self.doc.all_segments() {
                    if s.start != c.a && s.end != c.a { continue; }
                    if s.kind == crate::core::document::SegmentKind::Line { line = Some(sid); }
                    if s.kind == crate::core::document::SegmentKind::Arc { arc = Some(sid); }
                }
                (line.unwrap_or(crate::core::ids::SegmentId { idx: u32::MAX, generation: 0 }),
                 arc.unwrap_or(crate::core::ids::SegmentId { idx: u32::MAX, generation: 0 }))
            });
            let (Some(line), Some(arc)) = (self.doc.segment(line_id), self.doc.segment(arc_id)) else { continue; };
            // Fillet-adjacent tangency is owned end-to-end by the solver
            // equations plus refresh_fillets: rotating the line here would
            // fight H/V locks and read as slant on constrained sketches.
            // (Either side being fillet-linked is enough to skip.)
            if self.doc.modifiers.iter().any(|m| {
                m.arc == Some(arc_id) || m.first == line_id || m.second == line_id
            }) {
                continue;
            }
            let (Some(a), Some(b), Some(ctrl)) = (
                self.doc.point(arc.start), self.doc.point(arc.end),
                arc.ctrl.and_then(|id| self.doc.point(id))) else { continue; };
            let Some((o, _)) = crate::editor::arc::circumcircle(a, b, ctrl) else { continue; };
            let contact = if line.start == c.a { line.start } else if line.end == c.a { line.end } else { continue };
            let Some(cp) = self.doc.point(contact) else { continue; };
            let (other, Some(op)) = (if line.start == contact {
                (line.end, self.doc.point(line.end))
            } else { (line.start, self.doc.point(line.start)) }) else { continue };
            let rx = cp.x - o.x; let ry = cp.y - o.y;
            let rl = (rx * rx + ry * ry).sqrt();
            let ll = ((op.x - cp.x).powi(2) + (op.y - cp.y).powi(2)).sqrt();
            if rl < 1e-9 || ll < 1e-9 { continue; }
            let tx = -ry / rl; let ty = rx / rl;
            let sign = ((op.x - cp.x) * tx + (op.y - cp.y) * ty).signum();
            self.doc.move_point(other, Point2::new(cp.x + tx * sign * ll, cp.y + ty * sign * ll));
        }
    }
}
