use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::core::geometry::Point2;
use crate::core::ids::{PointId, SegmentId};

use super::joints::JointData;
use super::pick;
use super::tools::{ClayPath, PendingLine};
use super::{HANDLE_TOL_PX, InteractionMode};

// Clay creation (Phase 2.9): click-place joints, extrude, connect, close.
// Every click COMMITS immediately (own undo step); spans are Line kind
// (bezier conversion lands later). Placed joints enroll default G1.
// Esc ENDS the path and keeps geometry (deviation from the spec's
// cancel-path: per-click commits already own undo steps, so mass-delete
// on Esc would destroy wanted work — undo explicitly instead).

impl crate::editor::Editor {
    fn clay_layer(&self) -> u64 {
        self.doc.layers[0].id
    }

    /// Make a fresh joint: point + layer ref + default enrollment.
    fn clay_make_joint(&mut self, at: Point2) -> PointId {
        let j = self.doc.add_point(at);
        self.doc.push_to_layer(self.clay_layer(), ElementRef::Point(j));
        self.joint_data.insert(j, JointData::default());
        j
    }

    /// Enroll an adopted point if it can carry a grade (bezier endpoint).
    /// True when this call created the entry (caller records provenance).
    fn clay_enroll(&mut self, pid: PointId) -> bool {
        if !self.is_managed(pid) && self.is_bezier_endpoint(pid) {
            self.joint_data.insert(pid, JointData::default());
            true
        } else {
            false
        }
    }

    /// Commit a Line span end->to and advance the active end. Both joints
    /// must already exist (created or adopted). The new span joins the
    /// isolation scope (your new geometry is always editable).
    fn clay_commit_span(&mut self, from: PointId, to: PointId) -> SegmentId {
        let seg = self.doc.add_stroked_segment(from, to, 1.0);
        self.doc.push_to_layer(self.clay_layer(), ElementRef::Segment(seg));
        if let Some(path) = self.clay_path.as_mut() {
            path.spans.push(seg);
            if path.flipped {
                path.joints.insert(0, to);
            } else {
                path.joints.push(to);
            }
        }
        if let Some(scope) = self.edit_scope.as_mut() {
            if !scope.segments.contains(&seg) {
                scope.segments.push(seg);
            }
        }
        seg
    }

    /// Arm the extrude ghost: preview from the active end to `cursor`.
    /// The ghost is DERIVED state — live whenever a path is active, not a
    /// mode. Click commits its tip and continues; Enter commits its tip
    /// and finishes. (Reuses the Line pending slot for preview + drag
    /// tracking; the Clay arm owns all commits.)
    fn clay_arm_ghost(&mut self, cursor: Point2) {
        let end = self.clay_path.as_ref().and_then(|p| p.active_end());
        match end.and_then(|e| self.doc.point(e).map(|pos| (e, pos))) {
            Some((_, pos)) => {
                let (at, guides) = self.snap_creation_point(cursor);
                self.snap_guides = guides;
                self.pending_line = Some(PendingLine { start: pos, cursor: at });
                self.pending_via_click = false;
            }
            None => {
                self.pending_line = None;
            }
        }
    }

    /// Start a path: GLUE onto the hit point (fresh joint + coincident —
    /// Clay never shares foreign points, so touching never fuses objects)
    /// or create at the snapped cursor.
    fn clay_start(&mut self, hit: Option<PointId>, at: Point2) -> PointId {
        let mut path = ClayPath::default();
        let j = match hit {
            Some(pid) => {
                let pos = self.doc.point(pid).unwrap_or(at);
                let j = self.clay_make_joint(pos);
                self.doc
                    .add_constraint(ConstraintKind::Coincident, j, pid);
                path.owned.push(j);
                path.enrolled.push(j);
                j
            }
            None => {
                let j = self.clay_make_joint(at);
                path.owned.push(j);
                path.enrolled.push(j);
                j
            }
        };
        path.joints.push(j);
        self.clay_path = Some(path);
        self.selection = vec![ElementRef::Point(j)];
        self.clay_status = Some((
            "Path started".into(),
            "click continue · Enter finish · C close".into(),
        ));
        j
    }

