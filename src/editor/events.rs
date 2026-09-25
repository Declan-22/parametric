use crate::core::constraints::{ConstraintKind, DimTarget, ElementRef};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{PointId, SegmentId};

use super::DimDrag;
use super::DimInput;
use super::DimPick;
use super::EXACT_TOL_PX;
use super::Editor;
use super::HANDLE_TOL_PX;
use super::drag::DragState;
use super::PenMode;
use super::PendingCircle;
use super::PendingLine;
use super::PendingRuler;
use super::PendingShape;
use super::Tool;
use super::fillet;
use super::pick;
use super::tools;

// Canvas pointer events: down / drag / up / hover + chip hit-testing.
impl Editor {
    pub fn canvas_down(
        &mut self,
        button: gpui::MouseButton,
        cursor: gpui::Point<gpui::Pixels>,
        shift: bool,
        click_count: usize,
    ) -> bool {
        match button {
            gpui::MouseButton::Middle => {
                // MMB always pans, whatever tool is active. No history: a
                // pan never mutates the document.
                self.begin_pan(cursor);
                true
            }
            gpui::MouseButton::Left => {
                // Every Left gesture is one undo step; the snapshot commits
                // lazily only if the document actually changed.
                self.history_begin();
                // Keep the snap crosshair in sync with click placements
                // (click-created shapes land exactly on the drawn crosshair).
                let _ = self.update_creation_cursor(cursor);
                // Constraint chips sit ON TOP of geometry — clicking one
                // toggles the chip selection and never touches geometry.
                if self.tool == Tool::Move
                    && let Some(c) = self.constraint_chip_at(cursor)
                {
                    if self.selected_constraints.contains(&c) {
                        self.selected_constraints.retain(|&x| x != c);
                    } else {
                        self.selected_constraints.clear();
                        self.selected_constraints.push(c);
                    }
                    return true;
                }
                // Fillet center/tangent press: stage it (don't convert yet).
                // A real drag converts past the click threshold; a clean
                // release emulates the consumed click instead. Staging
                // (rather than grabbing immediately) keeps fill selects,
                // edge selects, and marquees working near handles.
                if matches!(self.tool, Tool::Move | Tool::Fillet) {
                    let at = self.cursor_doc(cursor);
                    if let Some(index) = self.fillet_handle_at(at) {
                        self.fillet_press =
                            Some(fillet::FilletPress { modifier_index: index, down: at });
                        return true;
                    }
                }
                if self.is_constraint_tool() {
                    return self.constraint_tool_click(cursor);
                }
                match self.tool {
                Tool::Pan => {
                    self.begin_pan(cursor);
                    true
                }
                Tool::Rectangle => {
                    // Second click commits a click-created pending rectangle.
                    if let Some(pending) = self.pending_shape.take() {
                        self.pending_via_click = false;
                        self.snap_guides.clear();
                        self.tool = Tool::Move;
                        self.creation_cursor = None;
                        let b = pending.bounds();
                        if b.size.w > 0. && b.size.h > 0. {
                            let layer_id = self.doc.layers[0].id;
                            let fill = self.create_rectangle(layer_id, b.origin, Point2::new(
                                b.origin.x + b.size.w,
                                b.origin.y + b.size.h,
                            ));
                            self.selection = vec![ElementRef::Fill(fill)];
                        }
                        return true;
                    }
                    let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    self.pending_shape =
                        Some(PendingShape { start: at, cursor: at, proportional: false });
                    self.pending_via_click = true;
                    true
                }
                Tool::Ruler => {
                    // Second click commits a click-created pending ruler.
                    if let Some(pending) = self.pending_ruler.take() {
                        self.pending_via_click = false;
                        self.snap_guides.clear();
                        self.tool = Tool::Move;
                        self.creation_cursor = None;
                        let (_, b) = pending.snapped(shift);
                        if pick::distance(b, pending.start) > 1e-6 {
                            let layer_id = self.doc.layers[0].id;
                            let seg = self.create_ruler(layer_id, pending.start, b);
                            self.selection = vec![ElementRef::Segment(seg)];
                        }
                        return true;
                    }
                    let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    self.pending_ruler = Some(PendingRuler { start: at, cursor: at });
                    self.pending_via_click = true;
                    true
                }
                Tool::Line => {
                    // Continuous mode: the tool stays active and each
                    // commit chains the next line from its endpoint.
                    // A click on a fresh (zero-length) link re-anchors
                    // the start instead of committing.
                    if let Some(pending) = self.pending_line.take() {
                        let (_, mut b) = pending.snapped(shift);
                        if shift {
                            if let Some(q) = self.tangent_snap_for_line(pending.start, pending.cursor) { b = q; }
                        }
                        if pick::distance(b, pending.start) > 1e-6 {
                            self.snap_guides.clear();
                            let layer_id = self.doc.layers[0].id;
                            let seg = self.create_line(layer_id, pending.start, b);
                            if let Some((source, _, _)) = self.perpendicular_preview.take() {
                                self.doc.add_perpendicular_constraint(source, seg);
                            }
                            if shift { self.maybe_add_tangent(seg, b); }
                            self.selection = vec![ElementRef::Segment(seg)];
                            // Chain: the next line starts where this one ended.
                            self.pending_line = Some(PendingLine { start: b, cursor: b });
                        } else {
                            let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                            self.snap_guides = guides;
                            self.pending_line = Some(PendingLine { start: at, cursor: at });
                        }
                        self.pending_via_click = true;
                        return true;
                    }
                    let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    self.pending_line = Some(PendingLine { start: at, cursor: at });
                    self.pending_via_click = true;
                    true
                }
                Tool::Circle => {
                    // 3-click arc: a -> b -> c (on-arc) commits.
                    if let Some(pending) = &self.pending_circle {
                        if pending.a.is_some() && pending.b.is_some() {
                            let pending = self.pending_circle.take().unwrap();
                            self.pending_via_click = false;
                            self.snap_guides.clear();
                            self.tool = Tool::Move;
                            self.creation_cursor = None;
                            if let (Some(a), Some(b)) = (pending.a, pending.b) {
                                let c = pending.cursor;
                                let layer_id = self.doc.layers[0].id;
                                let seg = self.create_arc(layer_id, a, b, c);
                                self.selection = vec![ElementRef::Segment(seg)];
                            }
                            return true;
                        }
                    }
                    let (mut at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    match self.pending_circle.as_mut() {
                        // Second click: fix the chord's far end. Shift locks
                        // the chord's direction to 45-degree steps.
                        Some(p) if p.a.is_some() && p.b.is_none() => {
                            if shift && let Some(a) = p.a {
                                at = tools::snap_angle(a, at);
                            }
                            p.b = Some(at);
                            p.cursor = at;
                            self.pending_via_click = true;
                        }
                        // First click: chord start.
                        _ => {
                            self.pending_circle =
                                Some(PendingCircle { a: Some(at), b: None, cursor: at });
                            self.pending_via_click = true;
                        }
                    }
                    true
                }
                Tool::Move => {
                    // Placed dimension containers float above geometry:
                    // double-click edits, first tap selects, second tap
                    // edits, drag repositions.
                    if self.dim_input.is_none()
                        && let Some(idx) = self.dim_at(cursor)
                    {
                        if click_count >= 2 {
                            self.begin_dim_edit(idx);
                            return true;
                        }
                        if self.selected_dim == Some(idx) {
                            self.dim_drag = Some(DimDrag {
                                index: idx,
                                down_doc: self.cursor_doc(cursor),
                                moved: false,
                                was_selected: true,
                            });
                            self.history_begin();
                        } else {
                            self.selected_dim = Some(idx);
                        }
                        return true;
                    }
                    // Tapping geometry or empty space deselects the dim.
                    self.selected_dim = None;
                    self.move_tool_down(cursor, shift, click_count)
                }
                Tool::Dimension => {
                    // Value-input state swallows clicks; Enter/Esc drive it.
                    if self.dim_input.is_some() {
                        return true;
                    }
                    // Placed containers: double-click edits, first tap
                    // selects, second tap edits, drag repositions.
                    if let Some(idx) = self.dim_at(cursor) {
                        if click_count >= 2 {
                            self.begin_dim_edit(idx);
                        } else if self.selected_dim == Some(idx) {
                            self.dim_drag = Some(DimDrag {
                                index: idx,
                                down_doc: self.cursor_doc(cursor),
                                moved: false,
                                was_selected: true,
                            });
                            self.history_begin();
                        } else {
                            self.selected_dim = Some(idx);
                        }
                        return true;
                    }
                    let doc_p = self.cursor_doc(cursor);
                    let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
                    let new_pick = picker
                        .point(doc_p)
                        .map(DimPick::Point)
                        .or_else(|| picker.segment(doc_p).map(DimPick::Line));
                    // Accumulate geometry picks before considering placement.
                    // A line is already a valid length target, so the old
                    // ordering treated the second edge click as placement
                    // instead of allowing an edge + edge angle target.
                    if let Some(pick) = new_pick {
                        // Re-clicking a pick deselects it.
                        if let Some(pos) = self.dim_picks.iter().position(|&p| p == pick) {
                            self.dim_picks.remove(pos);
                        } else {
                            self.dim_picks.push(pick);
                            // A pair overflows: drop the oldest pick so the two
                            // most recent picks define the dimension.
                            if self.dim_picks.len() > 2 {
                                self.dim_picks.remove(0);
                            }
                        }
                        self.dim_target = self.resolve_dim_target(&self.dim_picks);
                        // A pick implying its own type (arc radius, angle,
                        // point-line, ...) lifts a lock that can't apply to
                        // it, so the menu never shows a stale restriction.
                        self.drop_stale_dim_lock();
                        // Mirror the picks as selection so they highlight —
                        // without visible feedback a pick looks like it failed.
                        self.selection = self
                            .dim_picks
                            .iter()
                            .map(|p| match p {
                                DimPick::Point(id) => ElementRef::Point(*id),
                                DimPick::Line(id) => ElementRef::Segment(*id),
                            })
                            .collect();
                        return true;
                    }
                    // Placement mode: a click on empty space places the
                    // pending dimension at the cursor. Geometry clicks were
                    // handled above so a second edge can form an angle.
                    if let Some(target) = self.dim_target {
                        if let Some((mode, offset, slide, measured)) = self.dim_placement(target, doc_p) {
                            self.dim_picks.clear();
                            self.dim_target = None;
                            self.selection.clear();
                            self.dim_input = Some(DimInput {
                                target: target.with_mode(mode), offset, slide, measured,
                                buffer: String::new(), existing: None,
                            });
                        }
                        return true;
                    }
                    false
                }
                Tool::Fillet => self.fillet_click(cursor),
                Tool::Pen => self.pen_tool_click(cursor, shift),
                // Constraint tools are handled before this mode match so
                // their clicks never enter shape/dimension creation. Keep an
                // explicit arm for exhaustive enum matching.
                Tool::ConstraintHorizontalVertical
                | Tool::ConstraintTangent
                | Tool::ConstraintCoincident
                | Tool::ConstraintParallel
                | Tool::ConstraintPerpendicular => false,
            }
            }
            _ => false,
        }
    }

    fn move_tool_down(
        &mut self,
        cursor: gpui::Point<gpui::Pixels>,
        shift: bool,
        click_count: usize,
    ) -> bool {
        let p = self.cursor_doc(cursor);
        // Pressing the canvas dismisses constraint-chip selection.
        self.selected_constraints.clear();
        let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        // Shift extends the selection instead of replacing it.
        self.marquee_add = shift;

        // Exact hit (tight tolerance) grabs immediately. A TOLERANT-only
        // hit near geometry stays a marquee — the grab zone around
        // points/edges must not create dead zones for band selection. The
        // deferred pick resolves on mouse-up as a click if the band never
        // grew.
        let exact = pick::Picker::new(&self.doc, &self.camera, EXACT_TOL_PX).element(p);
        match picker.element(p) {
            Some(mut el) => {
                // Multi-selections grab on the TOLERANT hit: points are far
                // harder to hit exactly than lines, and a missed exact grab
                // would silently become a marquee and drop the selection.
                let part_of_multi_selection =
                    self.selection.len() > 1 && self.element_selected(el);
                if exact.is_none() && !part_of_multi_selection {
                    self.deferred_pick = Some(el);
                    self.marquee = Some((p, p));
                    return true;
                }
                // Double-click on an edge escalates to its containing object.
                if click_count >= 2
                    && let Some(sid) = el.as_segment()
                    && let Some(fid) = self.fill_containing(sid)
                {
                    el = ElementRef::Fill(fid);
                }
                // Pressing part of an ALREADY-SELECTED object keeps the
                // whole selection; pressing something unselected replaces
                // it — unless shift adds.
                if !self.element_selected(el) {
                    if self.marquee_add {
                        self.selection.push(el);
                    } else {
                        self.selection = vec![el];
                    }
                }

                // Grab semantics by what was pressed:
                //  - POINT with a SOLO selection -> resize mode: the corner
                //    chases the cursor; constraint-neighbors join as soft
                //    followers so they slide along their edges.
                //  - POINT within a MULTI-selection -> the whole selection
                //    translates together.
                //  - SEGMENT -> EDGE-STRETCH: the edge's two endpoints are
                //    dragged, constraint-neighbors follow, so pulling an
                //    edge of a selected rectangle reshapes it instead of
                //    translating. (Translate via the fill's interior.)
                //  - FILL -> selection-wide translation.
                // A literal point hit is always a point-resize gesture. In
                // particular, a tangent relation often leaves both spans
                // selected; treating the far line endpoint as a group drag
                // accidentally drags the arc contact and makes the arc spin.
                // Move a selected group through its body/edge instead.
                let solo_point = match el {
                    ElementRef::Point(pid) => Some(pid),
                    _ => None,
                };
                let ring_of = |pids: &[PointId]| -> Vec<(PointId, Point2)> {
                    let mut aux: Vec<(PointId, Point2)> = Vec::new();
                    let mut push_aux = |o: PointId, aux: &mut Vec<(PointId, Point2)>| {
                        if o != pids[0]
                            && !pids.contains(&o)
                            && !aux.iter().any(|&(id, _)| id == o)
                            && self.doc.point(o).is_some()
                        {
                            aux.push((o, self.doc.point(o).unwrap()));
                        }
                    };
                    for &pid in pids {
                        for c in &self.doc.constraints {
                            let other = if c.a == pid {
                                Some(c.b)
                            } else if c.b == pid {
                                Some(c.a)
                            } else {
                                None
                            };
                            if let Some(o) = other
                                && o != pid
                                && !pids.contains(&o)
                                && !aux.iter().any(|&(id, _)| id == o)
                                && self.doc.point(o).is_some()
                            {
                                aux.push((o, self.doc.point(o).unwrap()));
                            }
                            // Tangent stores a==b==contact, so the generic
                            // pair above yields nothing — pull the owning
                            // line's far endpoints explicitly so the line can
                            // rotate instead of pinning the arc center.
                            match c.kind {
                                ConstraintKind::Tangent => {
                                    let Some((line_id, _)) = c.tangent_segments else { continue };
                                    let Some(line) = self.doc.segment(line_id) else { continue };
                                    if c.a != pid && c.b != pid && line.start != pid && line.end != pid {
                                        continue;
                                    }
                                    for o in [line.start, line.end] {
                                        push_aux(o, &mut aux);
                                    }
                                }
                                ConstraintKind::Parallel => {
                                    let Some((first, second)) = c.tangent_segments else { continue };
                                    let (Some(a_seg), Some(b_seg)) = (self.doc.segment(first), self.doc.segment(second)) else { continue };
                                    let touches = [a_seg.start, a_seg.end, b_seg.start, b_seg.end].contains(&pid);
                                    if !touches { continue; }
                                    for o in [a_seg.start, a_seg.end, b_seg.start, b_seg.end] {
                                        push_aux(o, &mut aux);
                                    }
                                }
                                ConstraintKind::Perpendicular => {
                                    let Some((first, second)) = c.tangent_segments else { continue };
                                    let (Some(a_seg), Some(b_seg)) = (self.doc.segment(first), self.doc.segment(second)) else { continue };
                                    let touches = [a_seg.start, a_seg.end, b_seg.start, b_seg.end].contains(&pid);
                                    if !touches { continue; }
                                    for o in [a_seg.start, a_seg.end, b_seg.start, b_seg.end] { push_aux(o, &mut aux); }
                                }
                                _ => {}
                            }
                        }
                    }
                    aux
                };
                let coincident_cluster = |seed: PointId| -> Vec<PointId> {
                    let mut cluster = vec![seed];
                    let mut i = 0;
                    while i < cluster.len() {
                        let pid = cluster[i];
                        for c in &self.doc.constraints {
                            if c.kind != ConstraintKind::Coincident || c.point_on_segment.is_some() {
                                continue;
                            }
                            let other = if c.a == pid {
                                Some(c.b)
                            } else if c.b == pid {
                                Some(c.a)
                            } else {
                                None
                            };
                            if let Some(other) = other
                                && !cluster.contains(&other)
                                && self.doc.point(other).is_some()
                            {
                                cluster.push(other);
                            }
                        }
                        i += 1;
                    }
                    cluster
                };
                let (drag_pts, aux_pts) = if let Some(pid) = solo_point {
                    // Arc roles first — they override the generic cluster path:
                    //  - CENTER drag translates the ENTIRE arc rigidly (plus
                    //    any coincident partners glued to the center);
                    //  - CTRL (on-curve point) drag reshapes: only the glued
                    //    cluster moves and the solver re-seats the rest.
                    let arc_as_center = self.doc.all_segments().find(|(_, s)| {
                        s.kind == crate::core::document::SegmentKind::Arc && s.center == Some(pid)
                    }).map(|(_, s)| s);
                    let arc_as_ctrl = self.doc.all_segments().find(|(_, s)| {
                        s.kind == crate::core::document::SegmentKind::Arc && s.ctrl == Some(pid)
                    }).map(|(_, s)| s);
                    if let Some(seg) = arc_as_center {
                        let mut ids = vec![seg.start, seg.end, pid];
                        if let Some(c) = seg.ctrl {
                            ids.push(c);
                        }
                        for p in coincident_cluster(pid) {
                            if !ids.contains(&p) {
                                ids.push(p);
                            }
                        }
                        let aux = ring_of(&ids);
                        let drag = ids
                            .iter()
                            .filter_map(|&p| self.doc.point(p).map(|pos| (p, pos)))
                            .collect();
                        (drag, aux)
                    } else if arc_as_ctrl.is_some() {
                        let cluster = coincident_cluster(pid);
                        let aux = ring_of(&cluster);
                        let drag = cluster
                            .iter()
                            .filter_map(|&p| self.doc.point(p).map(|pos| (p, pos)))
                            .collect();
                        (drag, aux)
                    } else {
                    let cluster = coincident_cluster(pid);
                    let mut ids = cluster.clone();
                    // If pid is a bezier endpoint, its attached near control handle moves with it
                    // so the handle vector does not invert or collapse.
                    for (_, s) in self.doc.all_segments() {
                        if s.kind == crate::core::document::SegmentKind::Bezier {
                            if cluster.contains(&s.start) {
                                if let Some(h) = s.ctrl && !ids.contains(&h) {
                                    ids.push(h);
                                }
                            }
                            if cluster.contains(&s.end) {
                                if let Some(h) = s.center && !ids.contains(&h) {
                                    ids.push(h);
                                }
                            }
                        }
                    }
                    let drag = ids
                        .iter()
                        .filter_map(|&p| self.doc.point(p).map(|pos| (p, pos)))
                        .collect();
                    let aux = ring_of(&ids);
                    (drag, aux)
                    }
                } else if let (ElementRef::Segment(sid), false) =
                    (&el, self.selection.len() > 1 && self.element_selected(el))
                {
                    // SOLO segment -> edge-stretch / translation (filleted
                    // edges included: the follower completion + refresh
                    // keep the fillet riding along).
                    let seg = self.doc.segment(*sid);
                    if seg.is_some_and(|s| s.kind == crate::core::document::SegmentKind::Arc) {
                        // Fillet arcs are derived geometry, never kinematic
                        // bodies: with the value input still open the drag
                        // drives its number (radius resize); otherwise the
                        // whole corner moves like a corner-point drag
                        // (dim or not — free fillets ride at stored radius).
                        if let Some(index) = self
                            .doc
                            .modifiers
                            .iter()
                            .position(|m| m.arc == Some(*sid))
                        {
                            let provisional = self.doc.modifiers[index]
                                .arc
                                .and_then(|arc| {
                                    self.doc.dimensions.iter().enumerate().find(|(_, d)| {
                                        matches!(d.target, DimTarget::Radius { seg } if seg == arc)
                                    })
                                })
                                .is_some_and(|(idx, _)| {
                                    self.dim_input
                                        .as_ref()
                                        .is_some_and(|input| input.existing == Some(idx))
                                });
                            if provisional {
                                return self.route_fillet_grab(index, p);
                            }
                            return self.begin_fillet_corner_drag(index, p);
                        }
                        let s = seg.unwrap();
                        let mut ids = vec![s.start, s.end];
                        if let Some(c) = s.ctrl {
                            ids.push(c);
                        }
                        if let Some(c) = s.center {
                            ids.push(c);
                        }
                        let drag = ids
                            .iter()
                            .filter_map(|&p| self.doc.point(p).map(|pos| (p, pos)))
                            .collect();
                        (drag, Vec::new())
                    } else if seg.is_some_and(|s| s.kind == crate::core::document::SegmentKind::Bezier) {
                        let s = seg.unwrap();
                        let mut ids = vec![s.start, s.end];
                        if let Some(c) = s.ctrl {
                            ids.push(c);
                        }
                        if let Some(c) = s.center {
                            ids.push(c);
                        }
                        let drag = ids
                            .iter()
                            .filter_map(|&p| self.doc.point(p).map(|pos| (p, pos)))
                            .collect();
                        (drag, Vec::new())
                    } else {
                        let mut ends: Vec<PointId> = seg
                            .map(|s| vec![s.start, s.end])
                            .unwrap_or_default();
                        // Fillet source edge: the tangent endpoint is
                        // fillet-owned (refresh re-derives it from the
                        // corner + radius every frame), so only the far
                        // end drives. Dragging both rigidly fights the
                        // refresh and jitters. Legacy (unsubdivided)
                        // tangent points aren't endpoints — untouched.
                        if let Some(m) = self.doc.modifiers.iter().find(|m| {
                            m.first == *sid || m.second == *sid
                        }) {
                            let tangent_ends: Vec<PointId> = [
                                m.first_tangent,
                                m.second_tangent,
                            ]
                            .into_iter()
                            .flatten()
                            .collect();
                            if ends.iter().any(|e| tangent_ends.contains(e)) {
                                ends.retain(|e| !tangent_ends.contains(e));
                            }
                        }
                        let drag = ends
                            .iter()
                            .filter_map(|&pid| self.doc.point(pid).map(|pos| (pid, pos)))
                            .collect();
                        (drag, Vec::new())
                    }
                } else {
                    // Group/region drags chase every selected point rigidly,
                    // fillet derived points included: excluding them strands
                    // linking constraints (a dragged neighbor chases while
                    // the tangent point is anchored → unsatisfiable →
                    // frozen whole-object moves). Direct tangent grabs never
                    // reach here anyway (press-time routing turns them into
                    // fillet gestures); refresh confirms rigid translations
                    // exactly, so nothing fights.
                    let pts = self.doc.selection_points(&self.selection);
                    let drag = pts
                        .iter()
                        .filter_map(|&pid| self.doc.point(pid).map(|pos| (pid, pos)))
                        .collect();
                    (drag, Vec::new())
                };
                // A fillet radius input left open from placement would sit
                // editing-highlighted over every subsequent drag — commit
                // it implicitly (values are already stored) by closing it
                // when a geometry drag starts. Fillet gestures manage
                // their own input state and never reach here.
                let stale_fillet_input = self.dim_input.as_ref().is_some_and(|input| {
                    matches!(input.target, DimTarget::Radius { seg } if self
                        .doc
                        .modifiers
                        .iter()
                        .any(|m| m.arc == Some(seg)))
                });
                if stale_fillet_input {
                    self.dim_input = None;
                }
                // Arc body grabs scale about the fixed center (kinematic);
                // everything else takes the solver path.
                let arc_body_scale = match el {
                    ElementRef::Segment(sid)
                        if !(self.selection.len() > 1 && self.element_selected(el))
                            && self
                                .doc
                                .segment(sid)
                                .is_some_and(|s| s.kind == crate::core::document::SegmentKind::Arc) =>
                    {
                        Some(sid)
                    }
                    _ => None,
                };
                self.dragging = Some(DragState {
                    points: drag_pts,
                    aux: aux_pts,
                    start_cursor: p,
                    arc_body_scale,
                });
                true
            }
            None => {
                if !shift {
                    self.selection.clear();
                }
                self.marquee = Some((p, p));
                true
            }
        }
    }

    pub fn canvas_drag(&mut self, cursor: gpui::Point<gpui::Pixels>, shift: bool) -> bool {
        self.last_cursor = Some(cursor);
        self.shift = shift;
        // MMB pan wins over EVERY tool (dimension preview, rubber bands,
        // placed-dim drags): holding the middle button always pans.
        if self.pan_delta(cursor) {
            self.snap_guides.clear();
            return true;
        }
        // The snap crosshair tracks every move (idle or drag-out) so it's
        // always glued to the cursor when a creation tool is active.
        let mut changed = self.update_creation_cursor(cursor);
        // Dimension placement preview follows the cursor — every move is a
        // repaint, no exceptions. Without this the preview only updated
        // when some other event happened to trigger a frame (the lag).
        if self.tool == Tool::Dimension
            && self.dim_input.is_none()
            && self.dim_drag.is_none()
        {
            return true;
        }
        // Dragging a placed dimension's container repositions it.
        if self.dim_drag.is_some() {
            let cur = self.cursor_doc(cursor);
            let mut moved = false;
            if let Some(drag) = &mut self.dim_drag {
                if pick::distance(cur, drag.down_doc) * self.camera.zoom > 3. {
                    drag.moved = true;
                    moved = true;
                }
            }
            if moved {
                if let Some(idx) = self.dim_drag.as_ref().map(|d| d.index) {
                    self.dim_drag_update(idx, cursor);
                }
            }
            return true;
        }
        // Staged fillet-handle press: hold for a click, convert to the
        // routed gesture past the click threshold (then fall through to
        // the radius/corner blocks below in the same frame).
        if let Some(press) = self.fillet_press {
            let cur = self.cursor_doc(cursor);
            if pick::distance(cur, press.down) * self.camera.zoom <= 3. {
                return true;
            }
            self.fillet_press = None;
            self.route_fillet_grab(press.modifier_index, press.down);
        }
        // Fillet radius drag: cursor displacement from the grab resizes
        // the grabbed fillet. Consumed while active so no other gesture
        // path acts on the stale press.
        if self.fillet_radius_drag.is_some() {
            let cur = self.cursor_doc(cursor);
            self.update_fillet_radius_drag(cur);
            return true;
        }
        // Fillet corner drag: both tangent points chase the cursor
        // (locked-radius resize from the adjacent edges).
        if self.fillet_corner_drag.is_some() {
            let cur = self.cursor_doc(cursor);
            self.update_fillet_corner_drag(cur);
            return true;
        }

        // Rectangle rubber band.
        if self.pending_shape.is_some() {
            let at = self.cursor_doc(cursor);
            let (at, guides) = self.snap_creation_point(at);
            self.snap_guides = guides;
            if let Some(pending) = self.pending_shape.as_mut() {
                pending.cursor = at;
                pending.proportional = shift;
            }
            return true;
        }

        // Ruler rubber band.
        if self.pending_ruler.is_some() {
            let at = self.cursor_doc(cursor);
            let (at, guides) = self.snap_creation_point(at);
            self.snap_guides = guides;
            if let Some(pending) = self.pending_ruler.as_mut() {
                pending.cursor = at;
            }
            return true;
        }

        // Line rubber band.
        if self.pending_line.is_some() {
            let at = self.cursor_doc(cursor);
            let (at, guides) = self.snap_creation_point(at);
            self.snap_guides = guides;
            let tangent_at = self.pending_line.as_ref().and_then(|p| {
                if shift { self.tangent_snap_for_line(p.start, at) } else { None }
            });
            let start = self.pending_line.map(|p| p.start).unwrap_or(at);
            let perpendicular = self.perpendicular_snap_for_line(start, tangent_at.unwrap_or(at));
            let final_at = perpendicular.map(|(point, _)| point).unwrap_or(tangent_at.unwrap_or(at));
            self.perpendicular_preview = perpendicular.map(|(point, sid)| (sid, start, point));
            if let Some(pending) = self.pending_line.as_mut() {
                pending.cursor = final_at;
            }
            return true;
        }

        // Circle rubber band: cursor is the third (on-arc) point.
        if self.pending_circle.is_some() {
            let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
            let info = self.pending_circle.map(|p| (p.stage(), p.a, p.b));
            let (at, shifted) = match info {
                Some((stage, a, b)) => Self::arc_creation_shift(stage, a, b, at, shift),
                None => (at, false),
            };
            // A sweep transform supersedes raw-cursor alignment guides —
            // the arc itself is the constraint now.
            self.snap_guides = if shifted { Vec::new() } else { guides };
            if let Some(pending) = self.pending_circle.as_mut() {
                pending.cursor = at;
            }
            return true;
        }

        // Pen rubber band (unified line/arc/bezier preview).
        if self.pending_pen.is_some() {
            return self.pen_drag_update(cursor, shift);
        }

        if self.dragging.is_none() {
            // Marquee band update.
            if let Some((start, _)) = self.marquee {
                let cur = self.cursor_doc(cursor);
                self.marquee = Some((start, cur));
                return true;
            }
            // Creation tools keep their hover snap-lock guides here —
            // clearing them wiped the crosshair highlight every move.
            // (update_creation_cursor refreshes them above; non-creation
            // tools still clear stale drag leftovers.)
            if !matches!(self.tool, Tool::Line | Tool::Rectangle | Tool::Ruler | Tool::Circle | Tool::Pen) {
                self.snap_guides.clear();
            }
            return changed;
        }

        changed |= self.solve_drag(shift);
        self.refresh_fillets();
        changed |= self.post_handle_drag(shift);
        changed
    }

    /// Bezier handle post-pass, runs after every solved drag frame:
    /// dragging ONE handle mirrors its joint partner across the shared
    /// endpoint (symmetric handles, lengths preserved) and snaps the
    /// dragged direction onto a neighboring span's tangent rail when
    /// close (~10°). Alt-held drags stay fully free. Multi-drags never
    /// touch handles symmetrically (they translate).
    fn post_handle_drag(&mut self, shift: bool) -> bool {
        // Tangent-rail snapping is an explicit modifier action.  Applying it
        // unconditionally made a free handle silently acquire a neighboring
        // span's direction as the cursor passed nearby.
        if self.alt_down || !shift {
            return false;
        }
        let Some(drag) = self.dragging.as_ref() else {
            return false;
        };
        if drag.points.len() != 1 {
            return false;
        }
        let hid = drag.points[0].0;
        // Locate the dragged handle: its span + the joint endpoint.
        let mut found: Option<(SegmentId, PointId)> = None;
        for (sid, s) in self.doc.all_segments() {
            if s.kind != crate::core::document::SegmentKind::Bezier {
                continue;
            }
            if s.ctrl == Some(hid) {
                found = Some((sid, s.start));
                break;
            }
            if s.center == Some(hid) {
                found = Some((sid, s.end));
                break;
            }
        }
        let Some((sid, joint)) = found else {
            return false;
        };
        let (Some(hpos), Some(jpos)) = (self.doc.point(hid), self.doc.point(joint)) else {
            return false;
        };
        let hlen = pick::distance(hpos, jpos);
        if hlen < 1e-6 {
            return false;
        }
        // Tangent rail: any OTHER span touching the joint lends its
        // tangent; snap the dragged direction onto it when close.
        let mut hpos = hpos;
        let mut best_rail: Option<(f64, f64)> = None;
        let mut best_ang = 0.21f64; // ~12°
        for (osid, s) in self.doc.all_segments() {
            if osid == sid {
                continue;
            }
            if !matches!(
                s.kind,
                crate::core::document::SegmentKind::Line
                    | crate::core::document::SegmentKind::Arc
                    | crate::core::document::SegmentKind::Bezier
            ) {
                continue;
            }
            let touches = s.start == joint || s.end == joint;
            if !touches {
                continue;
            }
            // Neighbor direction AWAY from the joint.
            let rail = if s.end == joint {
                self.span_end_tangent(osid)
            } else {
                self.span_start_tangent(osid)
            };
            let Some((rx, ry)) = rail else {
                continue;
            };
            // G1 allows either orientation; snap to the nearer rail side.
            let (dx, dy) = ((hpos.x - jpos.x) / hlen, (hpos.y - jpos.y) / hlen);
            for (sx, sy) in [(rx, ry), (-rx, -ry)] {
                let dot = (dx * sx + dy * sy).clamp(-1., 1.);
                let ang = dot.acos();
                if ang < best_ang {
                    best_ang = ang;
                    best_rail = Some((sx, sy));
                }
            }
        }
        if let Some((rx, ry)) = best_rail {
            hpos = Point2::new(jpos.x + rx * hlen, jpos.y + ry * hlen);
            self.doc.move_point(hid, hpos);
        }
        // Mirror the partner handle (the other handle sharing this joint)
        // across the joint, preserving the partner's own length.
        let mut partner: Option<PointId> = None;
        for (osid, s) in self.doc.all_segments() {
            if s.kind != crate::core::document::SegmentKind::Bezier {
                continue;
            }
            if osid == sid {
                continue;
            }
            if s.start == joint && s.ctrl.is_some_and(|h| h != hid) {
                partner = s.ctrl;
                break;
            }
            if s.end == joint && s.center.is_some_and(|h| h != hid) {
                partner = s.center;
                break;
            }
        }
        let Some(pid) = partner else {
            return best_rail.is_some();
        };
        let (Some(ppos), ) = (self.doc.point(pid),) else {
            return best_rail.is_some();
        };
        let plen = pick::distance(ppos, jpos);
        if plen < 1e-6 {
            return best_rail.is_some();
        }
        let (dx, dy) = (hpos.x - jpos.x, hpos.y - jpos.y);
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        self.doc.move_point(
            pid,
            Point2::new(jpos.x - dx / l * plen, jpos.y - dy / l * plen),
        );
        true
    }

    pub fn canvas_hover(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        // Creation tools: the crosshair itself snap-locks and highlights
        // targets BEFORE any button press.
        match self.tool {
            Tool::Line | Tool::Rectangle | Tool::Ruler | Tool::Pen
                if self.pending_shape.is_none()
                    && self.pending_line.is_none()
                    && self.pending_ruler.is_none()
                    && self.pending_pen.is_none()
                    && self.pending_bezier.is_none() =>
            {
                let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                let changed = match (&self.snap_guides, &guides) {
                    (a, b) if a.len() == b.len() => a.iter().zip(b.iter()).any(|(x, y)| {
                        x.kind != y.kind || pick::distance(x.to, y.to) > 1e-9
                    }),
                    _ => true,
                };
                self.snap_guides = guides;
                return changed;
            }
            _ => {}
        }
        // Dimension tool: hovering still highlights pickable points/lines
        // (the picker uses `hover` for its own affordances below), but no
        // resize-handle logic runs — nothing moves in this tool.
        if self.tool == Tool::Dimension {
            if self.dragging.is_some() || self.pan_start.is_some() {
                return false;
            }
            let picked = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX)
                .element(self.cursor_doc(cursor));
            let changed = self.hover != picked;
            self.hover = picked;
            // Placed-dim container hover (recolor) tracked separately.
            let hdim = self.dim_at(cursor);
            let changed = changed || self.hovered_dim != hdim;
            self.hovered_dim = hdim;
            return changed;
        }
        if self.is_constraint_tool() {
            let picked = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX)
                .element(self.cursor_doc(cursor))
                .and_then(|element| self.normalize_constraint_hit(element));
            let changed = self.hover != picked;
            self.hover = picked;
            return changed;
        }
        // Fillet tool: hovering highlights pickable edges/points exactly
        // like the Dimension tool does, and additionally refreshes the
        // fillet preview.
        if self.tool == Tool::Fillet {
            if self.dragging.is_some() || self.pan_start.is_some() { return false; }
            let picked = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX)
                .element(self.cursor_doc(cursor));
            let mut changed = self.hover != picked;
            self.hover = picked;
            changed |= self.update_fillet_preview(cursor);
            return changed;
        }
        if self.tool != Tool::Move || self.dragging.is_some() || self.pan_start.is_some() {
            return false;
        }
        // Chips never block geometry hover (a chip hovering used to make
        // the line's hover highlight flash like crazy). The cursor chip is
        // tracked ONLY for the chip's own hover styling.
        let mut changed = self.update_chip_hover(cursor);
        let p = self.cursor_doc(cursor);
        let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        let info = picker.element(p);
        if self.hover != info {
            self.hover = info;
            changed = true;
        }
        // Placed-dim container hover (recolor on hover).
        let hdim = self.dim_at(cursor);
        if self.hovered_dim != hdim {
            self.hovered_dim = hdim;
            changed = true;
        }
        changed
    }

    fn constraint_hover_allowed(&self, element: ElementRef) -> bool {
        match self.tool {
            Tool::ConstraintHorizontalVertical => {
                matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line))
            }
            Tool::ConstraintTangent => {
                matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| matches!(s.kind, crate::core::document::SegmentKind::Line | crate::core::document::SegmentKind::Arc | crate::core::document::SegmentKind::Bezier)))
            }
            Tool::ConstraintCoincident => matches!(element, ElementRef::Point(_) | ElementRef::Segment(_)),
            Tool::ConstraintParallel => matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line)),
            Tool::ConstraintPerpendicular => matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line)),
            _ => false,
        }
    }

    pub(crate) fn normalize_constraint_hit(&self, element: ElementRef) -> Option<ElementRef> {
        if self.constraint_hover_allowed(element) {
            return Some(element);
        }
        let ElementRef::Point(point) = element else { return None };
        match self.tool {
            Tool::ConstraintHorizontalVertical | Tool::ConstraintTangent | Tool::ConstraintPerpendicular => self
                .doc
                .all_segments()
                .filter(|(_, s)| {
                    let allowed = match self.tool {
                        Tool::ConstraintHorizontalVertical => s.kind == crate::core::document::SegmentKind::Line,
                        Tool::ConstraintTangent => matches!(s.kind, crate::core::document::SegmentKind::Line | crate::core::document::SegmentKind::Arc | crate::core::document::SegmentKind::Bezier),
                        Tool::ConstraintPerpendicular => s.kind == crate::core::document::SegmentKind::Line,
                        _ => false,
                    };
                    allowed && (s.start == point || s.end == point || s.ctrl == Some(point))
                })
                .map(|(sid, _)| ElementRef::Segment(sid))
                .find(|candidate| !self.constraint_picks.contains(candidate)),
            _ => None,
        }
    }

    /// Tracks which chip (if any) is under the cursor for its own hover
    /// styling. Returns true if that changed.
    fn update_chip_hover(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        let key = self
            .constraint_chip_at(cursor)
            .map(|c| format!("{c:?}"));
        if self.hovered_constraint == key {
            return false;
        }
        self.hovered_constraint = key;
        true
    }

    /// The visible constraint chip under a screen-space cursor, if any.
    pub fn constraint_chip_at(
        &self,
        cursor: gpui::Point<gpui::Pixels>,
    ) -> Option<crate::core::constraints::Constraint> {
        let (x, y) = (f64::from(cursor.x) as f32, f64::from(cursor.y) as f32);
        const HALF: f32 = crate::ui::canvas::CHIP_SIZE / 2.;
        self.constraint_markers
            .iter()
            .filter(|m| m.visible)
            .find(|m| (m.cx_out - x).abs() <= HALF && (m.cy_out - y).abs() <= HALF)
            .map(|m| m.constraint)
    }

    pub fn cursor_style(&self) -> gpui::CursorStyle {
        use gpui::CursorStyle;
        if self.pan_start.is_some() {
            return CursorStyle::ClosedHand;
        }
        if self.tool == Tool::Pan {
            return CursorStyle::OpenHand;
        }
        // Creation tools keep the idle ARROW cursor: the drawn crosshair
        // (CanvasView::snap_cursor_layer) is the makeshift snapping cursor.
        CursorStyle::Arrow
    }

    /// Recomputes the drawn snap-cursor state for creation tools: position
    /// of the crosshair (the snapped point — detached from the raw cursor
    /// while locked) plus whether a snap is engaged. Returns true if the
    /// state changed (repaint needed). Always runs while a creation tool is
    /// active so the crosshair tracks every mouse move; hides itself while
    /// panning and for non-creation tools.
    fn update_creation_cursor(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        let is_creation = matches!(
            self.tool,
            Tool::Rectangle | Tool::Line | Tool::Ruler | Tool::Circle | Tool::Pen
        );
        if !is_creation || self.pan_start.is_some() {
            if self.creation_cursor.is_some() {
                self.creation_cursor = None;
                return true;
            }
            return false;
        }
        let (pos, guides) = self.snap_creation_point(self.cursor_doc(cursor));
        // Tangent lock mirrors the preview exactly: shift-gated arc/line
        // tangents (legacy behavior), plus the free on-curve bezier
        // continuation. Anything else leaves the plain snap crosshair.
        // Bezier staging never detaches the crosshair (the endpoint press
        // + drag owns the handles; a tangent ray would fight it and read
        // as the cursor "following the handle axis"). Shift still arms
        // the arc-tangent lock for every mode.
        let anchor = if self.tool == Tool::Pen {
            let bezier_active = self
                .pending_pen
                .is_some_and(|p| p.mode == PenMode::Bezier && p.bezier.is_some());
            if bezier_active && !self.shift {
                None
            } else {
                self.pending_pen.and_then(|p| match p.mode {
                    PenMode::Line => p.line.map(|l| l.start),
                    PenMode::Bezier => p.bezier.map(|b| b.p0),
                    PenMode::Arc => p.circle.and_then(|c| c.a),
                })
            }
        } else if self.tool == Tool::Line && self.shift {
            self.pending_line.map(|p| p.start)
        } else {
            None
        };
        if let Some(start) = anchor {
            let tan = if self.shift {
                self.tangent_snap_for_line(start, pos)
                    .or_else(|| self.bezier_tangent_snap(start, pos))
            } else {
                None
            };
            if let Some(at) = tan {
                self.snap_guides.clear();
                let next = Some((at.x, at.y, true));
                let changed = self.creation_cursor != next;
                self.creation_cursor = next;
                return changed;
            }
        }
        // Apply the arc-tool shift constraints so the crosshair matches the
        // pending preview exactly; a sweep transform invalidates the raw
        // cursor's alignment guides (the arc IS the constraint now).
        // Covers the legacy circle tool AND the pen's arc staging.
        let pending = self
            .pending_circle
            .map(|p| (p.stage(), p.a, p.b))
            .or_else(|| {
                self.pending_pen.and_then(|p| {
                    p.circle.map(|c| (c.stage(), c.a, c.b))
                })
            });
        let at = if let Some((stage, a, b)) = pending {
            let (at, shifted) = Self::arc_creation_shift(stage, a, b, pos, self.shift);
            if shifted {
                self.snap_guides.clear();
                let next = Some((at.x, at.y, false));
                let changed = self.creation_cursor != next;
                self.creation_cursor = next;
                return changed;
            }
            at
        } else {
            pos
        };
        // The badge means "fully locked onto a feature" — one-axis
        // alignments draw their connection lines but never light it up.
        let solid = guides.iter().any(|g| g.solid);
        let next = Some((at.x, at.y, solid));
        let mut changed = self.creation_cursor != next;
        self.creation_cursor = next;
        // Publish the guides on idle hover too — drag-out branches below
        // refresh them, but plain hovering never populated snap_guides, so
        // the dashed stubs to the snap target only existed mid-drag.
        if self.snap_guides != guides {
            self.snap_guides = guides;
            changed = true;
        }
        changed
    }

    pub fn canvas_up(&mut self, button: gpui::MouseButton, shift: bool) -> bool {
        // A drag that ended with points sitting on other points BONDS them:
        // a Coincident constraint glues the pair (solver-enforced, shown as
        // a deletable chip).
        if self.dragging.is_some() {
            self.queue_bond_menu();
        }
        // Panning ends on release of EITHER panning button - a stuck
        // pan_start made the camera chase the cursor forever.
        if (button == gpui::MouseButton::Left || button == gpui::MouseButton::Middle)
            && self.end_pan()
        {
            return true;
        }
        if button != gpui::MouseButton::Left {
            return false;
        }
        // Placed-dimension drag ends here: promote the snapshot (autosave
        // watches the generation bump). A CLEAN release (no movement) on an
        // already-selected dim is a second tap -> enter its value input.
        if let Some(drag) = self.dim_drag.take() {
            self.flush_pending_history();
            if !drag.moved && drag.was_selected {
                self.begin_dim_edit(drag.index);
            }
            return true;
        }
        // Fillet radius-drag release: promote the gesture snapshot (one
        // undo step for the whole drag).
        if self.fillet_radius_drag.take().is_some() {
            self.flush_pending_history();
            return true;
        }
        // Fillet corner-drag release: same single-undo-step promotion.
        if self.fillet_corner_drag.take().is_some() {
            self.flush_pending_history();
            return true;
        }
        // Staged fillet-handle press released clean: emulate the click
        // the staging consumed (single-click select semantics; no
        // double-click escalation without a count at release).
        if let Some(press) = self.fillet_press.take() {
            let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
            if let Some(el) = picker.element(press.down) {
                if !self.element_selected(el) {
                    if self.marquee_add {
                        self.selection.push(el);
                    } else {
                        self.selection = vec![el];
                    }
                }
            } else if !self.marquee_add {
                self.selection.clear();
            }
            return true;
        }
        if self.dragging.is_some() {
            self.enforce_dragged_curve_lengths();
        }
        self.dragging = None;
        self.snap_guides.clear();
        self.group_drag_last = None;
        // Gesture over: promote the history snapshot now (autosave watches
        // the generation bump) instead of waiting for the next gesture.
        self.flush_pending_history();

        // Marquee finalize.
        if let Some((a, b)) = self.marquee.take() {
            let band = Rect::from_points(a, b);
            if band.size.w > 1e-9 || band.size.h > 1e-9 {
                let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
                let picked = picker.marquee(band);
                if self.marquee_add {
                    for el in picked {
                        if !self.selection.contains(&el) {
                            self.selection.push(el);
                        }
                    }
                } else {
                    self.selection = picked;
                }
                self.marquee_add = false;
                self.deferred_pick = None;
                return true;
            }
            // Band never grew: a click on tolerant-only geometry.
            if let Some(el) = self.deferred_pick.take() {
                if self.marquee_add {
                    if !self.selection.contains(&el) {
                        self.selection.push(el);
                    }
                } else {
                    self.selection = vec![el];
                }
                self.marquee_add = false;
                return true;
            }
            self.marquee_add = false;
        }

        // Pen release: drag-release commits the live span (line mirrors
        // the legacy line tool; bezier commits its endpoint/handle
        // staging). Pure clicks keep staging for the next press.
        if self.tool == Tool::Pen && self.pending_pen.is_some() {
            if self.pen_release_commit(shift) {
                return true;
            }
        }
        // Click-created pending shapes survive mouse-up ONLY when the
        // cursor never moved (a true click); any real drag commits on
        // release. Only the no-motion case waits for the next click.
        if let Some(pending) = self.pending_line.take() {
            let (_, mut b) = pending.snapped(shift);
            if shift {
                if let Some(q) = self.tangent_snap_for_line(pending.start, pending.cursor) { b = q; }
            }
            if self.pending_via_click && pick::distance(b, pending.start) <= 1e-6 {
                self.pending_line = Some(pending);
                return true;
            }
            // Drag-release commit: chain the next line from this endpoint,
            // staying in line mode for continuous drawing.
            self.pending_via_click = true;
            self.snap_guides.clear();
            if pick::distance(b, pending.start) > 1e-6 {
                let layer_id = self.doc.layers[0].id;
                let seg = self.create_line(layer_id, pending.start, b);
                if let Some((source, _, _)) = self.perpendicular_preview.take() {
                    self.doc.add_perpendicular_constraint(source, seg);
                }
                if shift {
                    self.maybe_add_tangent(seg, b);
                }
                self.selection = vec![ElementRef::Segment(seg)];
                self.pending_line = Some(PendingLine { start: b, cursor: b });
            } else {
                self.pending_line = Some(pending);
            }
            return true;
        }
        if let Some(pending) = self.pending_ruler.take() {
            let (_, b) = pending.snapped(shift);
            if self.pending_via_click && pick::distance(b, pending.start) <= 1e-6 {
                self.pending_ruler = Some(pending);
                return true;
            }
            self.pending_via_click = false;
            self.tool = Tool::Move;
            self.creation_cursor = None;
            if pick::distance(b, pending.start) > 1e-6 {
                let layer_id = self.doc.layers[0].id;
                let seg = self.create_ruler(layer_id, pending.start, b);
                self.selection = vec![ElementRef::Segment(seg)];
            }
            return true;
        }
        let Some(pending) = self.pending_shape.take() else {
            return false;
        };
        // Click without motion: keep pending, commit on next click.
        if self.pending_via_click
            && (pending.cursor.x - pending.start.x).abs() < 1e-9
            && (pending.cursor.y - pending.start.y).abs() < 1e-9
        {
            self.pending_shape = Some(pending);
            return true;
        }
        self.pending_via_click = false;
        let b = pending.bounds();
        self.tool = Tool::Move;
        self.creation_cursor = None;
        if b.size.w > 0. && b.size.h > 0. {
            let layer_id = self.doc.layers[0].id;
            let fill = self.create_rectangle(
                layer_id,
                b.origin,
                Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h),
            );
            self.selection = vec![ElementRef::Fill(fill)];
        }
        true
    }
}
