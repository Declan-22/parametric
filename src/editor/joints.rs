use std::collections::{HashMap, HashSet};

use crate::core::constraints::{DimTarget, ElementRef};
use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::PointId;

// Clay joints (Phase 1): per-point shape contract for bezier spans.
// Sidecar lives editor-side; core entities and the solver never see it.
// PRESENCE in the map = managed (derived handles). Absence = legacy
// (user/solver-owned handles, derivation skips the span unless the OTHER
// endpoint is managed). Flipping a grade therefore IS the derive-on-touch
// migration: no separate touched-set, no flag-day.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Grade {
    /// Corner: adjacent spans aim at their neighbors independently.
    G0,
    /// Smooth: tangent derives from the neighbor chord, sides collinear.
    #[default]
    G1,
    /// Curvature-matched: arm lengths solved for curvature continuity.
    G2,
}

#[derive(Clone, Copy, Debug)]
pub struct JointData {
    pub grade: Grade,
    /// Arm-length scales (1.0 = thirds). 0 collapses the arm: needle cusp.
    pub full_in: f32,
    pub full_out: f32,
    /// Tangent rotation in radians (0 = automatic). G1 mirrors: the exit
    /// offset wins, else the entry offset rotates both sides equally.
    pub ang_in: f32,
    pub ang_out: f32,
}

impl Default for JointData {
    fn default() -> Self {
        Self {
            grade: Grade::G1,
            full_in: 1.0,
            full_out: 1.0,
            ang_in: 0.0,
            ang_out: 0.0,
        }
    }
}

/// View-state bundle for the canvas: what Clay may render this frame.
/// None (e.g. home thumbnails) = committed truth only, zero components.
#[derive(Clone, Copy)]
pub struct ClayView<'a> {
    pub edit: bool,
    pub joints: &'a HashMap<PointId, JointData>,
    pub combs: bool,
    pub comb_scale: f32,
    pub comb_density: usize,
    /// Active Clay path joints (all get the half-selected frame) + the
    /// active end (filled selected). None when no path is active.
    pub path_joints: Option<&'a [PointId]>,
    pub path_end: Option<PointId>,
    /// Edit isolation scope (None = everything). Out-of-scope strokes
    /// paint at 50%; points/dots stay full-bright (affordances).
    pub scope: Option<&'a crate::editor::EditScope>,
}

impl crate::editor::Editor {
    pub fn joint_or_default(&self, id: PointId) -> JointData {
        self.joint_data.get(&id).copied().unwrap_or_default()
    }

    pub fn is_managed(&self, id: PointId) -> bool {
        self.joint_data.contains_key(&id)
    }

    /// Insert-or-modify: keeps fullness/angles, flips the grade. Creating
    /// the entry enrolls the point (derive-on-touch).
    pub fn set_grade(&mut self, id: PointId, grade: Grade) {
        self.joint_data
            .entry(id)
            .and_modify(|j| j.grade = grade)
            .or_insert(JointData {
                grade,
                ..Default::default()
            });
    }

    pub fn joint_data_map(&self) -> &HashMap<PointId, JointData> {
        &self.joint_data
    }

    /// Handles of managed spans: derived positions, not components. Never
    /// pickable/grabbable/selectable in Edit (clicks pass through).
    pub fn managed_handles(&self) -> HashSet<PointId> {
        let mut out = HashSet::new();
        for (_, s) in self.doc.all_segments() {
            if s.kind != SegmentKind::Bezier {
                continue;
            }
            if self.joint_data.contains_key(&s.start)
                || self.joint_data.contains_key(&s.end)
            {
                out.extend(s.ctrl);
                out.extend(s.center);
            }
        }
        out
    }

    pub fn is_bezier_endpoint(&self, pid: PointId) -> bool {
        self.doc.all_segments().any(|(_, s)| {
            s.kind == SegmentKind::Bezier && (s.start == pid || s.end == pid)
        })
    }