    pub fn clay_click(&mut self, cursor: gpui::Point<gpui::Pixels>, _shift: bool) -> bool {
        if self.interaction_mode != InteractionMode::Edit {
            self.set_interaction_mode(InteractionMode::Edit);
        }
        let doc_p = self.cursor_doc(cursor);
        let (at, guides) = self.snap_creation_point(doc_p);
        self.snap_guides = guides;
        let hit = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX).point(doc_p);

        // No active path: start one (ghost arms immediately).
        if self.clay_path.is_none() {
            self.clay_start(hit, at);
            self.clay_arm_ghost(at);
            self.perpendicular_preview = None;
            return true;
        }
        let (end, other, len) = match &self.clay_path {
            Some(p) => (p.active_end(), p.other_end(), p.joints.len()),
            None => (None, None, 0),
        };
        match hit {
            // Click on the active end: no-op (extrude with E instead).
            Some(pid) if Some(pid) == end => true,
            // Click on the far end: flip the active end (extend from
            // either side; close with C). Geometry untouched.
            Some(pid) if Some(pid) == other && len >= 2 => {
                if let Some(path) = self.clay_path.as_mut() {
                    path.flipped = !path.flipped;
                }
                self.selection = vec![ElementRef::Point(pid)];
                self.clay_status = Some((
                    "Active end flipped".into(),
                    "click extends this end".into(),
                ));
                self.clay_arm_ghost(doc_p);
                true
            }
            // Own middle joint: ignore (no topology tangles in 2.9).
            Some(pid)
                if self
                    .clay_path
                    .as_ref()
                    .is_some_and(|p| p.joints.contains(&pid)) =>
            {
                true
            }
            // Glue onto an external point: fresh joint + coincident
            // constraint (two objects travelling together). Shared IDs
            // would fuse objects — fuse explicitly with J instead.
            Some(pid) => {
                // Isolation: a foreign point outside the scope doesn't
                // exist — swallow the click (creation in empty space stays
                // allowed; it joins the scope on commit).
                if !self.in_scope_point(pid) {
                    return true;
                }
                let end = end.expect("clay path without end");
                let pos = self.doc.point(pid).unwrap_or(at);
                let j = self.clay_make_joint(pos);
                self.doc
                    .add_constraint(ConstraintKind::Coincident, j, pid);
                if let Some(path) = self.clay_path.as_mut() {
                    path.owned.push(j);
                    path.enrolled.push(j);
                }
                self.clay_commit_span(end, j);
                self.selection = vec![ElementRef::Point(j)];
                self.clay_status = Some((
                    "Joint glued".into(),
                    "click continue · Enter finish · C close · ⌫ drop".into(),
                ));
                self.perpendicular_preview = None;
                self.clay_arm_ghost(at);
                true
            }
            // Empty space: fresh joint + span off the active end.
            None => {
                let end = end.expect("clay path without end");
                let j = self.clay_make_joint(at);
                if let Some(path) = self.clay_path.as_mut() {
                    path.owned.push(j);
                    path.enrolled.push(j);
                }
                self.clay_commit_span(end, j);
                self.selection = vec![ElementRef::Point(j)];
                self.clay_status = Some((
                    "Joint placed".into(),
                    "click continue · Enter finish · C close · ⌫ drop".into(),
                ));
                self.perpendicular_preview = None;
                self.clay_arm_ghost(at);
                true
            }
        }
    }

    /// Close the path: span active end -> far end, end path, keep geometry.
    pub fn clay_close(&mut self) -> bool {
        let (end, other, len) = match &self.clay_path {
            Some(p) => (p.active_end(), p.other_end(), p.joints.len()),
            None => return false,
        };
        let (Some(end), Some(other)) = (end, other) else {
            return false;
        };
        if len < 2 || end == other {
            return false;
        }
        let seg = self.doc.add_stroked_segment(end, other, 1.0);
        self.doc.push_to_layer(self.clay_layer(), ElementRef::Segment(seg));
        self.clay_path = None;
        self.pending_line = None;
        self.pending_via_click = false;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        self.selection = vec![ElementRef::Segment(seg)];
        self.clay_status = Some((
            "Path closed".into(),
            "click starts a new path".into(),
        ));
        true
    }

    /// Extrude: keyboard-only path entry. Starts a path at the last cursor
    /// when none is active (the mouse path is click). With a path active
    /// the ghost is already live, so E is a no-op — ghost is derived, not
    /// a mode. Returns whether anything happened.
    pub fn clay_extrude(&mut self) -> bool {
        let mut changed = false;
        if self.interaction_mode != InteractionMode::Edit {
            self.set_interaction_mode(InteractionMode::Edit);
            changed = true;
        }
        if self.clay_path.is_some() {
            return changed;
        }
        // last_cursor is screen px; the snap pipeline wants doc units.
        let at = match self.last_cursor {
            Some(c) => self
                .camera
                .screen_to_unit(Point2::new(f64::from(c.x), f64::from(c.y))),
            None => return changed,
        };
        let (at, guides) = self.snap_creation_point(at);
        self.snap_guides = guides;
        let hit = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX).point(at);
        self.clay_start(hit, at);
        self.clay_arm_ghost(at);
        self.perpendicular_preview = None;
        true
    }

    /// Drop the last joint: pop span (delete_element GCs orphaned owned
    /// points AND adopted loners alike), restore an adopted joint the GC
    /// ate (nothing referenced it, so a fresh id is safe — bookkeeping is
    /// patched), sweep clay-created sidecar, pop bookkeeping. Adopted
    /// points are never deleted by intent, only by GC + restore.
    pub fn clay_drop_last(&mut self) -> bool {
        let (seg, joint, owned) = match &self.clay_path {
            Some(p) => (
                p.spans.last().copied(),
                p.active_end(),
                p.active_end().map(|j| p.owned.contains(&j)).unwrap_or(false),
            ),
            None => return false,
        };
        if seg.is_none() && joint.is_none() {
            return false;
        }
        // Snapshot REMAINING adopted joints: delete_element GCs orphaned
        // endpoints alike, and an adopted loner has no other users. The
        // dropped active end is excluded (its removal is the point).
        let victims: Vec<(PointId, Point2)> = match &self.clay_path {
            Some(p) => {
                let drop = p.active_end();
                p.joints
                    .iter()
                    .filter(|j| Some(**j) != drop && !p.owned.contains(j))
                    .filter_map(|j| self.doc.point(*j).map(|pos| (*j, pos)))
                    .collect()
            }
            None => return false,
        };
        if let Some(s) = seg {
            self.delete_element(ElementRef::Segment(s));
        } else if let Some(j) = joint {
            // Single joint, no span: only owned points die.
            if owned {
                self.delete_element(ElementRef::Point(j));
            }
        }
        for (j, pos) in victims {
            if self.doc.point(j).is_none() {
                let nj = self.doc.add_point(pos);
                self.doc.push_to_layer(self.clay_layer(), ElementRef::Point(nj));
                // Patch bookkeeping to the fresh id (nothing referenced the
                // old one — the collector proved it).
                if let Some(path) = self.clay_path.as_mut() {
                    for q in path.joints.iter_mut() {
                        if *q == j {
                            *q = nj;
                        }
                    }
                }
            }
        }
        if let Some(j) = joint {
            if self
                .clay_path
                .as_ref()
                .is_some_and(|p| p.enrolled.contains(&j))
            {
                self.joint_data.remove(&j);
            }
        }
        if let Some(p) = self.clay_path.as_mut() {
            p.spans.pop();
            if p.flipped {
                if !p.joints.is_empty() {
                    p.joints.remove(0);
                }
            } else {
                p.joints.pop();
            }
            p.owned.retain(|o| Some(*o) != joint);
            p.enrolled.retain(|o| Some(*o) != joint);
            if p.joints.is_empty() {
                self.clay_path = None;
            }
        }
        // Feedback: select the new active end.
        if let Some(path) = &self.clay_path {
            if let Some(end) = path.active_end() {
                self.selection = vec![ElementRef::Point(end)];
            }
        }
        // Re-arm the ghost from the new active end (the popped span's
        // anchor is stale).
        match self
            .clay_path
            .as_ref()
            .and_then(|p| p.active_end())
            .and_then(|e| self.doc.point(e))
        {
            Some(pos) => {
                self.pending_line = Some(PendingLine { start: pos, cursor: pos });
                self.pending_via_click = false;
            }
            None => {
                self.pending_line = None;
            }
        }
        self.clay_status = Some((
            "Joint dropped".into(),
            "⌫ drop · C close · Enter end".into(),
        ));
        true
    }

    /// End the path, keep geometry. Commits a live extrude ghost at its
    /// cursor first (if non-degenerate).
    pub fn clay_end(&mut self) -> bool {        if self.clay_path.is_none() && self.pending_line.is_none() {
            return false;
        }
        if let Some(pending) = self.pending_line.take() {
            let end = self.clay_path.as_ref().and_then(|p| p.active_end());
            if let Some(end) = end {
                if pick::distance(pending.start, pending.cursor) > 1e-6 {
                    let j = self.clay_make_joint(pending.cursor);
                    if let Some(path) = self.clay_path.as_mut() {
                        path.owned.push(j);
                        path.enrolled.push(j);
                    }
                    self.clay_commit_span(end, j);
                    self.selection = vec![ElementRef::Point(j)];
                }
            }
        }
        self.pending_via_click = false;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        self.clay_path = None;
        self.clay_status = Some((
            "Path ended".into(),
            "click starts a new path".into(),
        ));
        true
    }

    /// Finish the path WITHOUT committing any ghost (Esc semantics).
    /// Pending slot untouched: under a foreign tool it belongs to Line.
    /// Selection clears: Esc leaves committed-clean, nothing half-lit.
    pub fn clay_finish(&mut self) -> bool {
        if self.clay_path.is_none() {
            return false;
        }
        self.clay_path = None;
        self.pending_via_click = false;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        self.selection.clear();
        self.clay_status = Some((
            "Path ended".into(),
            "click starts a new path".into(),
        ));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::constraints::{ConstraintKind, ElementRef};
    use crate::editor::Editor;

    fn click(ed: &mut Editor, x: f32, y: f32) {
        ed.snap_to_grid = false;
        ed.snap_to_objects = false;
        ed.tool = crate::editor::Tool::Clay;
        assert!(ed.clay_click(gpui::point(gpui::px(x), gpui::px(y)), false));
    }

    #[test]
    fn place_two_joints_commits_line() {
        let mut ed = Editor::new();
        click(&mut ed, 10., 10.);
        click(&mut ed, 100., 10.);
        let path = ed.clay_path.as_ref().unwrap();
        assert_eq!(path.joints.len(), 2);
        assert_eq!(path.spans.len(), 1);
        assert_eq!(path.owned.len(), 2);
        let seg = ed.doc.segment(path.spans[0]).unwrap();
        assert_eq!(seg.kind, crate::core::document::SegmentKind::Line);
        assert_eq!((seg.start, seg.end), (path.joints[0], path.joints[1]));
        // Enrolled + selected feedback on the new joint.
        assert!(ed.is_managed(path.joints[1]));
        assert_eq!(ed.selection, vec![ElementRef::Point(path.joints[1])]);
        // Modebar narration.
        let (label, hint) = ed.clay_status.clone().unwrap();
        assert_eq!(label, "Joint placed");
        assert!(hint.contains('E'));
        // Click the far end: flip (no geometry change).
        let start = path.joints[0];
        click(&mut ed, 10., 10.);
        let path = ed.clay_path.as_ref().unwrap();
        assert!(path.flipped);
        assert_eq!(path.spans.len(), 1);
        assert_eq!(path.active_end(), Some(start));
    }

    #[test]
    fn touch_glues_instead_of_fusing() {
        use crate::core::constraints::ConstraintKind;
        let mut ed = Editor::new();
        let ext = ed.doc.add_point(Point2::new(500., 500.));
        click(&mut ed, 500., 500.); // touch: fresh joint + coincident
        let path = ed.clay_path.as_ref().unwrap();
        assert_eq!(path.joints.len(), 1);
        let j0 = path.joints[0];
        assert_ne!(j0, ext, "never shares foreign points");
        assert_eq!(path.owned, vec![j0]);
        assert!(ed.doc.constraints.iter().any(|c| {
            c.kind == ConstraintKind::Coincident
                && ((c.a == j0 && c.b == ext) || (c.a == ext && c.b == j0))
        }));
        // Foreign point untouched: no segments reference it.
        assert!(ed
            .doc
            .all_segments()
            .all(|(_, s)| s.start != ext && s.end != ext));
        click(&mut ed, 100., 10.);
        assert!(ed.clay_close());
        assert!(ed.clay_path.is_none());
        assert_eq!(ed.doc.all_segments().count(), 2);
        // Islands: path spans are one object; ext stands alone.
        let islands = ed.doc.islands();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].len(), 2);
    }

    #[test]
    fn drop_last_removes_owned_only() {
        let mut ed = Editor::new();
        let ext = ed.doc.add_point(Point2::new(500., 500.));
        click(&mut ed, 500., 500.); // glued start (owned joint + coincident)
        click(&mut ed, 100., 10.); // owned j1
        let path = ed.clay_path.as_ref().unwrap();
        let (j0, j1) = (path.joints[0], path.joints[1]);
        assert_ne!(j0, ext);
        assert!(ed.clay_drop_last());
        // Span gone, owned j1 GC'd, sidecar swept.
        assert!(ed.doc.all_segments().count() == 0);
        assert!(ed.doc.point(j1).is_none());
        assert!(!ed.is_managed(j1));
        // Glued start survives (coincident pins it against the GC).
        let path = ed.clay_path.as_ref().unwrap();
        assert_eq!(path.joints.len(), 1);
        let start = path.joints[0];
        assert_eq!(start, j0);
        assert!(ed.doc.point(start).is_some());
        // Foreign point never referenced by any segment.
        assert!(ed
            .doc
            .all_segments()
            .all(|(_, s)| s.start != ext && s.end != ext));
        // Drop the last (owned) joint: bookkeeping ends, point + glue die.
        assert!(ed.clay_drop_last());
        assert!(ed.clay_path.is_none());
        assert!(ed.doc.point(start).is_none());
        assert!(ed.doc.point(ext).is_some());
    }

    #[test]
    fn ghost_is_derived_and_enter_finishes() {
        let mut ed = Editor::new();
        click(&mut ed, 10., 10.);
        // Ghost armed immediately: no E needed for the preview.
        assert!(ed.pending_line.is_some());
        if let Some(p) = ed.pending_line.as_mut() {
            p.cursor = Point2::new(60., 10.);
        }
        // Enter commits the ghost tip AND finishes.
        assert!(ed.clay_end());
        assert!(ed.clay_path.is_none());
        assert_eq!(ed.doc.all_segments().count(), 1);
    }

    #[test]
    fn extrude_starts_path_at_last_cursor() {
        let mut ed = Editor::new();
        ed.snap_to_grid = false;
        ed.snap_to_objects = false;
        ed.tool = crate::editor::Tool::Clay;
        ed.last_cursor = Some(gpui::point(gpui::px(25.), gpui::px(25.)));
        assert!(ed.clay_extrude());
        let path = ed.clay_path.as_ref().unwrap();
        assert_eq!(path.joints.len(), 1);
        assert!(ed.pending_line.is_some());
        // Second E with a live path: no-op (ghost already derived).
        assert!(!ed.clay_extrude());
    }

    #[test]
    fn edit_scope_freezes_and_clay_extends() {        use crate::core::constraints::ElementRef;
        let mut ed = Editor::new();
        // Two islands: chain A-B + lone segment C-D.
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(10., 0.));
        let c = ed.doc.add_point(Point2::new(20., 0.));
        let d = ed.doc.add_point(Point2::new(100., 0.));
        let e = ed.doc.add_point(Point2::new(110., 0.));
        let s1 = ed.doc.add_segment(a, b);
        let _s2 = ed.doc.add_segment(b, c);
        let other = ed.doc.add_segment(d, e);
        // Select one edge in Object, enter Edit: scope = its island.
        ed.selection = vec![ElementRef::Segment(s1)];
        assert!(ed.set_interaction_mode(crate::editor::InteractionMode::Edit));
        let scope = ed.edit_scope.clone().unwrap();
        assert_eq!(scope.segments.len(), 2);
        assert!(ed.in_scope_segment(s1));
        assert!(!ed.in_scope_segment(other));
        assert!(ed.in_scope_point(a));
        assert!(!ed.in_scope_point(d));
        assert!(!ed.in_scope_element(ElementRef::Segment(other)));
        // Clay creation off the scoped end joins the scope.
        ed.tool = crate::editor::Tool::Clay;
        click(&mut ed, 20., 0.); // adopt chain end (glue)
        click(&mut ed, 30., 0.); // fresh joint + span
        let path = ed.clay_path.as_ref().unwrap();
        assert_eq!(path.spans.len(), 1);
        let new_seg = path.spans[0];
        assert!(ed.in_scope_segment(new_seg));
        // Exiting to Object clears scope and expands selection to islands.
        assert!(ed.set_interaction_mode(crate::editor::InteractionMode::Object));
        assert!(ed.edit_scope.is_none());
    }

    #[test]
    fn annotations_die_outside_scope() {
        use crate::core::constraints::{DimMode, DimTarget, Dimension};
        let mut ed = Editor::new();
        // Island 1 (scoped) + island 2 (dead): one dim + one chip each.
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(10., 0.));
        let c = ed.doc.add_point(Point2::new(100., 0.));
        let d = ed.doc.add_point(Point2::new(110., 0.));
        let s1 = ed.doc.add_segment(a, b);
        let s2 = ed.doc.add_segment(c, d);
        ed.doc.dimensions.push(Dimension {
            target: DimTarget::Points { a, b, mode: DimMode::Aligned },
            value: 10.,
            offset: 0.,
            slide: 0.,
            sweep: 0.,
        });
        ed.doc.dimensions.push(Dimension {
            target: DimTarget::Points { a: c, b: d, mode: DimMode::Aligned },
            value: 10.,
            offset: 0.,
            slide: 0.,
            sweep: 0.,
        });
        ed.selection = vec![ElementRef::Segment(s1)];
        assert!(ed.set_interaction_mode(crate::editor::InteractionMode::Edit));
        assert!(ed.dim_in_scope(0));
        assert!(!ed.dim_in_scope(1));
        assert!(!ed.dim_in_scope(99));
        // Out-of-scope dim selection dies on entry.
        ed.selected_dim = Some(1);
        ed.set_interaction_mode(crate::editor::InteractionMode::Object);
        ed.set_interaction_mode(crate::editor::InteractionMode::Edit);
        assert!(ed.selected_dim.is_none());
        // Chips follow the same rule.
        let inchip = crate::core::constraints::Constraint {
            kind: crate::core::constraints::ConstraintKind::Horizontal,
            a,
            b,
            tangent_segments: None,
            point_on_segment: None,
        };
        let outchip = crate::core::constraints::Constraint {
            kind: crate::core::constraints::ConstraintKind::Horizontal,
            a: c,
            b: d,
            tangent_segments: None,
            point_on_segment: None,
        };
        assert!(ed.marker_in_scope(&inchip));
        assert!(!ed.marker_in_scope(&outchip));
        let _ = s2;
    }

    #[test]
    fn touch_leaves_foreign_points_unenrolled() {        let mut ed = Editor::new();
        let a = ed.doc.add_point(Point2::new(0., 0.));
        let b = ed.doc.add_point(Point2::new(90., 0.));
        let h1 = ed.doc.add_point(Point2::new(1., 1.));
        let h2 = ed.doc.add_point(Point2::new(2., 2.));
        ed.doc.add_bezier_segment(a, h1, h2, b);
        click(&mut ed, 0., 0.); // touch bezier endpoint: glue, don't enroll
        assert!(!ed.is_managed(a), "foreign points stay unenrolled");
        let j0 = ed.clay_path.as_ref().unwrap().joints[0];
        assert_ne!(j0, a);
        assert!(ed.is_managed(j0));
    }
}
