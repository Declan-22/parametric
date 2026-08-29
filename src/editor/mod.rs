pub mod arc;
mod clipboard;
mod camera;
pub mod dims;
pub mod grid;
pub mod pick;
pub mod ruler;
mod snapping;
mod tools;

pub use camera::Camera;

pub use snapping::SnapGuide;
pub use tools::{PendingCircle, PendingLine, PendingRuler, PendingShape, Tool};

use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::core::document::{Document, Layer};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{FillId, PointId};

// The session: the permanent design plus view/editing state.
// Owns nothing about GPUI widgets; the UI layer drives it.
//
// Subsystems live in sibling modules:
//   tools    - tool enum + per-tool pending drag geometry
//   pick     - unified hit-testing (the ONE notion of "under the cursor")
//   snapping - snap candidates, best-match search, visual guides
//   dims     - dimension render-data computation
//   ruler    - the ruler component's procedural vector design

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

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
}

pub struct Editor {
    pub doc: Document,
    pub camera: Camera,
    pub tool: Tool,
    pub pending_shape: Option<PendingShape>,
    pub pending_ruler: Option<PendingRuler>,
    pub pending_line: Option<PendingLine>,
    pub pending_circle: Option<PendingCircle>,
    // Pending shape created by a single click (commit on next click).
    pub pending_via_click: bool,
    pub selection: Vec<ElementRef>,
    // Selected constraint chips (identity = the Constraint value).
    pub selected_constraints: Vec<crate::core::constraints::Constraint>,
    pub hover: Option<ElementRef>,
    // Rubber-band marquee: (start doc, current doc).
    pub marquee: Option<(Point2, Point2)>,
    // Shift held at marquee start: extend the selection instead of replacing.
    pub marquee_add: bool,
    // Tolerant-only hit awaiting mouse-up: becomes a click-select when the
    // band never grew, else the marquee result takes over.
    pub(crate) deferred_pick: Option<ElementRef>,
    pub group_drag_last: Option<Point2>,
    pub snap_guides: Vec<SnapGuide>,
    // Per-frame dimension render data.
    pub dim_renders: Vec<dims::DimRender>,
    // Per-frame constraint chip render data.
    pub constraint_markers: Vec<dims::ConstraintMarker>,
    // Chip currently under the cursor (hit-tested in screen px).
    pub hovered_constraint: Option<String>,
    // Pending snap-bond choice menu: points ended on top of other points.
    pub context_menu: Option<crate::ui::canvas::context_menu::ContextMenu>,
    // Pairs awaiting the user's bond choice while the menu is open.
    pub pending_bonds: Vec<(PointId, PointId)>,
    // Canvas context menu hover/pop fades — stored on Editor (not Shell) so
    // reading them during Shell::render doesn't re-entrantly borrow Shell.
    pub(crate) context_menu_fades: std::collections::HashMap<String, f32>,
    pub(crate) context_menu_fade_pending: std::collections::HashMap<String, f32>,
    pub(crate) context_menu_fade_active: std::collections::HashSet<String>,
    pub(crate) context_menu_pop: f32,
    // Undo/redo history (full-document snapshots; commands/ module drives).
    pub(crate) undo_stack: Vec<Document>,
    pub(crate) redo_stack: Vec<Document>,
    pub(crate) gesture_snapshot: Option<Document>,
    // Last known cursor + modifier state so changes can re-derive drags.
    pub last_cursor: Option<gpui::Point<gpui::Pixels>>,
    // Last known canvas size in px, for viewport-culled snapping.
    pub viewport_size: (f64, f64),
    pub shift: bool,
    pub alt_down: bool,
    pub(crate) dragging: Option<DragState>,
    next_layer_id: u64,
    pan_start: Option<(gpui::Pixels, gpui::Pixels, Camera)>,
    // Canvas grid + snapping (phase 2). The grid size is FIXED
    // (grid::GRID_BASE — not a setting); only visibility and the snap
    // toggles are user-facing.
    pub show_grid: bool,
    pub snap_to_grid: bool,
    pub snap_to_objects: bool,
    // Creation-tool snap cursor: position (canvas-local px) of the drawn
    // crosshair plus whether it's currently locked onto a target. The OS
    // cursor stays a plain arrow; this is the makeshift snapping cursor.
    // None when no creation tool is active or while panning.
    pub creation_cursor: Option<(f32, f32, bool)>,
}

const HANDLE_TOL_PX: f64 = 14.0;
// Tight tolerance for press-to-grab: inside this, a drag moves geometry;
// outside it (but within HANDLE_TOL_PX), a drag is a marquee.
const EXACT_TOL_PX: f64 = 7.0;
const SNAP_TOL_PX: f64 = 10.0;

impl Editor {
    pub fn new() -> Self {
        let mut doc = Document::new();
        doc.layers.push(Layer {
            id: 1,
            name: "Layer 1".into(),
            elements: Vec::new(),
        });
        Self::from_document(doc)
    }