    /// Edit-mode pick guard: managed handles don't exist as components.
    /// None = treat as no-hit (marquee/empty takes over).
    pub fn edit_pick(&self, el: ElementRef) -> Option<ElementRef> {
        if self.interaction_mode != crate::editor::InteractionMode::Edit {
            return Some(el);
        }
        match el {
            ElementRef::Point(pid) if self.managed_handles().contains(&pid) => None,
            _ => Some(el),
        }
    }

    /// Object-mode selection expansion (Move tool only): any picked element
    /// grows to its whole island (shared endpoints + point-coincident glue;
    /// endpoints and construction slots both match). Fills never
    /// expand (fill drag = translate interior; expansion would rewire it to
    /// edge-stretch). Lone points are their own unit. Everywhere else
    /// (Edit, other tools) is identity.
    pub fn expand_to_islands(&self, els: &[ElementRef]) -> Vec<ElementRef> {
        if self.interaction_mode != crate::editor::InteractionMode::Object
            || self.tool != crate::editor::Tool::Move
        {
            return els.to_vec();
        }
        self.doc.island_elements(els)
    }

    /// Post-solve derivation (Clay layer rule): write derived positions into
    /// FREE handle points of managed spans. Handles referenced by any
    /// constraint/dimension stay user-parametric (never fight the solver).
    /// Call after every solution-apply + grade flip. Returns points moved.
    pub fn derive_handles(&mut self) -> Vec<PointId> {
        let pinned = pinned_points(&self.doc);
        let sids = crate::editor::spans::managed_segments(&self.doc, &self.joint_data);
        let mut moves: Vec<(PointId, Point2)> = Vec::new();
        for sid in sids {
            let Some((c1, c2)) =
                crate::editor::spans::derive_segment(&self.doc, &self.joint_data, sid)
            else {
                continue;
            };
            let Some(seg) = self.doc.segment(sid) else {
                continue;
            };
            let (h1, h2) = seg.bezier_handles();
            if let Some(h) = h1 {
                if !pinned.contains(&h) {
                    moves.push((h, c1));
                }
            }
            if let Some(h) = h2 {
                if !pinned.contains(&h) {
                    moves.push((h, c2));
                }
            }
        }
        for (h, p) in &moves {
            self.doc.move_point(*h, *p);
        }
        moves.into_iter().map(|(h, _)| h).collect()
    }
}

/// Points the solver may own: constraint endpoints + every point a dimension
/// references (segments expand to start/end/ctrl/center).
fn pinned_points(doc: &Document) -> HashSet<PointId> {
    let mut set = HashSet::new();
    for c in &doc.constraints {
        set.insert(c.a);
        set.insert(c.b);
    }
    for d in &doc.dimensions {
        for p in dim_target_points(doc, &d.target) {
            set.insert(p);
        }
    }
    set
}

/// Every point a dimension references (segments expand to all four
/// points). Shared by the derive-pass pin set and scope tests.
pub(crate) fn dim_target_points(doc: &Document, target: &DimTarget) -> Vec<PointId> {    match target {
        DimTarget::Points { a, b, .. } => vec![*a, *b],
        DimTarget::PointLine { p, .. } => vec![*p],
        DimTarget::EdgeMid { a, b, .. } | DimTarget::Lines { a, b } | DimTarget::Angle { a, b } => {
            let mut out = Vec::new();
            for s in [a, b] {
                if let Some(seg) = doc.segment(*s) {
                    out.extend([seg.start, seg.end]);
                    out.extend(seg.ctrl);
                    out.extend(seg.center);
                }
            }
            out
        }
        DimTarget::Radius { seg } | DimTarget::CurveLength { seg } => {
            doc.segment(*seg)
                .map(|s| {
                    let mut v = vec![s.start, s.end];
                    v.extend(s.ctrl);
                    v.extend(s.center);
                    v
                })
                .unwrap_or_default()
        }
    }
}