    pub fn from_document(doc: Document) -> Self {
        let next_layer_id = doc.layers.iter().map(|l| l.id + 1).max().unwrap_or(1);
        Self {
            doc,
            camera: Camera::new(),
            tool: Tool::Move,
            pending_shape: None,
            pending_ruler: None,
            pending_line: None,
            pending_circle: None,
            pending_via_click: false,
            selection: Vec::new(),
            selected_constraints: Vec::new(),
            hover: None,
            marquee: None,
            marquee_add: false,
            deferred_pick: None,
            group_drag_last: None,
            snap_guides: Vec::new(),
            dim_renders: Vec::new(),
            constraint_markers: Vec::new(),
            hovered_constraint: None,
            context_menu: None,
            pending_bonds: Vec::new(),
            context_menu_fades: std::collections::HashMap::new(),
            context_menu_fade_pending: std::collections::HashMap::new(),
            context_menu_fade_active: std::collections::HashSet::new(),
            context_menu_pop: 0.0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            gesture_snapshot: None,
            last_cursor: None,
            viewport_size: (0., 0.),
            shift: false,
            alt_down: false,
            dragging: None,
            next_layer_id: next_layer_id.max(2),
            pan_start: None,
            show_grid: true,
            snap_to_grid: false,
            snap_to_objects: true,
            creation_cursor: None,
        }
    }

    pub fn set_tool(&mut self, tool: Tool) -> bool {
        if self.tool == tool {
            return false;
        }
        self.tool = tool;
        // The snap crosshair belongs to creation tools only; it reappears
        // (freshly positioned) on the first mouse move over the canvas.
        self.creation_cursor = None;
        self.pending_shape = None;
        self.pending_ruler = None;
        self.pending_line = None;
        self.pending_circle = None;
        self.pending_via_click = false;
        self.selection.clear();
        self.selected_constraints.clear();
        self.context_menu = None;
        self.pending_bonds = Vec::new();
        self.marquee = None;
        self.group_drag_last = None;
        self.dragging = None;
        true
    }

    // True while no drag or pan is in progress (gates hover tracking).
    pub fn is_idle(&self) -> bool {
        self.pan_start.is_none() && self.dragging.is_none()
    }

    // -- canvas input (called from the canvas view) --

    fn cursor_doc(&self, cursor: gpui::Point<gpui::Pixels>) -> Point2 {
        self.camera
            .screen_to_unit(Point2::new(f64::from(cursor.x), f64::from(cursor.y)))
    }

    fn snap_tol_doc(&self) -> f64 {
        SNAP_TOL_PX / self.camera.zoom
    }

    /// Visible doc region expanded by a margin — the snap search space.
    /// Snapping only considers nearby, on-screen geometry. Falls back to
    /// unbounded when the viewport size isn't known yet.
    fn snap_visible(&self) -> Rect {
        const MARGIN_PX: f64 = 80.;
        if self.viewport_size.0 <= 0. || self.viewport_size.1 <= 0. {
            return Rect::from_points(
                Point2::new(-1e9, -1e9),
                Point2::new(1e9, 1e9),
            );
        }
        let mut v = self.visible_bounds(Size {
            w: self.viewport_size.0,
            h: self.viewport_size.1,
        });
        let m = MARGIN_PX / self.camera.zoom;
        let min = Point2::new(v.origin.x - m, v.origin.y - m);
        let max = Point2::new(
            v.origin.x + v.size.w + m,
            v.origin.y + v.size.h + m,
        );
        v = Rect::from_points(min, max);
        v
    }

    /// The drawn grid's snap lattice step right now (doc units). Snap
    /// targets are exactly the intersections you can SEE: the fixed base
    /// grid subdivides 5x per level as you zoom, and `grid::snap_step`
    /// returns the finest currently-drawn level.
    fn grid_step(&self) -> Option<f64> {
        if self.snap_to_grid {
            Some(grid::snap_step(self.camera.zoom))
        } else {
            None
        }
    }

    /// Creation-tool cursor snapping — Fusion-style combined snapping:
    ///   1. OBJECTS FIRST, all-or-nothing: nearest endpoint > arc center >
    ///      midpoint > edge body > arc body within tolerance locks BOTH
    ///      axes (works whether Snap to Grid is on or off);
    ///   2. otherwise, per-axis: nearest point coordinate / axis-aligned
    ///      edge span can lock ONE axis;
    ///   3. axes still free go to the GRID, intersections only: both axes
    ///      free snaps to the nearest lattice crossing (both within tol);
    ///      one axis object-locked snaps the other to the nearest grid
    ///      line, so you ride object edges landing exactly on crossings;
    ///   4. whatever remains stays free — between intersections the
    ///      cursor is never yanked along a grid line.
    fn snap_creation_point(&self, p: Point2) -> (Point2, Vec<SnapGuide>) {
        snapping::cursor_snap_combined(
            &self.doc,
            self.snap_tol_doc(),
            p,
            self.snap_visible(),
            self.grid_step(),
            self.snap_to_objects,
            self.camera.zoom,
        )
    }

    // Mouse down on the canvas. Returns true if a repaint is needed.
    pub fn canvas_down(
        &mut self,
        button: gpui::MouseButton,
        cursor: gpui::Point<gpui::Pixels>,
        shift: bool,
        click_count: usize,
    ) -> bool {
        // Every gesture is one undo step; the snapshot commits lazily only
        // if the document actually changed.
        self.history_begin();
        // Any click dismisses the pending bond-choice menu first.
        if self.context_menu.take().is_some() {
            return true;
        }
        match button {
            gpui::MouseButton::Middle => {
                self.begin_pan(cursor);
                true
            }
            gpui::MouseButton::Left => {
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
                    // Second click commits a click-created pending line.
                    if let Some(pending) = self.pending_line.take() {
                        self.pending_via_click = false;
                        self.snap_guides.clear();
                        self.tool = Tool::Move;
                        self.creation_cursor = None;
                        let (_, b) = pending.snapped(shift);
                        if pick::distance(b, pending.start) > 1e-6 {
                            let layer_id = self.doc.layers[0].id;
                            let seg = self.create_line(layer_id, pending.start, b);
                            self.selection = vec![ElementRef::Segment(seg)];
                        }
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
                    let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    match self.pending_circle.as_mut() {
                        // Second click: fix the chord's far end.
                        Some(p) if p.a.is_some() && p.b.is_none() => {
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
                Tool::Move => self.move_tool_down(cursor, shift, click_count),
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
                let solo_point = match el {
                    ElementRef::Point(pid) if self.selection.len() == 1 => Some(pid),
                    _ => None,
                };
                let ring_of = |pids: &[PointId]| -> Vec<(PointId, Point2)> {
                    let mut aux: Vec<(PointId, Point2)> = Vec::new();
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
                        }
                    }
                    aux
                };
                let (drag_pts, aux_pts) = if let Some(pid) = solo_point {
                    let start = self.doc.point(pid).unwrap();
                    let aux = ring_of(&[pid]);
                    (vec![(pid, start)], aux)
                } else if let (ElementRef::Segment(sid), false) =
                    (&el, self.selection.len() > 1 && self.element_selected(el))
                {
                    // SOLO segment -> edge-stretch. With a MULTI-selection,
                    // dragging any selected member moves the whole group
                    // instead (see the fallback arm below).
                    // No aux followers here: the H/V constraints alone
                    // collapse an edge drag to a PERPENDICULAR stretch
                    // (tangential pulls are projected out), so grabbing an
                    // edge never translates the shape.
                    let ends: Vec<PointId> = self
                        .doc
                        .segment(*sid)
                        .map(|s| vec![s.start, s.end])
                        .unwrap_or_default();
                    let drag = ends
                        .iter()
                        .filter_map(|&pid| self.doc.point(pid).map(|pos| (pid, pos)))
                        .collect();
                    (drag, Vec::new())
                } else {
                    let pts = self.doc.selection_points(&self.selection);
                    let drag = pts
                        .iter()
                        .filter_map(|&pid| self.doc.point(pid).map(|pos| (pid, pos)))
                        .collect();
                    (drag, Vec::new())
                };
                self.dragging =
                    Some(DragState { points: drag_pts, aux: aux_pts, start_cursor: p });
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
        // The snap crosshair tracks every move (idle or drag-out) so it's
        // always glued to the cursor when a creation tool is active.
        let mut changed = self.update_creation_cursor(cursor);
        if self.pan_delta(cursor) {
            self.snap_guides.clear();
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
            if let Some(pending) = self.pending_line.as_mut() {
                pending.cursor = at;
            }
            return true;
        }

        // Circle rubber band: cursor is the third (on-arc) point.
        if self.pending_circle.is_some() {
            let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
            self.snap_guides = guides;
            if let Some(pending) = self.pending_circle.as_mut() {
                pending.cursor = at;
            }
            return true;
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
            if !matches!(self.tool, Tool::Line | Tool::Rectangle | Tool::Ruler) {
                self.snap_guides.clear();
            }
            return changed;
        }

        changed |= self.solve_drag(shift);
        changed
    }

    // Live constraint-solve drag: cursor targets go in, solved positions
    // come out — geometry satisfies H/V/dimension constraints continuously
    // while dragging.
    fn solve_drag(&mut self, shift: bool) -> bool {
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
        // fill the axes objects leave free.
        let mut proposals_x: Vec<f64> = Vec::new();
        let mut proposals_y: Vec<f64> = Vec::new();
        if !self.alt_down && (self.snap_to_objects || self.snap_to_grid) {
            for &(_pid, start) in &drag.points {
                let target = Point2::new(start.x + delta.x, start.y + delta.y);
                let (adj, _) = snapping::best(
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
                let (adj_x, adj_y) = (adj.x, adj.y);
                if adj_x != 0. {
                    proposals_x.push(adj_x);
                }
                if adj_y != 0. {
                    proposals_y.push(adj_y);
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
        let sx = consensus(&proposals_x).unwrap_or(0.);
        let sy = consensus(&proposals_y).unwrap_or(0.);
        let snapped_delta = Point2::new(delta.x + sx, delta.y + sy);

        let mut targets: Vec<(PointId, Point2)> = drag
            .points
            .iter()
            .map(|&(pid, start)| {
                (pid, Point2::new(start.x + snapped_delta.x, start.y + snapped_delta.y))
            })
            .collect();

        // Shift on a single-endpoint drag: snap the direction to 45-degree
        // steps around the segment's other endpoint.
        if shift && targets.len() == 1 {
            snap_target_direction(&self.doc, &mut targets);
        }

        let solver = crate::core::solver::Solver::build(&self.doc, &targets, &drag.aux);
        let solution = solver.solve();
        let mut moved: std::collections::HashSet<PointId> = std::collections::HashSet::new();
        for (id, pos) in solution.positions {
            moved.insert(id);
            self.doc.move_point(id, pos);
        }
        self.resolve_arcs_after_drag(&moved);
        true
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
        let arcs: Vec<crate::core::document::Segment> = self
            .doc
            .all_segments()
            .filter(|(_, s)| s.kind == crate::core::document::SegmentKind::Arc)
            .map(|(_, s)| s)
            .collect();
        for seg in arcs {
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
                // Center pinned by a constraint: keep all defining points on
                // the circle around it. Radius anchors to whichever defining
                // point the user did NOT move.
                let radius = if !moved.contains(&seg.start) {
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
            // Free center: follow the geometry.
            self.doc.move_point(center_id, old_o);
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

    /// If `p` is near a selected arc's circumcenter, return a drag that
    /// moves the whole arc (all points including center).
    // -- dimensions (delegates to the dims subsystem) --

    pub fn update_dim_geom(&mut self) {
        dims::update(self);
    }

    // -- hover --

    pub fn canvas_hover(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        // Creation tools: the crosshair itself snap-locks and highlights
        // targets BEFORE any button press.
        match self.tool {
            Tool::Line | Tool::Rectangle | Tool::Ruler
                if self.pending_shape.is_none()
                    && self.pending_line.is_none()
                    && self.pending_ruler.is_none() =>
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
        if self.tool != Tool::Move || self.dragging.is_some() || self.pan_start.is_some() {
            return false;
        }
        // Chips never block geometry hover (a chip hovering used to make
        // the line's hover highlight flash like crazy). The cursor chip is
        // tracked ONLY for the chip's own hover styling.
        let changed = self.update_chip_hover(cursor);
        let p = self.cursor_doc(cursor);
        let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        let info = picker.element(p);
        if self.hover != info {
            self.hover = info;
            return true;
        }
        changed
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
            Tool::Rectangle | Tool::Line | Tool::Ruler | Tool::Circle
        );
        let next = if is_creation && self.pan_start.is_none() {
            let (pos, guides) = self.snap_creation_point(self.cursor_doc(cursor));
            Some((pos.x as f32, pos.y as f32, !guides.is_empty()))
        } else {
            None
        };
        if self.creation_cursor == next {
            return false;
        }
        self.creation_cursor = next;
        true
    }

    pub fn canvas_up(&mut self, button: gpui::MouseButton, shift: bool) -> bool {
        // A drag that ended with points sitting on other points BONDS them:
        // a Coincident constraint glues the pair (solver-enforced, shown as
        // a deletable chip).
        if self.dragging.is_some() {
            self.queue_bond_menu();
        }
        // Panning ends on release of EITHER panning button — a stuck
        // pan_start made the camera chase the cursor forever.
        if (button == gpui::MouseButton::Left || button == gpui::MouseButton::Middle)
            && self.end_pan()
        {
            return true;
        }
        if button != gpui::MouseButton::Left {
            return false;
        }
        self.dragging = None;
        self.snap_guides.clear();
        self.group_drag_last = None;

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

        // Click-created pending shapes survive mouse-up ONLY when the
        // cursor never moved (a true click); any real drag commits on
        // release. Only the no-motion case waits for the next click.
        if let Some(pending) = self.pending_line.take() {
            let (_, b) = pending.snapped(shift);
            if self.pending_via_click && pick::distance(b, pending.start) <= 1e-6 {
                self.pending_line = Some(pending);
                return true;
            }
            self.pending_via_click = false;
            self.tool = Tool::Move;
            self.creation_cursor = None;
            if pick::distance(b, pending.start) > 1e-6 {
                let layer_id = self.doc.layers[0].id;
                let seg = self.create_line(layer_id, pending.start, b);
                self.selection = vec![ElementRef::Segment(seg)];
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

    // -- object creation --

    /// Emits a rectangle composite: 4 points, 4 chained segments, H/V
    /// constraints, and a closed-loop fill. Returns the fill id. This is
    /// the ONLY way a rectangle exists — there is no rectangle object.
    pub fn create_rectangle(&mut self, layer_id: u64, a: Point2, c: Point2) -> FillId {
        let tl = self.doc.add_point(a);
        let tr = self.doc.add_point(Point2::new(c.x, a.y));
        let br = self.doc.add_point(c);
        let bl = self.doc.add_point(Point2::new(a.x, c.y));

        let top = self.doc.add_segment(tl, tr);
        let right = self.doc.add_segment(tr, br);
        let bottom = self.doc.add_segment(br, bl);
        let left = self.doc.add_segment(bl, tl);

        self.doc.add_constraint(ConstraintKind::Horizontal, tl, tr);
        self.doc.add_constraint(ConstraintKind::Horizontal, bl, br);
        self.doc.add_constraint(ConstraintKind::Vertical, tl, bl);
        self.doc.add_constraint(ConstraintKind::Vertical, tr, br);

        let fill = self.doc.add_fill(vec![top, right, bottom, left]);

        for el in [
            ElementRef::Point(tl),
            ElementRef::Point(tr),
            ElementRef::Point(br),
            ElementRef::Point(bl),
            ElementRef::Segment(top),
            ElementRef::Segment(right),
            ElementRef::Segment(bottom),
            ElementRef::Segment(left),
            ElementRef::Fill(fill),
        ] {
            self.doc.push_to_layer(layer_id, el);
        }
        fill
    }

    /// Emits a ruler: 2 points + 1 segment of kind Ruler. No constraints,
    /// no fill — it measures and renders, nothing more.
    pub fn create_ruler(
        &mut self,
        layer_id: u64,
        a: Point2,
        b: Point2,
    ) -> crate::core::ids::SegmentId {
        let p1 = self.doc.add_point(a);
        let p2 = self.doc.add_point(b);
        let seg = self.doc.add_segment_kind(p1, p2, crate::core::document::SegmentKind::Ruler);
        self.doc.push_to_layer(layer_id, ElementRef::Point(p1));
        self.doc.push_to_layer(layer_id, ElementRef::Point(p2));
        self.doc.push_to_layer(layer_id, ElementRef::Segment(seg));
        seg
    }

    /// Emits a standalone line: 2 points + 1 stroked segment. No
    /// constraints, no fill — the line tool's output.
    pub fn create_line(
        &mut self,
        layer_id: u64,
        a: Point2,
        b: Point2,
    ) -> crate::core::ids::SegmentId {
        const LINE_STROKE_PX: f64 = 1.0;
        let p1 = self.doc.add_point(a);
        let p2 = self.doc.add_point(b);
        let seg =
            self.doc
                .add_stroked_segment(p1, p2, LINE_STROKE_PX);
        self.doc.push_to_layer(layer_id, ElementRef::Point(p1));
        self.doc.push_to_layer(layer_id, ElementRef::Point(p2));
        self.doc.push_to_layer(layer_id, ElementRef::Segment(seg));
        seg
    }

    /// Emits a circular arc: 3 real points (a, c on-arc control, b) + a
    /// real center point (circumcenter). No constraints, no fill.
    pub fn create_arc(
        &mut self,
        layer_id: u64,
        a: Point2,
        b: Point2,
        c: Point2,
    ) -> crate::core::ids::SegmentId {
        let p1 = self.doc.add_point(a);
        let p2 = self.doc.add_point(b);
        let pc = self.doc.add_point(c);
        let center_pos = crate::editor::arc::circumcircle(a, b, c)
            .map(|(o, _)| o)
            .unwrap_or(Point2::new((a.x + b.x) / 2., (a.y + b.y) / 2.));
        let p_center = self.doc.add_point(center_pos);
        let seg = self.doc.add_arc_segment(p1, pc, p2, p_center);
        for el in [
            ElementRef::Point(p1),
            ElementRef::Point(p2),
            ElementRef::Point(pc),
            ElementRef::Point(p_center),
            ElementRef::Segment(seg),
        ] {
            self.doc.push_to_layer(layer_id, el);
        }
        seg
    }

    /// Deletes an element from the document and clears it from selection.
    pub fn delete_element(&mut self, el: ElementRef) {
        match el {
            ElementRef::Point(p) => {
                self.doc.remove_point(p);
            }
            ElementRef::Segment(s) => {
                // Standalone endpoints die with their segment: once nothing
                // else references an endpoint, remove it too.
                let ends: Vec<PointId> = self
                    .doc
                    .segment(s)
                    .map(|seg| {
                        let mut v = vec![seg.start, seg.end];
                        if let Some(c) = seg.ctrl {
                            v.push(c);
                        }
                        if let Some(c) = seg.center {
                            v.push(c);
                        }
                        v
                    })
                    .unwrap_or_default();
                self.doc.remove_segment(s);
                for pid in ends {
                    let still_used = self.doc.all_segments().any(|(_, seg)| seg.start == pid || seg.end == pid)
                        || self.doc.constraints.iter().any(|c| c.a == pid || c.b == pid)
                        || self.doc.dimensions.iter().any(|d| d.a == pid || d.b == pid)
                        || self
                            .doc
                            .all_fills()
                            .any(|(_, f)| f.segments.iter().any(|&fs| {
                                self.doc.segment(fs).is_some_and(|seg| seg.start == pid || seg.end == pid)
                            }));
                    if !still_used {
                        self.doc.remove_point(pid);
                    }
                }
            }
            ElementRef::Fill(f) => {
                // Deleting a fill takes its edges and corners with it —
                // otherwise the skeleton lingers after the body is gone.
                let seg_ids: Vec<crate::core::ids::SegmentId> = self
                    .doc
                    .fill(f)
                    .map(|fl| fl.segments.clone())
                    .unwrap_or_default();
                let pts = self.doc.element_points(ElementRef::Fill(f));
                self.doc.remove_fill(f);
                // Drop constraints internal to the fill (H/V rectangle
                // edges etc.) so corners aren't held alive by them.
                self.doc
                    .constraints
                    .retain(|c| !(pts.contains(&c.a) && pts.contains(&c.b)));
                for s in seg_ids {
                    self.delete_element(ElementRef::Segment(s));
                }
            }
        }
        self.selection.retain(|&e| e != el);
    }

    /// Queues the bond-choice context menu for points dropped onto points.
    fn queue_bond_menu(&mut self) {
        let tol = self.snap_tol_doc();
        let Some(drag) = &self.dragging else { return };
        let dragged: Vec<PointId> =
            drag.points.iter().chain(drag.aux.iter()).map(|&(id, _)| id).collect();
        let mut pairs: Vec<(PointId, PointId)> = Vec::new();
        for &pid in &dragged {
            let Some(p) = self.doc.point(pid) else { continue };
            for (qid, q) in self.doc.all_points() {
                if qid == pid || dragged.contains(&qid) {
                    continue;
                }
                if pick::distance(p, q) > tol {
                    continue;
                }
                // Skip pairs already glued in either order or queued twice.
                if pairs.contains(&(pid, qid))
                    || pairs.contains(&(qid, pid))
                    || self.doc.constraints.iter().any(|c| {
                        c.kind == ConstraintKind::Coincident
                            && ((c.a == pid && c.b == qid) || (c.a == qid && c.b == pid))
                    })
                {
                    continue;
                }
                pairs.push((pid, qid));
            }
        }
        if pairs.is_empty() {
            return;
        }
        // Anchor beside the first junction, then clamp on screen.
        if let Some(p) = self.doc.point(pairs[0].0) {
            use crate::ui::canvas::context_menu::{ContextMenu, ContextAction, ContextMenuEntry,
                ICON_COINCIDENT, ICON_MERGE_POINTS};
            let s = self.camera.unit_to_screen(p);
            let mut menu = ContextMenu {
                x: s.x as f32 + 16.,
                y: s.y as f32 - 8.,
                entries: vec![
                    ContextMenuEntry {
                        icon: ICON_COINCIDENT,
                        label: "Coincident",
                        shortcut: "1",
                        action: ContextAction::BondCoincident,
                    },
                    ContextMenuEntry {
                        icon: ICON_MERGE_POINTS,
                        label: "Merge Points",
                        shortcut: "2",
                        action: ContextAction::BondMerge,
                    },
                ],
            };
            let (vw, vh) = (self.viewport_size.0 as f32, self.viewport_size.1 as f32);
            menu.clamp_to(vw, vh);
            self.pending_bonds = pairs;
            self.context_menu_pop = 0.0;
            self.context_menu = Some(menu);
        }
    }

    /// Applies the bond choice to every pending pair.
    fn apply_bond_choice(&mut self, combine: bool) -> bool {
        if self.pending_bonds.is_empty() {
            return false;
        }
        self.context_menu = None;
        let pairs = std::mem::take(&mut self.pending_bonds);
        for (a, b) in pairs {
            if combine {
                self.doc.merge_point(a, b);
            } else {
                self.doc.add_constraint(ConstraintKind::Coincident, a, b);
            }
        }
        self.selection.retain(|el| match *el {
            ElementRef::Point(p) => self.doc.point(p).is_some(),
            ElementRef::Segment(s) => self.doc.segment(s).is_some(),
            _ => true,
        });
        true
    }

    /// Applies a context menu entry's action. Returns whether anything
    /// changed.
    pub fn apply_context_action(
        &mut self,
        action: crate::ui::canvas::context_menu::ContextAction,
    ) -> bool {
        use crate::ui::canvas::context_menu::ContextAction;
        self.history_begin();
        let changed = match action {
            ContextAction::BondCoincident => self.apply_bond_choice(false),
            ContextAction::BondMerge => self.apply_bond_choice(true),
        };
        if !changed {
            // Drop the useless snapshot.
            self.gesture_snapshot = None;
        }
        changed
    }

    /// Triggers the Nth context menu entry (keyboard shortcuts).
    pub fn trigger_context_shortcut(&mut self, index: usize) -> bool {
        let Some(menu) = &self.context_menu else {
            return false;
        };
        let Some(entry) = menu.entries.get(index).map(|e| e.action) else {
            return false;
        };
        self.apply_context_action(entry)
    }

    /// Closes the context menu without applying anything.
    pub fn dismiss_context_menu(&mut self) -> bool {
        let had = self.context_menu.take().is_some();
        if had {
            self.pending_bonds.clear();
            self.context_menu_pop = 0.0;
            self.context_menu_fades.clear();
            self.context_menu_fade_pending.clear();
            self.context_menu_fade_active.clear();
        }
        had
    }

    pub(crate) fn context_menu_fade(&self, key: &str) -> f32 {
        self.context_menu_fades.get(key).copied().unwrap_or(0.0)
    }

    pub(crate) fn animate_context_menu_fade(
        &mut self,
        key: &str,
        target: f32,
        cx: &mut gpui::Context<Self>,
    ) {
        self.context_menu_fade_pending.insert(key.to_string(), target);
        if !self.context_menu_fade_active.insert(key.to_string()) {
            return;
        }
        let key_owned = key.to_string();
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(12))
                    .await;
                let mut done = false;
                let _ = this.update(cx, |ed, cx| {
                    if let Some(&target) = ed.context_menu_fade_pending.get(&key_owned) {
                        let cur = ed.context_menu_fade(&key_owned);
                        let next = cur + (target - cur) * 0.4;
                        if (next - target).abs() < 0.01 {
                            ed.context_menu_fades.insert(key_owned.clone(), target);
                            done = true;
                        } else {
                            ed.context_menu_fades.insert(key_owned.clone(), next);
                        }
                        cx.notify();
                    } else {
                        done = true;
                    }
                });
                if !done {
                    continue;
                }
                let _ = this.update(cx, |ed, _| {
                    ed.context_menu_fade_pending.remove(&key_owned);
                    ed.context_menu_fade_active.remove(&key_owned);
                });
                break;
            }
        })
        .detach();
    }

    pub(crate) fn animate_context_menu_pop(
        &mut self,
        target: f32,
        cx: &mut gpui::Context<Self>,
    ) {
        let start = self.context_menu_pop;
        if (start - target).abs() < f32::EPSILON {
            return;
        }
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let steps = 8;
            for i in 1..=steps {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let _ = this.update(cx, |ed, cx| {
                    let t = i as f32 / steps as f32;
                    ed.context_menu_pop = start + (target - start) * (1.0 - (1.0 - t).powi(3));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Re-derives session state after the document was swapped by
    /// undo/redo — drops anything referencing ids that may not exist.
    pub(crate) fn after_history_restore(&mut self) {
        self.selection.retain(|el| match *el {
            ElementRef::Point(p) => self.doc.point(p).is_some(),
            ElementRef::Segment(s) => self.doc.segment(s).is_some(),
            ElementRef::Fill(f) => self.doc.fill(f).is_some(),
        });
        self.selected_constraints
            .retain(|c| self.doc.constraints.contains(c));
        self.pending_bonds.clear();
        self.context_menu = None;
        self.hovered_constraint = None;
        self.snap_guides.clear();
        self.pending_shape = None;
        self.pending_ruler = None;
        self.pending_line = None;
        self.pending_circle = None;
        self.pending_via_click = false;
        self.dragging = None;
        self.marquee = None;
        self.deferred_pick = None;
        self.group_drag_last = None;
    }

    // True when the element itself, or anything SELECTED that contains it,
    // covers it — a corner shared by selected edges counts as selected.
    fn element_selected(&self, el: ElementRef) -> bool {
        if self.selection.contains(&el) {
            return true;
        }
        match el {
            ElementRef::Segment(sid) => self
                .fill_containing(sid)
                .is_some_and(|f| self.selection.contains(&ElementRef::Fill(f))),
            ElementRef::Point(pid) => self.selection.iter().any(|sel| match *sel {
                ElementRef::Segment(s) => self
                    .doc
                    .segment(s)
                    .is_some_and(|seg| seg.start == pid || seg.end == pid),
                ElementRef::Fill(f) => {
                    self.doc.element_points(ElementRef::Fill(f)).contains(&pid)
                }
                _ => false,
            }),
            _ => false,
        }
    }

    fn fill_containing(&self, sid: crate::core::ids::SegmentId) -> Option<FillId> {
        self.doc
            .all_fills()
            .find(|(_, f)| f.segments.contains(&sid))
            .map(|(id, _)| id)
    }

    // -- viewport interaction (called from the canvas view) --

    pub fn begin_pan(&mut self, cursor: gpui::Point<gpui::Pixels>) {
        self.pan_start = Some((cursor.x, cursor.y, self.camera));
    }

    // Returns true if the view changed and a repaint is needed.
    pub fn pan_delta(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        let Some((x0, y0, start)) = self.pan_start else {
            return false;
        };
        let dx = f64::from(cursor.x - x0);
        let dy = f64::from(cursor.y - y0);
        self.camera.pan = Point2::new(
            start.pan.x - dx / start.zoom,
            start.pan.y - dy / start.zoom,
        );
        true
    }

    pub fn end_pan(&mut self) -> bool {
        self.pan_start.take().is_some()
    }

    // Zooms keeping the document point under the cursor anchored.
    pub fn zoom_at(&mut self, cursor: gpui::Point<gpui::Pixels>, delta: f32) {
        let cursor_doc = |cam: &Camera, c: gpui::Point<gpui::Pixels>| {
            cam.screen_to_unit(Point2::new(f64::from(c.x), f64::from(c.y)))
        };
        let before = cursor_doc(&self.camera, cursor);
        let factor = f64::from((-delta / 400.).exp());
        self.camera.set_zoom(self.camera.zoom * factor);
        let after = cursor_doc(&self.camera, cursor);
        self.camera.pan = Point2::new(
            self.camera.pan.x + (before.x - after.x),
            self.camera.pan.y + (before.y - after.y),
        );
    }

    /// One zoom step around the VIEWPORT center (menu/key triggered —
    /// there is no cursor anchor).
    pub fn zoom_step(&mut self, dir: f64) {
        const STEP: f32 = 120.;
        let c = gpui::Point {
            x: gpui::px((self.viewport_size.0 / 2.) as f32),
            y: gpui::px((self.viewport_size.1 / 2.) as f32),
        };
        self.zoom_at(c, (dir * STEP as f64) as f32);
    }

    /// Fits the camera to a document-space rect with padding.
    pub fn zoom_to_bounds(&mut self, bounds: Rect) -> bool {
        if bounds.size.w < 1e-6 || bounds.size.h < 1e-6 {
            return false;
        }
        if self.viewport_size.0 <= 0. || self.viewport_size.1 <= 0. {
            return false;
        }
        const PAD_PX: f64 = 16.;
        let zw = (self.viewport_size.0 - PAD_PX * 2.) / bounds.size.w;
        let zh = (self.viewport_size.1 - PAD_PX * 2.) / bounds.size.h;
        self.camera.set_zoom(zw.min(zh));
        let c = Point2::new(
            bounds.origin.x + bounds.size.w / 2.,
            bounds.origin.y + bounds.size.h / 2.,
        );
        self.camera.pan = Point2::new(
            c.x - self.viewport_size.0 / (2. * self.camera.zoom),
            c.y - self.viewport_size.1 / (2. * self.camera.zoom),
        );
        true
    }

    /// Bounding rect of the current selection, if any.
    pub fn selection_bounds(&self) -> Option<Rect> {
        if self.selection.is_empty() {
            return None;
        }
        let pts = self.doc.selection_points(&self.selection);
        self.doc.bounds_of_points(&pts)
    }

    pub fn zoom_to_fit(&mut self) -> bool {
        let mut acc: Option<Rect> = None;
        for layer in &self.doc.layers {
            for &el in &layer.elements {
                let pts = self.doc.element_points(el);
                if let Some(b) = self.doc.bounds_of_points(&pts) {
                    acc = Some(match acc {
                        Some(a) => a.union(&b),
                        None => b,
                    });
                }
            }
        }
        match acc {
            Some(b) => self.zoom_to_bounds(b),
            None => false,
        }
    }

    pub fn zoom_to_selection(&mut self) -> bool {
        match self.selection_bounds() {
            Some(b) => self.zoom_to_bounds(b),
            None => false,
        }
    }

    pub fn add_layer(&mut self, name: &str) -> u64 {
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        self.doc.layers.push(Layer {
            id,
            name: name.into(),
            elements: Vec::new(),
        });
        id
    }

    // Visible region in document units — used for culling before paint.
    pub fn visible_bounds(&self, size: Size) -> Rect {
        let min = self.camera.screen_to_unit(Point2::new(0., 0.));
        let max = self.camera.screen_to_unit(Point2::new(size.w, size.h));
        Rect::from_points(min, max)
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