/// Every point a constraint touches: endpoints + tangent-span geometry.
/// A chip/dim lives iff ALL of these are in scope.
pub(crate) fn constraint_points(
    doc: &Document,
    c: &crate::core::constraints::Constraint,
) -> Vec<PointId> {
    let mut v = vec![c.a, c.b];
    if let Some((s1, s2)) = c.tangent_segments {
        for s in [s1, s2] {
            if let Some(g) = doc.segment(s) {
                v.extend([g.start, g.end]);
                v.extend(g.ctrl);
                v.extend(g.center);
            }
        }
    }
    v
}

/// In-scope point set for an isolation scope (segment endpoints + handles
/// + lone points). None = everything (no scope).
pub(crate) fn scope_point_set(
    doc: &Document,
    scope: Option<&crate::editor::EditScope>,
) -> Option<HashSet<PointId>> {
    let s = scope?;
    let mut set: HashSet<PointId> = s.points.iter().copied().collect();
    for id in &s.segments {
        if let Some(g) = doc.segment(*id) {
            set.insert(g.start);
            set.insert(g.end);
            set.extend(g.ctrl);
            set.extend(g.center);
        }
    }
    Some(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::constraints::{Constraint, ConstraintKind};
    use crate::core::geometry::Point2;
    use crate::editor::Editor;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn derive_pass_writes_thirds_and_enrolls() {
        let mut ed = Editor::new();
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(90., 0.));
        let h1 = ed.doc.add_point(Point2::new(1., 2.));
        let h2 = ed.doc.add_point(Point2::new(3., 4.));
        ed.doc.add_bezier_segment(a, h1, h2, b);
        // Unmanaged: nothing moves.
        assert!(ed.derive_handles().is_empty());
        assert!(ed.doc.point(h1).unwrap().x == 1.);
        // Enroll one end (derive-on-touch): both arms derive.
        ed.set_grade(a, Grade::G1);
        let moved = ed.derive_handles();
        assert_eq!(moved.len(), 2);
        let p1 = ed.doc.point(h1).unwrap();
        let p2 = ed.doc.point(h2).unwrap();
        assert!(approx(p1.x, 30.) && approx(p1.y, 0.));
        assert!(approx(p2.x, 60.) && approx(p2.y, 0.));
    }

    #[test]
    fn edit_pick_filters_managed_handles() {
        use crate::core::constraints::ElementRef;
        let mut ed = Editor::new();
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(90., 0.));
        let h1 = ed.doc.add_point(Point2::new(1., 2.));
        let h2 = ed.doc.add_point(Point2::new(3., 4.));
        ed.doc.add_bezier_segment(a, h1, h2, b);
        ed.set_grade(a, Grade::G1);
        ed.derive_handles();
        // Object mode: everything pickable (abdication-free).
        assert!(ed.edit_pick(ElementRef::Point(h1)).is_some());
        ed.set_interaction_mode(crate::editor::InteractionMode::Edit);
        // Edit: derived handles dissolve; joints stay.
        assert!(ed.edit_pick(ElementRef::Point(h1)).is_none());
        assert!(ed.edit_pick(ElementRef::Point(h2)).is_none());
        assert!(ed.edit_pick(ElementRef::Point(a)).is_some());
    }

    #[test]
    fn derive_pass_skips_constrained_handles() {
        let mut ed = Editor::new();
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(90., 0.));
        let anchor = ed.doc.add_point(Point2::new(5., 5.));
        let h1 = ed.doc.add_point(Point2::new(1., 2.));
        let h2 = ed.doc.add_point(Point2::new(3., 4.));
        ed.doc.add_bezier_segment(a, h1, h2, b);
        // Pin h1 with a coincident constraint: solver owns it now.
        ed.doc.constraints.push(Constraint {
            kind: ConstraintKind::Coincident,
            a: h1,
            b: anchor,
            tangent_segments: None,
            point_on_segment: None,
        });
        ed.set_grade(a, Grade::G1);
        let moved = ed.derive_handles();
        // Only the free handle moves; the pinned one keeps its position.
        assert_eq!(moved, vec![h2]);
        assert!(ed.doc.point(h1).unwrap().x == 1.);
        let p2 = ed.doc.point(h2).unwrap();
        assert!(approx(p2.x, 60.) && approx(p2.y, 0.));
    }
}
