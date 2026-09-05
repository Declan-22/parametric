pub mod arc;
pub mod bezier;
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
pub use tools::{DimInput, DimPick, PendingCircle, PendingLine, PendingPen, PendingRuler, PendingShape, Tool};

use crate::core::constraints::{ConstraintKind, DimTarget, ElementRef};
use crate::core::document::{Document, Layer};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{FillId, PathId, PointId, SegmentId};
use std::collections::HashSet;
use std::time::Instant;

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
    // Arc body grab: the whole arc scales about its fixed center (see
    // plan_arc_drag). None for every other gesture (solver path).
    pub arc_body_scale: Option<SegmentId>,
    // A cubic edge grab edits its handles directly. Keeping the grab
    // parameter and original controls makes the operation stable across
    // mouse-move events and avoids routing a simple curve edit through the
    // global constraint solver.
    pub bezier_edge: Option<BezierEdgeDrag>,
}

#[derive(Clone, Copy)]
pub(crate) struct BezierEdgeDrag {
    pub sid: SegmentId,
    pub t: f64,
    pub start_cursor: Point2,
    pub handle_out: Point2,
    pub handle_in: Point2,
}

/// Kinematic arc drag outcome: exact targets plus points to hard-pin for
/// the follow-up solve, plus an optional exact center refit to apply first.
struct ArcKin {
    targets: Vec<(PointId, Point2)>,
    pins: Vec<(PointId, Point2)>,
    premove: Option<(PointId, Point2)>,
}

pub struct Editor {
    pub doc: Document,
    pub camera: Camera,
    pub tool: Tool,
    pub pending_shape: Option<PendingShape>,
    pub pending_ruler: Option<PendingRuler>,
    pub pending_line: Option<PendingLine>,
    pub pending_circle: Option<PendingCircle>,
    // In-progress pen chain (docs/pen-tool.md): live path in the doc.
    pub pending_pen: Option<PendingPen>,
    // Pending shape created by a single click (commit on next click).
    pub pending_via_click: bool,
    pub selection: Vec<ElementRef>,
    // Elements picked by an active constraint tool before its constraint is
    // committed.
    pub constraint_picks: Vec<ElementRef>,
    pub constraint_point_picks: Vec<PointId>,
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
    // Creation-tool snap cursor: the crosshair's position in DOC
    // coordinates plus whether a snap is engaged. Storing doc coords (not
    // screen) lets the render layer re-project every frame, so the
    // crosshair stays glued to its point across zoom and pan. The OS
    // cursor stays a plain arrow. None when no creation tool is active or
    // while panning.
    pub creation_cursor: Option<(f64, f64, bool)>,
    // Dimension tool: picks accumulating toward a dimension (0..2), the
    // resolved pending target (Some = placement mode, preview follows the
    // cursor), plus the placed value-input state (Enter/typing commits,
    // Esc cancels). `existing` inside DimInput marks an EDIT of a stored
    // dimension.
    pub dim_picks: Vec<DimPick>,
    pub dim_target: Option<crate::core::constraints::DimTarget>,
    pub dim_input: Option<DimInput>,
    // Per-frame angle-dimension render data (dashed arc + container).
    pub angle_dim_renders: Vec<dims::AngleDimRender>,
    // Bumped on every committed document change; Shell watches it for
    // debounced autosave.
    pub doc_gen: u64,
    // Label hitboxes for placed dimensions: (dimension index, x, y, w, h)
    // in canvas-local px — rebuilt every frame by the label layer.
    pub dim_hitboxes: Vec<(usize, [f32; 4])>,
    // Placed dimension under the cursor (hover highlight).
    pub hovered_dim: Option<usize>,
    // The SELECTED placed dimension (tap): stays highlighted, Delete removes
    // it, a second tap enters its value input.
    pub selected_dim: Option<usize>,
    // Index of the placed dimension being repositioned by a drag.
    pub dim_drag: Option<DimDrag>,
    // Caret blink for the dimension value input.
    pub dim_caret_visible: bool,
    // Over-constrained modal: set when a dimension placement is infeasible.
    pub overconstrained: bool,
    // Arc centers currently revealed (cursor inside the arc's disk).
    // Drives repaint detection: cursor moves that change nothing else must
    // still repaint when the reveal set changes, or the dot sticks around
    // after the mouse leaves the area.
    pub arc_center_reveal: Vec<SegmentId>,
    // Paint-only diagnostics; deliberately not part of Document or undo
    // snapshots.
    pub(crate) fps_counter: FpsCounter,
}

pub(crate) struct FpsCounter {
    last_sample: Instant,
    frames: u32,
    fps: f32,
}

impl Default for FpsCounter {
    fn default() -> Self {
        Self { last_sample: Instant::now(), frames: 0, fps: 0.0 }
    }
}

impl FpsCounter {
    pub(crate) fn tick(&mut self) -> f32 {
        self.frames = self.frames.saturating_add(1);
        let elapsed = self.last_sample.elapsed();
        if elapsed.as_millis() >= 250 {
            self.fps = self.frames as f32 / elapsed.as_secs_f32().max(1e-6);
            self.frames = 0;
            self.last_sample = Instant::now();
        }
        self.fps
    }
}

/// An in-progress drag of a placed dimension container: `down` is the
/// grab position (doc space); a clean release (no movement) on an
/// already-selected dim counts as a second tap -> edit input.
pub struct DimDrag {
    pub index: usize,
    pub down_doc: Point2,
    pub moved: bool,
    pub was_selected: bool,
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
        let settings = doc.settings;
        let next_layer_id = doc.layers.iter().map(|l| l.id + 1).max().unwrap_or(1);
        Self {
            doc,
            camera: Camera::new(),
            tool: Tool::Move,
            pending_shape: None,
            pending_ruler: None,
            pending_line: None,
            pending_circle: None,
            pending_pen: None,
            pending_via_click: false,
            selection: Vec::new(),
            constraint_picks: Vec::new(),
            constraint_point_picks: Vec::new(),
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
            show_grid: settings.show_grid,
            snap_to_grid: settings.snap_to_grid,
            snap_to_objects: settings.snap_to_objects,
            creation_cursor: None,
            dim_picks: Vec::new(),
            dim_target: None,
            dim_input: None,
            angle_dim_renders: Vec::new(),
            doc_gen: 0,
            dim_hitboxes: Vec::new(),
            hovered_dim: None,
            selected_dim: None,
            dim_drag: None,
            dim_caret_visible: true,
            overconstrained: false,
            arc_center_reveal: Vec::new(),
            fps_counter: FpsCounter::default(),
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
        self.abort_pending_pen();
        self.pending_shape = None;
        self.pending_ruler = None;
        self.pending_line = None;
        self.pending_circle = None;
        self.pending_via_click = false;
        self.selection.clear();
        self.constraint_picks.clear();
        self.constraint_point_picks.clear();
        self.selected_constraints.clear();
        self.context_menu = None;
        self.pending_bonds = Vec::new();
        self.marquee = None;
        self.group_drag_last = None;
        self.dragging = None;
        // Dimension tool picks + value input reset on every tool switch.
        self.dim_picks.clear();
        self.dim_target = None;
        self.dim_input = None;
        self.overconstrained = false;
        true
    }

    // True while no drag or pan is in progress (gates hover tracking).
    pub fn is_idle(&self) -> bool {
        self.pan_start.is_none() && self.dragging.is_none()
    }

    // -- canvas input (called from the canvas view) --

    pub(crate) fn cursor_doc(&self, cursor: gpui::Point<gpui::Pixels>) -> Point2 {
        self.camera
            .screen_to_unit(Point2::new(f64::from(cursor.x), f64::from(cursor.y)))
    }

    /// Recomputes which arc centers are revealed (cursor inside the arc's
    /// disk). Returns true when the set changed — the caller must repaint
    /// even if nothing else changed, or the dot sticks around after the
    /// mouse leaves the area.
    pub fn update_arc_reveal(&mut self, cur: Point2) -> bool {
        let mut inside: Vec<SegmentId> = Vec::new();
        for (sid, seg) in self.doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Arc {
                continue;
            }
            let Some(sc) = seg.ctrl else { continue };
            let (Some(a), Some(b), Some(c)) = (
                self.doc.point(seg.start),
                self.doc.point(seg.end),
                self.doc.point(sc),
            ) else {
                continue;
            };
            let Some((center, r)) = crate::editor::arc::circumcircle(a, b, c) else {
                continue;
            };
            if r < 1e-9 {
                continue;
            }
            let dx = cur.x - center.x;
            let dy = cur.y - center.y;
            if (dx * dx + dy * dy).sqrt() <= r {
                inside.push(sid);
            }
        }
        inside.sort_by(|a, b| (a.idx, a.generation).cmp(&(b.idx, b.generation)));
        if inside == self.arc_center_reveal {
            false
        } else {
            self.arc_center_reveal = inside;
            true
        }
    }

    /// Clears the reveal set (mouse left the canvas). True when something
    /// was showing.
    pub fn clear_arc_reveal(&mut self) -> bool {
        if self.arc_center_reveal.is_empty() {
            false
        } else {
            self.arc_center_reveal.clear();
            true
        }
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
        // Any click dismisses the pending bond-choice menu first.
        if self.context_menu.take().is_some() {
            return true;
        }
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
                Tool::Pen => {
                    let p = self.cursor_doc(cursor);
                    let exact =
                        pick::Picker::new(&self.doc, &self.camera, EXACT_TOL_PX);
                    // Double-click an anchor toggles smooth/corner.
                    if click_count >= 2
                        && let Some(pid) = exact.point(p)
                        && self.path_anchor_index(pid).is_some()
                    {
                        self.toggle_anchor_continuity(pid);
                        return true;
                    }
                    // Clicking a pathed curve splits it exactly; the open
                    // chain (if any) continues from its live tip.
                    if exact.point(p).is_none()
                        && let Some(sid) = pick::Picker::new(
                            &self.doc,
                            &self.camera,
                            HANDLE_TOL_PX,
                        )
                        .segment(p)
                        && self.path_containing(sid).is_some()
                    {
                        self.insert_pen_anchor(sid, p);
                        return true;
                    }
                    // Otherwise commit an anchor at the snapped cursor and
                    // arm the handle pull (click-drag bends it this gesture).
                    let (mut at, guides) = self.snap_creation_point(p);
                    if shift
                        && let Some(pending) = &self.pending_pen
                        && let Some(start) = self.doc.point(pending.last)
                    {
                        at = tools::snap_angle(start, at);
                    }
                    self.snap_guides = guides;
                    self.commit_pen_anchor(at);
                    if let Some(pending) = self.pending_pen.as_mut() {
                        pending.pulling = true;
                        pending.cursor = at;
                    }
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
                            if shift { self.maybe_add_tangent(seg, b); }
                            self.selection = vec![ElementRef::Segment(seg)];
                            // Chain: the next line starts where this one ended.
                            self.pending_line = Some(PendingLine { start: b, cursor: b, anchor: None });
                        } else {
                            let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                            self.snap_guides = guides;
                            self.pending_line = Some(PendingLine { start: at, cursor: at, anchor: None });
                        }
                        self.pending_via_click = true;
                        return true;
                    }
                    let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    self.pending_line = Some(PendingLine { start: at, cursor: at, anchor: None });
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
                    // Placement mode: a click on EMPTY SPACE places the
                    // dimension at the cursor; a click on geometry
                    // re-resolves the pending target instead (edge + edge
                    // turns into an angle/distance pair, point + edge into
                    // a point-line distance...).
                    if self.dim_target.is_some() {
                        let doc_p = self.cursor_doc(cursor);
                        if let Some(target) = self.dim_target {
                            if let Some((mode, offset, slide, measured)) = self.dim_placement(target, doc_p) {
                                // Once two picks form a target, every click is
                                // a placement click. Previously a point hit
                                // near a slanted dimension was interpreted as
                                // another pick, so the value editor never
                                // opened for aligned displacement dimensions.
                                self.dim_picks.clear();
                                self.dim_target = None;
                                self.selection.clear();
                                self.dim_input = Some(DimInput {
                                    target: target.with_mode(mode), offset, slide, measured,
                                    buffer: String::new(), existing: None,
                                });
                            }
                        }
                        return true;
                    }
                    // Accumulate picks: points and whole lines (an edge is a
                    // complete dimension all by itself; an arc is a radius).
                    // Re-clicking a pick deselects it.
                    let doc_p = self.cursor_doc(cursor);
                    let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
                    let new_pick = picker
                        .point(doc_p)
                        .map(DimPick::Point)
                        .or_else(|| picker.segment(doc_p).map(DimPick::Line));
                    let Some(pick) = new_pick else {
                        return false;
                    };
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
                    true
                }
                // Constraint tools are handled before this mode match so
                // their clicks never enter shape/dimension creation. Keep an
                // explicit arm for exhaustive enum matching.
                Tool::ConstraintHorizontalVertical
                | Tool::ConstraintTangent
                | Tool::ConstraintCoincident
                | Tool::ConstraintParallel => false,
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
                    if cluster.len() > 1 {
                        // Explicit coincident points are separate entities in
                        // the model, but a coincident stack is one temporary
                        // drag handle. Drag every member as a primary target
                        // so the stack cannot stretch apart while moving.
                        let drag = cluster
                            .iter()
                            .filter_map(|&p| self.doc.point(p).map(|pos| (p, pos)))
                            .collect();
                        let aux = ring_of(&cluster);
                        (drag, aux)
                    } else {
                        let start = self.doc.point(pid).unwrap();
                        let aux = ring_of(&[pid]);
                        (vec![(pid, start)], aux)
                    }
                    }
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
                    // ARCS are the exception: dragging the arc body scales
                    // the arc about its fixed center (see plan_arc_drag);
                    // its endpoints are the sweep handles.
                    let seg = self.doc.segment(*sid);
                    if seg.is_some_and(|s| s.kind == crate::core::document::SegmentKind::Arc) {
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
                        let ends: Vec<PointId> = seg
                            .map(|s| vec![s.start, s.end])
                            .unwrap_or_default();
                        let drag = ends
                            .iter()
                            .filter_map(|&pid| self.doc.point(pid).map(|pos| (pid, pos)))
                            .collect();
                        (drag, Vec::new())
                    }
                } else {
                    let pts = self.doc.selection_points(&self.selection);
                    let drag = pts
                        .iter()
                        .filter_map(|&pid| self.doc.point(pid).map(|pos| (pid, pos)))
                        .collect();
                    (drag, Vec::new())
                };
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
                let bezier_edge = match el {
                    ElementRef::Segment(sid)
                        if !(self.selection.len() > 1 && self.element_selected(el)) =>
                    {
                        let seg = self.doc.segment(sid);
                        if seg.is_some_and(|s| s.kind == crate::core::document::SegmentKind::Bezier) {
                            self.doc.bezier_geom(sid).map(|(p0, p1, p2, p3)| {
                                let (t, _) = crate::editor::bezier::param_of_closest(p, p0, p1, p2, p3);
                                BezierEdgeDrag {
                                    sid,
                                    t: t.clamp(0.05, 0.95),
                                    start_cursor: p,
                                    handle_out: p1,
                                    handle_in: p2,
                                }
                            })
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                self.dragging = Some(DragState {
                    points: drag_pts,
                    aux: aux_pts,
                    start_cursor: p,
                    arc_body_scale,
                    bezier_edge,
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
        if self.tool == Tool::Dimension && self.dim_input.is_none() {
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

        // Pen chain: rubber band from the live tip, or the fresh anchor's
        // handle pull while its commit gesture is still held.
        if self.tool == Tool::Pen && self.pending_pen.is_some() {
            let mut at = self.cursor_doc(cursor);
            let pulling = self.pending_pen.as_ref().is_some_and(|p| p.pulling);
            if pulling {
                self.update_pen_pull(at, shift);
                return true;
            }
            let (snapped, guides) = self.snap_creation_point(at);
            at = snapped;
            self.snap_guides = guides;
            if let Some(pending) = self.pending_pen.as_mut() {
                // The tip IS the live anchor id (no position snapshot),
                // so mid-chain edits to it move the rubber band for free.
                pending.cursor = at;
            }
            changed = true;
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
            if let Some(pending) = self.pending_line.as_mut() {
                pending.cursor = tangent_at.unwrap_or(at);
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
            if !matches!(
                self.tool,
                Tool::Line | Tool::Rectangle | Tool::Ruler | Tool::Circle | Tool::Pen
            ) {
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
        if let Some(edge) = drag.bezier_edge {
            return self.update_bezier_edge_drag(edge, p);
        }

        // Snap exclusion, two flavors:
        //  - exclude_pts: everything belonging to the dragged system
        //    (transitively connected) plus points co-located with a drag
        //    start. Endpoint/midpoint targets from this set are DEAD — you
        //    never relocate onto your own geometry.
        //  - exclude_segs: only the actually-dragged segments. Edge-span
        //    ALIGNMENTS from the rest of the component remain live, so a
        //    fully-connected drawing still snaps to axis alignments.
        let mut exclude_pts: HashSet<PointId> = drag.points.iter().map(|(id, _)| *id).collect();
        let mut exclude_segs: Vec<crate::core::ids::SegmentId> = Vec::new();
        for &(pid, _) in &drag.aux {
            exclude_pts.insert(pid);
        }
        let selected_pt_ids = self.doc.selection_points(&self.selection);
        for pid in &selected_pt_ids {
            exclude_pts.insert(*pid);
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
        let mut frontier: Vec<PointId> = exclude_pts.iter().copied().collect();
        let mut i = 0;
        while i < frontier.len() {
            let pid = frontier[i];
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
                    exclude_pts.insert(o);
                    frontier.push(o);
                }
            }
            i += 1;
        }
        // Arc defining points form ONE snap-unit: the ctrl point is not a
        // segment endpoint (the closure above never reaches it), so dragging
        // the bend would otherwise snap to its own arc's endpoints/midpoint.
        // Bezier handles are the same story. If any of the set is excluded,
        // all of it is.
        let mut arc_closure = true;
        while arc_closure {
            arc_closure = false;
            for (_, s) in self.doc.all_segments() {
                let defs: Vec<PointId> = if s.kind == crate::core::document::SegmentKind::Arc {
                    let Some(ctrl) = s.ctrl else { continue };
                    vec![s.start, s.end, ctrl]
                } else if s.kind == crate::core::document::SegmentKind::Bezier {
                    let mut v = vec![s.start, s.end];
                    if let Some(h) = s.handle_out {
                        v.push(h);
                    }
                    if let Some(h) = s.handle_in {
                        v.push(h);
                    }
                    v
                } else {
                    continue;
                };
                let any_in = defs.iter().any(|d| exclude_pts.contains(d));
                let any_out = defs.iter().any(|d| !exclude_pts.contains(d));
                if any_in && any_out {
                    for d in defs {
                        if exclude_pts.insert(d) {
                            // The closure is intentionally repeated because a
                            // newly excluded Bezier/arc can expose another
                            // defining point in a later segment.
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
                exclude_pts.insert(pid);
            }
        }

        // `snapping::best` takes a compact slice; keep the hash set for all
        // membership checks above, then materialize this once for the snap
        // queries instead of paying O(n) for every closure lookup.
        let exclude_pts_vec: Vec<PointId> = exclude_pts.iter().copied().collect();

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
                let target = Point2::new(start.x + delta.x, start.y + delta.y);
                let (adj, guides) = snapping::best(
                    &self.doc,
                    self.snap_tol_doc(),
                    target,
                    &exclude_pts_vec,
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

        // Kinematic arc drags bypass the least-squares hunt entirely:
        // endpoints slide on the circle, the bend refits it, the body
        // scales about the center. Everything else takes the solver path.
        let solver = match self.plan_arc_drag(drag, &targets) {
            Some(kin) => {
                if let Some((pid, pos)) = kin.premove {
                    self.doc.move_point(pid, pos);
                }
                // 1.0 = the default live-drag anchor weight.
                crate::core::solver::Solver::build_pinned(
                    &self.doc,
                    &kin.targets,
                    &aux_all,
                    &kin.pins,
                    1.0,
                )
            }
            None => crate::core::solver::Solver::build(&self.doc, &targets, &aux_all),
        };
        let solution = solver.solve();
        let mut moved: std::collections::HashSet<PointId> = std::collections::HashSet::new();
        for (id, pos) in solution.positions {
            moved.insert(id);
            self.doc.move_point(id, pos);
        }
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
        true
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
    ///  - body (DragState::arc_body_scale): uniform scale about the frozen
    ///    center;
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
        let circ_dist = |x: f64, y: f64| wrap(x - y).abs();

        let drag_ids: Vec<PointId> = drag.points.iter().map(|&(id, _)| id).collect();
        // Any center dragged -> rigid translate, already exact. Legacy path.
        for (_, s) in self.doc.all_segments() {
            if s.kind == SegmentKind::Arc
                && s.center.is_some_and(|c| drag_ids.contains(&c))
            {
                return None;
            }
        }
        // Exactly one arc touched, else legacy (e.g. shared vertices).
        let mut arc_sid = None;
        for (sid, s) in self.doc.all_segments() {
            if s.kind != SegmentKind::Arc {
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

        let start_of = |pid: PointId| -> Option<Point2> {
            drag.points.iter().find(|(id, _)| *id == pid).map(|&(_, s)| s)
        };
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
            if radius_locked || !partners.is_empty() {
                return None;
            }
            let Some(ct) = target_of(ctrl) else {
                return None;
            };
            let f = (pick::distance(ct, cc) / r).clamp(0.2, 5.0);
            let mut out = Vec::with_capacity(targets.len());
            for &(pid, _) in targets {
                let Some(s) = start_of(pid) else {
                    return None;
                };
                out.push(if pid == center_id {
                    (pid, cc)
                } else {
                    (
                        pid,
                        Point2::new(cc.x + (s.x - cc.x) * f, cc.y + (s.y - cc.y) * f),
                    )
                });
            }
            return Some(ArcKin {
                targets: out,
                pins: Vec::new(),
                premove: Some((center_id, cc)),
            });
        }

        let ctrl_d = in_drag(ctrl);
        let start_d = in_drag(seg.start);
        let end_d = in_drag(seg.end);

        // Endpoint: slide along the existing circle, clamped away from the
        // other endpoint and the bend so the sweep can never invert.
        if (start_d ^ end_d) && !ctrl_d {
            let (e_pid, f_cur) = if start_d {
                (seg.start, b)
            } else {
                (seg.end, a)
            };
            let Some(t0) = target_of(e_pid) else {
                return None;
            };
            let ang = |p: Point2| (p.y - cc.y).atan2(p.x - cc.x);
            let e_cur = if start_d { a } else { b };
            let th_prev = ang(e_cur);
            let dth = wrap(ang(t0) - th_prev).clamp(-0.2, 0.2);
            let mut th = th_prev + dth;
            const MARGIN: f64 = 0.14;
            if circ_dist(th, ang(f_cur)) < MARGIN || circ_dist(th, ang(c)) < MARGIN {
                // Forbidden zone straddling the other endpoint or the bend:
                // hold last frame instead of crossing (crossing inverts).
                th = th_prev;
            }
            let t = Point2::new(cc.x + r * th.cos(), cc.y + r * th.sin());
            let pins = vec![(seg.start, a), (seg.end, b), (ctrl, c), (center_id, cc)];
            let pins = pins.into_iter().filter(|(id, _)| *id != e_pid).collect();
            let mut out = Vec::with_capacity(targets.len());
            for &(pid, _) in targets {
                out.push((pid, t));
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
            if radius_locked {
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
        for (_, s) in self.doc.all_segments() {
            if s.kind != crate::core::document::SegmentKind::Arc {
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
                    DimTarget::Radius { seg } if seg == seg_id => Some(d.value),
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
            Tool::Pen if self.pending_pen.is_none() => {
                let (_at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
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

    /// Edit a cubic by grabbing the curve itself. The hit parameter is fixed
    /// at mouse-down, so the same part of the curve remains under the cursor
    /// while dragging. A normal drag moves that curve point exactly by moving
    /// both handles; Alt changes the control hull's normal offset instead,
    /// which changes the overall roundness without sliding the endpoints.
    fn update_bezier_edge_drag(&mut self, edge: BezierEdgeDrag, cursor: Point2) -> bool {
        let Some(seg) = self.doc.segment(edge.sid) else { return false };
        let (Some(h0), Some(h1)) = (seg.handle_out, seg.handle_in) else { return false };
        let Some((p0, p1, p2, p3)) = self.doc.bezier_geom(edge.sid) else { return false };
        let t = edge.t;
        let mt = 1.0 - t;
        let tangent = Point2::new(
            3.0 * mt * mt * (p1.x - p0.x) + 6.0 * mt * t * (p2.x - p1.x) + 3.0 * t * t * (p3.x - p2.x),
            3.0 * mt * mt * (p1.y - p0.y) + 6.0 * mt * t * (p2.y - p1.y) + 3.0 * t * t * (p3.y - p2.y),
        );
        let length = (tangent.x * tangent.x + tangent.y * tangent.y).sqrt().max(1e-9);
        let normal = Point2::new(-tangent.y / length, tangent.x / length);
        let delta = Point2::new(cursor.x - edge.start_cursor.x, cursor.y - edge.start_cursor.y);
        let amount = delta.x * normal.x + delta.y * normal.y;
        if amount.abs() < 1e-9 { return false; }

        let (n0, n1) = if self.alt_down {
            // Alt uses the chord normal, rather than the local tangent, so
            // the gesture controls the curve's broad roundness as a whole.
            let chord = Point2::new(p3.x - p0.x, p3.y - p0.y);
            let cl = (chord.x * chord.x + chord.y * chord.y).sqrt().max(1e-9);
            let chord_normal = Point2::new(-chord.y / cl, chord.x / cl);
            (Point2::new(chord_normal.x * amount, chord_normal.y * amount),
             Point2::new(chord_normal.x * amount, chord_normal.y * amount))
        } else {
            let scale = amount / (3.0 * t * mt);
            (Point2::new(normal.x * scale, normal.y * scale),
             Point2::new(normal.x * scale, normal.y * scale))
        };
        self.doc.move_point(h0, Point2::new(edge.handle_out.x + n0.x, edge.handle_out.y + n0.y));
        self.doc.move_point(h1, Point2::new(edge.handle_in.x + n1.x, edge.handle_in.y + n1.y));
        true
    }

    fn constraint_hover_allowed(&self, element: ElementRef) -> bool {
        match self.tool {
            Tool::ConstraintHorizontalVertical => {
                matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line))
            }
            Tool::ConstraintTangent => {
                matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| matches!(s.kind, crate::core::document::SegmentKind::Line | crate::core::document::SegmentKind::Arc)))
            }
            Tool::ConstraintCoincident => matches!(element, ElementRef::Point(_) | ElementRef::Segment(_)),
            Tool::ConstraintParallel => matches!(element, ElementRef::Segment(sid) if self.doc.segment(sid).is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line)),
            _ => false,
        }
    }

    fn normalize_constraint_hit(&self, element: ElementRef) -> Option<ElementRef> {
        if self.constraint_hover_allowed(element) {
            return Some(element);
        }
        let ElementRef::Point(point) = element else { return None };
        match self.tool {
            Tool::ConstraintHorizontalVertical | Tool::ConstraintTangent => self
                .doc
                .all_segments()
                .filter(|(_, s)| {
                    let allowed = match self.tool {
                        Tool::ConstraintHorizontalVertical => s.kind == crate::core::document::SegmentKind::Line,
                        Tool::ConstraintTangent => matches!(s.kind, crate::core::document::SegmentKind::Line | crate::core::document::SegmentKind::Arc),
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

    /// SHIFT constraints for the arc creation cursor, applied to the
    /// snapped position: stage 2 (chord) locks its direction to 45-degree
    /// steps; stage 3 (bulge) snaps the sweep to 90-degree steps. Returns
    /// the adjusted position plus whether a sweep transform engaged (which
    /// invalidates raw-cursor alignment guides).
    fn arc_creation_shift(
        stage: u8,
        a: Option<Point2>,
        b: Option<Point2>,
        at: Point2,
        shift: bool,
    ) -> (Point2, bool) {
        if !shift {
            return (at, false);
        }
        match stage {
            2 => (a.map(|a| tools::snap_angle(a, at)).unwrap_or(at), false),
            3 => match (a, b) {
                (Some(a), Some(b)) => match crate::editor::arc::snap_sweep(a, b, at) {
                    Some(snapped) => (snapped, true),
                    None => (at, false),
                },
                _ => (at, false),
            },
            _ => (at, false),
        }
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
        if self.tool == Tool::Line && self.shift {
            if let Some(pending) = self.pending_line {
                if let Some(at) = self.tangent_snap_for_line(pending.start, pos) {
                    self.snap_guides.clear();
                    let next = Some((at.x, at.y, true));
                    let changed = self.creation_cursor != next;
                    self.creation_cursor = next;
                    return changed;
                }
            }
        }
        // Apply the arc-tool shift constraints so the crosshair matches the
        // pending preview exactly; a sweep transform invalidates the raw
        // cursor's alignment guides (the arc IS the constraint now).
        let pending = self.pending_circle.map(|p| (p.stage(), p.a, p.b));
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

    // -- settings (per design + app-wide last-used defaults) --

    /// Writes the current view/snap settings as the app-wide defaults used
    /// to seed NEW designs. Existing design files are never touched by this.
    fn save_default_settings(
        cx: &gpui::App,
        settings: &crate::core::document::DocSettings,
    ) {
        if let Some(reg) = cx.try_global::<crate::persistence::registry::Registry>() {
            reg.set_default_doc_settings(settings);
        }
    }

    /// Persists one toggle: live editor field, the document's own settings
    /// (autosaved with the design), and the app-wide last-used defaults.
    fn apply_setting(
        &mut self,
        which: u8,
        on: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        let changed = match which {
            0 => self.show_grid != on,
            1 => self.snap_to_grid != on,
            _ => self.snap_to_objects != on,
        };
        if !changed {
            return;
        }
        match which {
            0 => self.show_grid = on,
            1 => self.snap_to_grid = on,
            _ => self.snap_to_objects = on,
        }
        self.doc.settings = crate::core::document::DocSettings {
            show_grid: self.show_grid,
            snap_to_grid: self.snap_to_grid,
            snap_to_objects: self.snap_to_objects,
        };
        self.doc_gen += 1;
        Self::save_default_settings(cx, &self.doc.settings);
        cx.notify();
    }

    pub fn set_show_grid(&mut self, on: bool, cx: &mut gpui::Context<Self>) {
        self.apply_setting(0, on, cx);
    }

    pub fn set_snap_to_grid(&mut self, on: bool, cx: &mut gpui::Context<Self>) {
        self.apply_setting(1, on, cx);
    }

    pub fn set_snap_to_objects(&mut self, on: bool, cx: &mut gpui::Context<Self>) {
        self.apply_setting(2, on, cx);
    }

    // -- dimension tool --

    /// Resolves a pick sequence into a dimension target. ONE straight edge
    /// is already a complete dimension (its own length); ONE arc is a
    /// radius; two picks pair up smartly (parallel lines -> distance,
    /// crossing lines -> angle, point+line -> point-line distance). The
    /// Aligned/X/Y mode for point-pair dims is decided live by the cursor
    /// during placement (dim_placement), not here.
    fn resolve_dim_target(&self, picks: &[DimPick]) -> Option<crate::core::constraints::DimTarget> {
        use crate::core::constraints::{DimMode, DimTarget};
        use crate::core::document::SegmentKind;
        let is_arc = |l: crate::core::ids::SegmentId| {
            self.doc
                .segment(l)
                .is_some_and(|s| s.kind == SegmentKind::Arc)
        };
        // An arc's circumcenter is a REAL document point — distance dims
        // to an arc run to its center (Fusion-style), not to a chord.
        let arc_center = |l: crate::core::ids::SegmentId| -> Option<PointId> {
            self.doc.segment(l)?.center
        };
        match picks {
            // A single pick already completes for lines/arcs: an edge
            // measures its own length; an arc measures its radius.
            [DimPick::Line(l)] => {
                if is_arc(*l) {
                    Some(DimTarget::Radius { seg: *l })
                } else {
                    let seg = self.doc.segment(*l)?;
                    Some(DimTarget::Points { a: seg.start, b: seg.end, mode: DimMode::Aligned })
                }
            }
            [DimPick::Point(a), DimPick::Point(b)] => {
                Some(DimTarget::Points { a: *a, b: *b, mode: DimMode::Aligned })
            }
            [DimPick::Point(p), DimPick::Line(l)]
            | [DimPick::Line(l), DimPick::Point(p)] => {
                if is_arc(*l) {
                    // Point + arc: distance from the point to the arc's
                    // center (a radius dim would ignore the point).
                    match arc_center(*l) {
                        Some(c) => Some(DimTarget::Points { a: *p, b: c, mode: DimMode::Aligned }),
                        None => Some(DimTarget::Radius { seg: *l }),
                    }
                } else {
                    Some(DimTarget::PointLine { p: *p, line: *l })
                }
            }
            [DimPick::Line(a), DimPick::Line(b)] => {
                if is_arc(*a) && is_arc(*b) {
                    return None;
                }
                if is_arc(*a) {
                    // Line + arc: perpendicular distance from the arc's
                    // center to the line.
                    match arc_center(*a) {
                        Some(c) => return Some(DimTarget::PointLine { p: c, line: *b }),
                        None => return Some(DimTarget::Radius { seg: *a }),
                    }
                }
                if is_arc(*b) {
                    match arc_center(*b) {
                        Some(c) => return Some(DimTarget::PointLine { p: c, line: *a }),
                        None => return Some(DimTarget::Radius { seg: *b }),
                    }
                }
                let (ga, gb) = (self.doc.segment_geom(*a)?, self.doc.segment_geom(*b)?);
                let (u1, _) = dims::dim_axes(ga.1.x - ga.0.x, ga.1.y - ga.0.y);
                let (u2, _) = dims::dim_axes(gb.1.x - gb.0.x, gb.1.y - gb.0.y);
                let sin = u1.0 * u2.1 - u1.1 * u2.0;
                if sin.abs() < 1e-3 {
                    Some(DimTarget::Lines { a: *a, b: *b })
                } else {
                    Some(DimTarget::Angle { a: *a, b: *b })
                }
            }
            _ => None,
        }
    }

    /// Placement geometry for a dimension target from the current cursor
    /// position: (mode, offset, slide, measured value), all in doc units.
    /// For angles the offset is SIGNED: its side picks which supplementary
    /// sweep the arc occupies. For point-pair dims the CURSOR DECIDES THE
    /// SEMANTICS: axis-aligned edges never offer their zero span (vertical
    /// -> Y, horizontal -> X); slanted pairs give Aligned inside the ±30°
    /// perpendicular cone around the edge, Y when the cursor sits
    /// left/right of center, X when above/below. The mode is part of the
    /// result — callers must apply it via DimTarget::with_mode or it never
    /// reaches the render.
    fn dim_placement(
        &self,
        target: crate::core::constraints::DimTarget,
        cursor: Point2,
    ) -> Option<(crate::core::constraints::DimMode, f64, f64, f64)> {
        use crate::core::constraints::{DimMode, DimTarget};
        Some(match target {
            DimTarget::Points { a, b, .. } => {
                let (pa, pb) = (self.doc.point(a)?, self.doc.point(b)?);
                let (u, n) = dims::dim_axes(pb.x - pa.x, pb.y - pa.y);
                let rel = (cursor.x - pa.x, cursor.y - pa.y);
                let len = pick::distance(pa, pb);
                let dx = pb.x - pa.x;
                let dy = pb.y - pa.y;
                // Mode from where the cursor sits relative to the pair's
                // midpoint (not its first endpoint — endpoint-relative zones
                // slide around as the pair moves and feel arbitrary).
                //  - axis-aligned edges never offer the zero span: a vertical
                //    edge is ALWAYS Y (height), a horizontal edge ALWAYS X
                //    (width), wherever the cursor is;
                //  - slanted pairs: the perpendicular cone around the edge
                //    (±30° of the normal) gives the Aligned displacement;
                //  - elsewhere left/right of center means height (Y) and
                //    above/below means width (X).
                let mid = Point2::new((pa.x + pb.x) / 2., (pa.y + pb.y) / 2.);
                let vm = (cursor.x - mid.x, cursor.y - mid.y);
                let is_vertical =
                    dy.abs() > 1e-9 && dx.abs() <= dy.abs() * 0.0875;
                let is_horizontal =
                    dx.abs() > 1e-9 && dy.abs() <= dx.abs() * 0.0875;
                let mode = if is_vertical {
                    DimMode::Y
                } else if is_horizontal {
                    DimMode::X
                } else {
                    let vm_len = (vm.0 * vm.0 + vm.1 * vm.1).sqrt();
                    let cos_perp = if vm_len < 1e-9 {
                        1.0
                    } else {
                        ((vm.0 * n.0 + vm.1 * n.1).abs()) / vm_len
                    };
                    if cos_perp > 0.866 {
                        DimMode::Aligned
                    } else if vm.0.abs() >= vm.1.abs() {
                        DimMode::Y
                    } else {
                        DimMode::X
                    }
                };
                let along = rel.0 * u.0 + rel.1 * u.1;
                let perp = rel.0 * n.0 + rel.1 * n.1;
                let (offset, slide, measured) = match mode {
                    DimMode::Aligned => (
                        perp,
                        along.clamp(0., len),
                        len,
                    ),
                    DimMode::X => (
                        // Dim line rides horizontally at the cursor's height
                        // above the pair; slide along the X span.
                        rel.1,
                        (rel.0 * dx.signum()).clamp(0., dx.abs()),
                        dx.abs(),
                    ),
                    DimMode::Y => (
                        rel.0,
                        (rel.1 * dy.signum()).clamp(0., dy.abs()),
                        dy.abs(),
                    ),
                };
                (mode, offset, slide, measured)
            }
            DimTarget::PointLine { p, line } => {
                let sp = self.doc.point(p)?;
                let (la, lb) = self.doc.segment_geom(line)?;
                let (u, n) = dims::dim_axes(lb.x - la.x, lb.y - la.y);
                // Offset: cursor's signed distance from the line (side
                // only — the value is the point's own distance); slide:
                // along it, clamped to the line.
                let rel = (cursor.x - la.x, cursor.y - la.y);
                let prel = (sp.x - la.x, sp.y - la.y);
                let measured = prel.0 * n.0 + prel.1 * n.1;
                let len = pick::distance(la, lb);
                (
                    DimMode::Aligned,
                    rel.0 * n.0 + rel.1 * n.1,
                    (rel.0 * u.0 + rel.1 * u.1).clamp(0., len),
                    measured.abs(),
                )
            }
            DimTarget::Lines { a, b } => {
                let (la, _) = self.doc.segment_geom(a)?;
                let (lb0, lb1) = self.doc.segment_geom(b)?;
                let (u, n) = dims::dim_axes(lb1.x - lb0.x, lb1.y - lb0.y);
                let rel = (cursor.x - la.x, cursor.y - la.y);
                let gap = (lb0.x - la.x) * n.0 + (lb0.y - la.y) * n.1;
                let len = pick::distance(la, lb0) + 0.;
                (
                    DimMode::Aligned,
                    rel.0 * n.0 + rel.1 * n.1,
                    (rel.0 * u.0 + rel.1 * u.1).clamp(0., len),
                    gap.abs(),
                )
            }
            DimTarget::Angle { a, b } => {
                let (v, _da, sweep, frac, r) =
                    dims::dim_angle_geometry(self, a, b, Some(cursor), 0., 0., 0.)?;
                let _ = (v, _da);
                // measured = SIGNED sweep in degrees; offset = plain radius.
                (
                    DimMode::Aligned,
                    r,
                    frac,
                    sweep * 180.0 / std::f64::consts::PI,
                )
            }
            DimTarget::Radius { seg } => {
                let Some(seg_d) = self.doc.segment(seg) else {
                    return None;
                };
                let (Some(a), Some(b)) =
                    (self.doc.point(seg_d.start), self.doc.point(seg_d.end))
                else {
                    return None;
                };
                let Some(c) = seg_d.ctrl.and_then(|id| self.doc.point(id)) else {
                    return None;
                };
                let Some((center, r)) = crate::editor::arc::circumcircle(a, b, c) else {
                    return None;
                };
                // Container rides the center->bend line at the cursor's
                // radial fraction.
                let frac = if r > 1e-9 {
                    (pick::distance(cursor, center) / r).clamp(0.25, 1.0)
                } else {
                    1.0
                };
                (DimMode::Aligned, r, frac, r)
            }
        })
    }

    /// Value-input keystrokes: Enter commits (typed value or measured) and
    /// RESHAPES the geometry to match — the affected constraint component
    /// is freed (soft-anchored) and the solver snaps it to the new value.
    /// Esc cancels, Backspace edits, digits/.-/build the buffer. Returns
    /// true when the frame changed.
    pub fn dim_input_key(&mut self, key: &str) -> bool {
        use crate::core::constraints::DimTarget;
        let mut input = match self.dim_input.take() {
            Some(input) => input,
            None => return false,
        };
        match key {
            "enter" => {
                // Angle dims: value carries the SIGNED sweep (degrees) -
                // typing replaces the magnitude, keeping the placed side.
                let is_angle = matches!(input.target, DimTarget::Angle { .. });
                let old_existing_value = input.existing.and_then(|idx| self.doc.dimensions.get(idx).map(|d| d.value));
                let typed = input.buffer.parse::<f64>().ok();
                let value = if is_angle {
                    let mag = typed.map(|t| t.abs()).unwrap_or(input.measured.abs());
                    input.measured.signum() * mag
                } else {
                    typed.unwrap_or(input.measured)
                };
                if is_angle {
                    input.measured = value;
                }
                self.dim_picks.clear();
                self.dim_target = None;
                let applied = match input.existing {
                    Some(idx) => {
                        if let Some(dim) = self.doc.dimensions.get_mut(idx) {
                            dim.value = value;
                            if is_angle {
                                dim.sweep = value;
                            }
                        }
                        self.reapply_dimension(idx)
                    }
                    None => {
                        let dim = crate::core::constraints::Dimension {
                            target: input.target,
                            value,
                            offset: input.offset,
                            slide: input.slide,
                            sweep: if is_angle { value } else { 0. },
                        };
                        self.try_apply_dimension(dim)
                    }
                };
                if applied {
                    return true;
                }
                // Keep the editor alive when a typed value cannot be solved.
                // Previously this branch cleared the pending input and
                // switched to Move, making a failed typed slanted dimension
                // look like the Enter key deleted it. Restore an existing
                // value and leave the input visible so the user can correct
                // the number or cancel explicitly with Esc.
                if let (Some(idx), Some(old)) = (input.existing, old_existing_value) {
                    if let Some(dim) = self.doc.dimensions.get_mut(idx) { dim.value = old; }
                }
                self.overconstrained = true;
                self.dim_input = Some(input);
                true
            }
            "escape" => {
                self.dim_picks.clear();
                self.dim_target = None;
                // One Esc from the edit goes all the way back to Move.
                self.set_tool(Tool::Move);
                true
            }
            "backspace" => {
                input.buffer.pop();
                self.dim_input = Some(input);
                true
            }
            k => {
                let ok = k.len() == 1
                    && k.chars()
                        .next()
                        .map(|c| c.is_ascii_digit() || c == '.' || c == '-')
                        .unwrap_or(false);
                if ok {
                    input.buffer.push_str(k);
                    self.dim_input = Some(input);
                } else {
                    self.dim_input = Some(input);
                }
                ok
            }
        }
    }

    /// Pushes a new dimension and immediately enforces it: the constraint
    /// component owning the referenced geometry is freed (soft-anchored at
    /// its current positions) and the solver stretches it minimally to
    /// satisfy the new equation. Infeasible (over-constrained) placements
    /// show the modal and leave the document untouched.
    fn try_apply_dimension(&mut self, dim: crate::core::constraints::Dimension) -> bool {
        let mut trial = self.doc.clone();
        trial.dimensions.push(dim);
        self.solve_and_apply(trial, dim.target)
    }

    /// Re-solves after an EDITED dimension value on the stored dimension
    /// `idx` (already mutated in place).
    fn reapply_dimension(&mut self, idx: usize) -> bool {
        let trial = self.doc.clone();
        let target = trial.dimensions[idx].target;
        self.solve_and_apply(trial, target)
    }

    /// Runs the trial solve for `trial` (which already carries the dimension
    /// under test). On success the solved document replaces the live one;
    /// on failure the over-constrained modal comes up and nothing changes.
    fn solve_and_apply(
        &mut self,
        mut trial: crate::core::document::Document,
        target: crate::core::constraints::DimTarget,
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
            crate::core::constraints::DimTarget::Lines { a, b } | crate::core::constraints::DimTarget::Angle { a, b } => {
                let mut v = Vec::new();
                for sid in [a, b] {
                    if let Some(seg) = trial.segment(sid) {
                        v.push(seg.start);
                        v.push(seg.end);
                    }
                }
                v
            }
            crate::core::constraints::DimTarget::Radius { seg } => {
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
        let pins: Vec<(PointId, Point2)> = aux
            .iter()
            .copied()
            .min_by(|a, b| {
                (a.1.y, a.1.x)
                    .partial_cmp(&(b.1.y, b.1.x))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .into_iter()
            .collect();
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
        if (!direct_distance && solution.max_lin_residual > 0.5) || solution.max_angle_residual > 0.02 {
            self.overconstrained = true;
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
        }
        self.flush_pending_history();
        true
    }

    fn is_constraint_tool(&self) -> bool {
        matches!(
            self.tool,
            Tool::ConstraintHorizontalVertical
                | Tool::ConstraintTangent
                | Tool::ConstraintCoincident
                | Tool::ConstraintParallel
        )
    }

    fn constraint_tool_click(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        let p = self.cursor_doc(cursor);
        let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        // At a shared endpoint the generic picker intentionally prefers a
        // point. Constraint tools need the underlying edge when the cursor
        // is on that edge, otherwise a line endpoint can hide the arc (or
        // vice versa) and tangent receives the same element twice.
        let hit = if matches!(self.tool, Tool::ConstraintTangent | Tool::ConstraintHorizontalVertical) {
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
                let arc = self.constraint_picks.iter().find_map(|e| e.as_segment()).and_then(|sid| {
                    self.doc.segment(sid).filter(|s| s.kind == crate::core::document::SegmentKind::Arc).map(|_| sid)
                });
                if let (Some(line), Some(arc)) = (line, arc) {
                    let point = self.tangent_contact_point(line, arc, p);
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
                                self.doc.segment(arc).and_then(|s| s.center).and_then(|id| self.doc.point(id)),
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
                        self.doc.add_tangent_constraint(line, arc, point);
                        self.solve_constraint_now(&[ElementRef::Segment(line), ElementRef::Segment(arc)]);
                    }
                    self.selection = self.constraint_picks.clone();
                    self.constraint_picks.clear();
                }
            }
            Tool::ConstraintParallel => {
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
                    self.doc.add_parallel_constraint(foundation, moving);
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

    fn solve_constraint_now(&mut self, elements: &[ElementRef]) {
        let mut ids = Vec::new();
        for &element in elements {
            for id in self.doc.element_points(element) {
                if !ids.contains(&id) { ids.push(id); }
            }
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
        if solution.max_lin_residual <= 0.5 && solution.max_angle_residual <= 0.02 {
            for (id, pos) in solution.positions {
                self.doc.move_point(id, pos);
            }
            self.doc_gen += 1;
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

    /// Projects the free end of each tangent line onto the exact tangent at
    /// the stored arc contact. This keeps the relationship exact after
    /// radius edits and ordinary point drags without asking the solver to
    /// optimize a poorly conditioned circle/line equation.
    fn enforce_tangencies(&mut self) {
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

    /// Opens the value input for an already-placed dimension (second tap or
    /// double-click): the container freezes, the current value shows
    /// highlighted, typing replaces it, Enter re-applies.
    fn begin_dim_edit(&mut self, idx: usize) {
        if let Some(dim) = self.doc.dimensions.get(idx).copied() {
            self.selected_dim = Some(idx);
            self.dim_input = Some(DimInput {
                target: dim.target,
                offset: dim.offset,
                slide: dim.slide,
                measured: dim.value,
                buffer: String::new(),
                existing: Some(idx),
            });
        }
    }

    /// The placed dimension container under the cursor, if any.
    pub fn dim_at(&self, cursor: gpui::Point<gpui::Pixels>) -> Option<usize> {
        let (x, y) = (f64::from(cursor.x) as f32, f64::from(cursor.y) as f32);
        self.dim_hitboxes
            .iter()
            .rev()
            .find(|(_, [rx, ry, rw, rh])| {
                x >= *rx && x <= *rx + *rw && y >= *ry && y <= *ry + *rh
            })
            .map(|(idx, _)| *idx)
    }

    /// Repositions a placed dimension by dragging its container. Placement
    /// only — values never change from a drag.
    fn dim_drag_update(&mut self, idx: usize, cursor: gpui::Point<gpui::Pixels>) {
        use crate::core::constraints::DimTarget;
        let cur = self.cursor_doc(cursor);
        let Some(dim) = self.doc.dimensions.get(idx).copied() else {
            return;
        };
        match dim.target {
            DimTarget::Points { a, b, mode } => {
                let (Some(pa), Some(pb)) = (self.doc.point(a), self.doc.point(b)) else {
                    return;
                };
                let (u, n) = dims::dim_axes(pb.x - pa.x, pb.y - pa.y);
                let rel = (cur.x - pa.x, cur.y - pa.y);
                let len = pick::distance(pa, pb);
                let dx = pb.x - pa.x;
                let dy = pb.y - pa.y;
                let dim = &mut self.doc.dimensions[idx];
                // The stored mode is kept on drag (flipping modes of a live
                // constraint would re-solve the geometry mid-gesture); the
                // placement follows the cursor within that mode's frame.
                match mode {
                    crate::core::constraints::DimMode::Aligned => {
                        dim.offset = rel.0 * n.0 + rel.1 * n.1;
                        dim.slide = (rel.0 * u.0 + rel.1 * u.1).clamp(0., len);
                    }
                    crate::core::constraints::DimMode::X => {
                        dim.offset = rel.1;
                        dim.slide = (rel.0 * dx.signum()).clamp(0., dx.abs());
                    }
                    crate::core::constraints::DimMode::Y => {
                        dim.offset = rel.0;
                        dim.slide = (rel.1 * dy.signum()).clamp(0., dy.abs());
                    }
                }
            }
            DimTarget::PointLine { line, .. } => {
                let Some((la, lb)) = self.doc.segment_geom(line) else {
                    return;
                };
                let (u, _) = dims::dim_axes(lb.x - la.x, lb.y - la.y);
                let rel = (cur.x - la.x, cur.y - la.y);
                let len = pick::distance(la, lb);
                self.doc.dimensions[idx].slide = (rel.0 * u.0 + rel.1 * u.1).clamp(0., len);
            }
            DimTarget::Lines { a, .. } => {
                let Some((la, lb)) = self.doc.segment_geom(a) else {
                    return;
                };
                let (u, _) = dims::dim_axes(lb.x - la.x, lb.y - la.y);
                let rel = (cur.x - la.x, cur.y - la.y);
                let len = pick::distance(la, lb);
                self.doc.dimensions[idx].slide = (rel.0 * u.0 + rel.1 * u.1).clamp(0., len);
            }
            DimTarget::Angle { a, b } => {
                // Radius + fraction follow the cursor; the stored sweep
                // (sign + magnitude) is untouched by the drag.
                if let Some((_, _, _, frac, r)) = dims::dim_angle_geometry(
                    self,
                    a,
                    b,
                    Some(cur),
                    dim.sweep.to_radians(),
                    dim.offset.abs(),
                    dim.slide,
                ) {
                    let dim = &mut self.doc.dimensions[idx];
                    dim.offset = r;
                    dim.slide = frac;
                }
            }
            DimTarget::Radius { seg } => {
                // Container slides along the center->bend line.
                let Some(seg_d) = self.doc.segment(seg) else {
                    return;
                };
                let (Some(a), Some(b)) =
                    (self.doc.point(seg_d.start), self.doc.point(seg_d.end))
                else {
                    return;
                };
                let Some(c) = seg_d.ctrl.and_then(|id| self.doc.point(id)) else {
                    return;
                };
                let Some((center, r)) = crate::editor::arc::circumcircle(a, b, c) else {
                    return;
                };
                let frac = if r > 1e-9 {
                    (pick::distance(cur, center) / r).clamp(0.25, 1.0)
                } else {
                    1.0
                };
                self.doc.dimensions[idx].slide = frac;
            }
        }
    }

    /// Esc with the dimension tool: drop the value input first, then the
    /// accumulated picks, then leave the tool entirely. Returns true when
    /// anything changed.
    pub fn dim_escape(&mut self) -> bool {
        if self.dim_input.take().is_some() {
            self.dim_picks.clear();
            self.dim_target = None;
            return true;
        }
        if self.dim_target.is_some() || !self.dim_picks.is_empty() {
            self.dim_picks.clear();
            self.dim_target = None;
            return true;
        }
        false
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
        // Pen handle pull (or plain pen click) ends here: the gesture is
        // over but the open chain stays armed for the next anchor.
        if self.tool == Tool::Pen
            && self.pending_pen.as_ref().is_some_and(|p| p.pulling)
        {
            if let Some(pending) = self.pending_pen.as_mut() {
                pending.pulling = false;
            }
            self.flush_pending_history();
            return true;
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
                if shift {
                    self.maybe_add_tangent(seg, b);
                }
                self.selection = vec![ElementRef::Segment(seg)];
                self.pending_line = Some(PendingLine { start: b, cursor: b, anchor: None });
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

    fn maybe_add_tangent(&mut self, line: crate::core::ids::SegmentId, contact: Point2) {
        let Some(line_seg) = self.doc.segment(line) else { return; };
        let arcs: Vec<_> = self.doc.all_segments()
            .filter(|(_, s)| s.kind == crate::core::document::SegmentKind::Arc)
            .map(|(id, s)| (id, s))
            .collect();
        for (arc_id, arc) in arcs {
            let (Some(a), Some(b), Some(ctrl), Some(lp_start), Some(lp_end)) = (
                self.doc.point(arc.start), self.doc.point(arc.end),
                arc.ctrl.and_then(|id| self.doc.point(id)),
                self.doc.point(line_seg.start), self.doc.point(line_seg.end),
            ) else { continue };
            let Some((o, r)) = crate::editor::arc::circumcircle(a, b, ctrl) else { continue };
            let on_circle = |p: Point2| {
                let dx = p.x - o.x; let dy = p.y - o.y;
                let d = (dx * dx + dy * dy).sqrt();
                d > 1e-9 && (d - r).abs() <= self.snap_tol_doc() * 1.5
            };
            let point = if on_circle(lp_start) {
                Some(line_seg.start)
            } else if on_circle(lp_end) {
                Some(line_seg.end)
            } else { None };
            if let Some(point) = point {
                self.doc.add_tangent_constraint(line, arc_id, point);
                // At an arc endpoint the new line has a separate point id;
                // bond it to the arc endpoint so later radius edits carry
                // the tangent contact along with the arc.
                let endpoint = [arc.start, arc.end].into_iter()
                    .min_by(|x, y| {
                        let anchor = self.doc.point(point).unwrap_or(contact);
                        let px = self.doc.point(*x).unwrap_or(anchor);
                        let py = self.doc.point(*y).unwrap_or(anchor);
                        pick::distance(px, anchor).partial_cmp(&pick::distance(py, anchor)).unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some(endpoint) = endpoint
                    && self.doc.point(point).zip(self.doc.point(endpoint)).is_some_and(|(p, e)| pick::distance(p, e) <= self.snap_tol_doc() * 1.5)
                    && endpoint != point
                {
                    self.doc.add_constraint(ConstraintKind::Coincident, point, endpoint);
                }
                return;
            }
        }
    }

    pub(crate) fn tangent_snap_for_line(&self, start: Point2, cursor: Point2) -> Option<Point2> {
        let mut best: Option<(f64, Point2)> = None;
        for (_, arc) in self.doc.all_segments().filter(|(_, s)| s.kind == crate::core::document::SegmentKind::Arc) {
            let (Some(a), Some(b), Some(ctrl)) = (self.doc.point(arc.start), self.doc.point(arc.end), arc.ctrl.and_then(|id| self.doc.point(id))) else { continue };
            let Some((o, r)) = crate::editor::arc::circumcircle(a, b, ctrl) else { continue };
            let wx = start.x - o.x; let wy = start.y - o.y;
            let d = (wx * wx + wy * wy).sqrt();
            // If the line starts on the arc, there is one tangent direction
            // at that point. The old external-tangent construction rejected
            // this exact endpoint case, which is the common CAD workflow.
            if (d - r).abs() <= self.snap_tol_doc() {
                let ux = -wy / d; let uy = wx / d;
                let length = pick::distance(start, cursor).max(self.snap_tol_doc());
                for side in [-1.0, 1.0] {
                    let q = Point2::new(start.x + ux * length * side, start.y + uy * length * side);
                    let score = pick::distance(q, cursor);
                    // Once Shift is held on a point already on an arc, the
                    // tangent is the primary direction lock—not the generic
                    // 45-degree line snap. Pick the nearer of the two rays
                    // without requiring pixel-perfect mouse placement.
                    if best.map_or(true, |(s, _)| score < s) {
                        best = Some((score, q));
                    }
                }
                continue;
            }
            if d < r { continue; }
            let ux = wx / d; let uy = wy / d;
            let alpha = -r * r / d;
            let beta = (r * r - alpha * alpha).sqrt();
            for side in [-1.0, 1.0] {
                let q = Point2::new(o.x + ux * alpha - uy * beta * side, o.y + uy * alpha + ux * beta * side);
                let score = pick::distance(q, cursor);
                let on_arc = crate::editor::arc::samples_through(a, b, ctrl, 96)
                    .iter().map(|p| pick::distance(*p, q)).fold(f64::INFINITY, f64::min)
                    <= (r / 96.0).max(self.snap_tol_doc());
                if on_arc && score <= self.snap_tol_doc() * 2.0 && best.map_or(true, |(s, _)| score < s) {
                    best = Some((score, q));
                }
            }
        }
        best.map(|(_, q)| q)
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

    // -- pen tool (docs/pen-tool.md) --

    /// Pushes an element to a layer unless it is already listed.
    fn push_layer_once(&mut self, layer_id: u64, el: ElementRef) {
        if let Some(layer) = self.doc.layer_mut(layer_id)
            && !layer.elements.contains(&el)
        {
            layer.elements.push(el);
        }
    }

    /// Weld lookup: nearest document point within snap tolerance, for the
    /// pen's share-ids-silently rule. None = free space, create fresh.
    fn pen_weld_target(&self, at: Point2) -> Option<PointId> {
        let tol = self.snap_tol_doc();
        let mut best: Option<(f64, PointId)> = None;
        for (pid, p) in self.doc.all_points() {
            let d = pick::distance(p, at);
            if d <= tol && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, pid));
            }
        }
        best.map(|(_, pid)| pid)
    }

    /// This path's handle pair for an anchor, if the anchor belongs to it.
    fn pen_anchor_handles(&self, pid: PathId, anchor: PointId) -> Option<(PointId, PointId)> {
        let path = self.doc.path(pid)?;
        let anchors = self.doc.path_anchors(pid)?;
        let i = anchors.iter().position(|&a| a == anchor)?;
        path.handles.get(i).copied()
    }

    /// Index of an anchor in the first path owning it (for continuity and
    /// close-target lookups).
    fn path_anchor_index(&self, anchor: PointId) -> Option<(PathId, usize)> {
        for (pid, _) in self.doc.all_paths() {
            if let Some(anchors) = self.doc.path_anchors(pid)
                && let Some(i) = anchors.iter().position(|&a| a == anchor)
            {
                return Some((pid, i));
            }
        }
        None
    }

    /// True while anything else alive references the point: segments,
    /// path pairs, constraints, dimensions, or other paths' anchors.
    fn point_in_use(&self, pid: PointId, ignore_path: Option<PathId>) -> bool {
        self.doc.all_segments().any(|(_, s)| {
            s.start == pid
                || s.end == pid
                || s.ctrl == Some(pid)
                || s.center == Some(pid)
                || s.handle_out == Some(pid)
                || s.handle_in == Some(pid)
        }) || self.doc.all_paths().any(|(id, p)| {
            id != ignore_path.unwrap_or(PathId::NONE)
                && (p.handles.iter().any(|&(h0, h1)| h0 == pid || h1 == pid)
                    || self
                        .doc
                        .path_anchors(id)
                        .is_some_and(|anchors| anchors.contains(&pid)))
        }) || self.doc.constraints.iter().any(|c| c.a == pid || c.b == pid)
            || self.doc.dimensions.iter().any(|d| match d.target {
                DimTarget::Points { a, b, .. } => a == pid || b == pid,
                DimTarget::PointLine { p, .. } => p == pid,
                _ => false,
            })
    }

    /// Commits one pen anchor at the snapped cursor: weld-shares the point
    /// when it locks onto real geometry, appends a segment to the live
    /// path (or starts one), closes on a clean click of the start anchor.
    /// Arms the handle pull for click-drag bends.
    fn commit_pen_anchor(&mut self, at: Point2) {
        use crate::core::document::ContinuityMode;
        let layer_id = self.doc.layers[0].id;
        // Weld first: the shared point IS the anchor (no coincident chip).
        let (anchor, fresh) = match self.pen_weld_target(at) {
            Some(pid) => (pid, false),
            None => (self.doc.add_point(at), true),
        };
        let apos = self.doc.point(anchor).unwrap_or(at);
        // Every anchor owns a handle pair from birth (collapsed here; the
        // pull or later edits extend it).
        let h0 = self.doc.add_point(apos);
        let h1 = self.doc.add_point(apos);
        let mut pending = self.pending_pen.take();
        // The live chain may have died underneath us (Delete key while
        // drawing): a dangling path or tip restarts the chain fresh.
        if let Some(p) = &pending {
            let path_ok = p.path.is_none_or(|id| self.doc.path(id).is_some());
            if !path_ok || self.doc.point(p.last).is_none() {
                pending = None;
            }
        }
        match pending {
            None => {
                self.push_layer_once(layer_id, ElementRef::Point(anchor));
                self.push_layer_once(layer_id, ElementRef::Point(h0));
                self.push_layer_once(layer_id, ElementRef::Point(h1));
                self.pending_pen = Some(PendingPen {
                    path: None,
                    last: anchor,
                    cursor: at,
                    pulling: true,
                    last_fresh: fresh,
                    last_handles: (h0, h1),
                });
                self.selection = vec![ElementRef::Point(anchor)];
            }
            Some(pending) => {
                // Close on a clean click of the start anchor (shared ID =
                // genuinely closed loop). Needs a real chain behind it.
                let start_of = pending.path.and_then(|pid| {
                    self.doc.path(pid).and_then(|p| {
                        p.segments
                            .first()
                            .and_then(|&sid| self.doc.segment(sid).map(|s| s.start))
                    })
                });
                if !fresh
                    && Some(anchor) == start_of
                    && anchor != pending.last
                    && let Some(path_id) = pending.path
                {
                    let (from, from_pair) = (pending.last, pending.last_handles);
                    self.doc.remove_point(h0);
                    self.doc.remove_point(h1);
                    // Closing segment: out of the tip, into the start
                    // anchor's own incoming handle.
                    let first_in = self
                        .pen_anchor_handles(path_id, anchor)
                        .map(|(_, h1)| h1)
                        .unwrap_or(anchor);
                    let sid =
                        self.doc.add_bezier_segment(from, from_pair.0, first_in, anchor);
                    self.push_layer_once(layer_id, ElementRef::Segment(sid));
                    self.close_pen_path(path_id, sid);
                    // A pulled tip smooths itself, like any other joint.
                    self.smooth_pen_joint_if_bent(path_id, from);
                    self.selection = vec![ElementRef::Path(path_id)];
                    self.pending_pen = None;
                    return;
                }
                // Clicking the active anchor itself is a no-op (keeps the
                // chain armed for the next click elsewhere).
                if anchor == pending.last && !fresh {
                    // Weld hit our own tip: drop the spare pair, stay armed.
                    self.doc.remove_point(h0);
                    self.doc.remove_point(h1);
                    self.pending_pen = Some(PendingPen {
                        pulling: true,
                        ..pending
                    });
                    return;
                }
                if anchor == pending.last && fresh {
                    // Fresh duplicate of our own tip (shouldn't normally
                    // happen — weld radius); fold it back in.
                    self.doc.remove_point(anchor);
                    self.pending_pen = Some(PendingPen {
                        last_handles: pending.last_handles,
                        pulling: true,
                        ..pending
                    });
                    return;
                }
                // Normal link: segment last -> anchor.
                let (from, from_pair) = (pending.last, pending.last_handles);
                let sid = self.doc.add_bezier_segment(from, from_pair.0, h1, anchor);
                self.push_layer_once(layer_id, ElementRef::Point(anchor));
                self.push_layer_once(layer_id, ElementRef::Point(h0));
                self.push_layer_once(layer_id, ElementRef::Point(h1));
                self.push_layer_once(layer_id, ElementRef::Segment(sid));
                let path_id = match pending.path {
                    Some(pid) => {
                        if let Some(path) = self.doc.path_mut(pid) {
                            path.segments.push(sid);
                            path.continuity.push(ContinuityMode::Corner);
                            path.handles.push((h0, h1));
                        }
                        pid
                    }
                    None => {
                        let pid = self.doc.add_path(
                            vec![sid],
                            false,
                            vec![ContinuityMode::Corner, ContinuityMode::Corner],
                            vec![pending.last_handles, (h0, h1)],
                        );
                        self.push_layer_once(layer_id, ElementRef::Path(pid));
                        pid
                    }
                };
                // A pulled joint smooths itself: extended handles imply it.
                self.smooth_pen_joint_if_bent(path_id, from);
                self.selection = vec![ElementRef::Segment(sid)];
                self.pending_pen = Some(PendingPen {
                    path: Some(path_id),
                    last: anchor,
                    cursor: at,
                    pulling: true,
                    last_fresh: fresh,
                    last_handles: (h0, h1),
                });
            }
        }
    }

    /// Appends the closing segment and seals the path. The continuity vec
    /// already runs parallel to the closed anchors (open length n+1 for n
    /// segments becomes n+1 anchors for n+1 segments), so only the flag
    /// flips; the start anchor keeps its mode.
    fn close_pen_path(&mut self, path_id: PathId, closing: SegmentId) {
        if let Some(path) = self.doc.path_mut(path_id) {
            path.segments.push(closing);
            path.closed = true;
        }
    }

    /// Click-drag on a fresh pen anchor pulls symmetric handles out of it:
    /// the far handle rides the cursor, the near one mirrors. Below a small
    /// screen-space threshold the pair stays collapsed (a plain click is a
    /// sharp Corner anchor). Shift constrains the pull to 45-degree steps.
    /// Continuity follows at the next commit (smooth_if_bent).
    fn update_pen_pull(&mut self, cursor: Point2, shift: bool) {
        let (last, pair) = match &self.pending_pen {
            Some(p) if p.pulling => (p.last, p.last_handles),
            _ => return,
        };
        let Some(a) = self.doc.point(last) else {
            return;
        };
        if let Some(pending) = self.pending_pen.as_mut() {
            pending.cursor = cursor;
        }
        let mut v = Point2::new(cursor.x - a.x, cursor.y - a.y);
        if shift {
            let snapped = tools::snap_angle(a, cursor);
            v = Point2::new(snapped.x - a.x, snapped.y - a.y);
        }
        let len = (v.x * v.x + v.y * v.y).sqrt();
        if len * self.camera.zoom < 5. {
            self.doc.move_point(pair.0, a);
            self.doc.move_point(pair.1, a);
            return;
        }
        self.doc
            .move_point(pair.0, Point2::new(a.x + v.x, a.y + v.y));
        self.doc
            .move_point(pair.1, Point2::new(a.x - v.x, a.y - v.y));
    }

    /// A joint smooths itself when its outgoing handle left the anchor: an
    /// extended handle implies curvature, so Corner would lie.
    fn smooth_pen_joint_if_bent(&mut self, path_id: PathId, joint: PointId) {
        use crate::core::document::ContinuityMode;
        let bent = match self.pen_anchor_handles(path_id, joint) {
            Some((h0, _)) => self
                .doc
                .point(joint)
                .zip(self.doc.point(h0))
                .is_some_and(|(a, h)| pick::distance(a, h) > 1e-6),
            None => false,
        };
        if !bent {
            return;
        }
        // Only joints of THIS path, same index (sequential borrows).
        let Some((_, i)) = self.path_anchor_index(joint) else {
            return;
        };
        if !self
            .doc
            .path_anchors(path_id)
            .is_some_and(|anchors| anchors.get(i) == Some(&joint))
        {
            return;
        }
        if let Some(path) = self.doc.path_mut(path_id)
            && let Some(mode) = path.continuity.get_mut(i)
        {
            *mode = ContinuityMode::Smooth;
        }
    }

    /// Abandons the open chain (tool switch / Esc): a lone fresh anchor
    /// with no segments is removed again, everything else stays drawn.
    fn abort_pending_pen(&mut self) {
        let Some(pending) = self.pending_pen.take() else {
            return;
        };
        if pending.path.is_none() && pending.last_fresh {
            let (h0, h1) = pending.last_handles;
            self.doc.remove_point(pending.last);
            self.doc.remove_point(h0);
            self.doc.remove_point(h1);
        }
    }

    /// Backspace while drawing: pops the last anchor, the chain stays live
    /// on the new tip. Shared (welded) points survive; only truly orphaned
    /// points are removed.
    pub(crate) fn pop_pen_anchor(&mut self) -> bool {
        let Some(pending) = self.pending_pen.take() else {
            return false;
        };
        let Some(path_id) = pending.path else {
            // Lone anchor, no segments yet.
            if pending.last_fresh {
                let (h0, h1) = pending.last_handles;
                self.doc.remove_point(pending.last);
                self.doc.remove_point(h0);
                self.doc.remove_point(h1);
            }
            self.selection.clear();
            return true;
        };
        // Drop the last segment record first (manual splice: remove_segment
        // would reset the surviving continuity modes).
        let last_sid = self
            .doc
            .path(path_id)
            .and_then(|p| p.segments.last().copied());
        let Some(sid) = last_sid else {
            self.doc.remove_path(path_id);
            self.pending_pen = None;
            self.selection.clear();
            return true;
        };
        if let Some(path) = self.doc.path_mut(path_id) {
            path.segments.retain(|&s| s != sid);
            let want = path.segments.len() + !path.closed as usize;
            path.continuity.truncate(want);
            path.handles.pop();
        }
        // Segment record + layer cleanup without touching the path again
        // (it no longer lists this segment).
        self.doc.remove_segment(sid);
        self.doc.detach_from_layers(ElementRef::Segment(sid));
        // The popped tip and its pair die only as orphans.
        let tip = pending.last;
        let (h0, h1) = pending.last_handles;
        for pid in [tip, h0, h1] {
            if !self.point_in_use(pid, Some(path_id)) {
                self.doc.remove_point(pid);
                self.doc.detach_from_layers(ElementRef::Point(pid));
            }
        }
        // Re-arm on the new tip, or unwind fully when nothing remains.
        let anchors = self.doc.path_anchors(path_id).unwrap_or_default();
        match anchors.last().copied() {
            Some(new_tip) => {
                let pair = self
                    .pen_anchor_handles(path_id, new_tip)
                    .unwrap_or((new_tip, new_tip));
                self.pending_pen = Some(PendingPen {
                    path: Some(path_id),
                    last: new_tip,
                    cursor: self.doc.point(new_tip).unwrap_or(pending.cursor),
                    pulling: false,
                    last_fresh: false,
                    last_handles: pair,
                });
                self.selection = vec![ElementRef::Point(new_tip)];
            }
            None => {
                self.doc.remove_path(path_id);
                self.pending_pen = None;
                self.selection.clear();
            }
        }
        true
    }

    /// Double-click an anchor: Corner becomes Smooth, anything else becomes
    /// Corner — in every path sharing the anchor.
    fn toggle_anchor_continuity(&mut self, anchor: PointId) -> bool {
        use crate::core::document::ContinuityMode;
        let mut touched = false;
        let owners: Vec<(PathId, usize)> = self
            .doc
            .all_paths()
            .filter_map(|(pid, _)| {
                self.doc.path_anchors(pid).and_then(|anchors| {
                    anchors
                        .iter()
                        .position(|&a| a == anchor)
                        .map(|i| (pid, i))
                })
            })
            .collect();
        for (pid, i) in owners {
            if let Some(path) = self.doc.path_mut(pid)
                && let Some(mode) = path.continuity.get_mut(i)
            {
                *mode = if *mode == ContinuityMode::Corner {
                    ContinuityMode::Smooth
                } else {
                    ContinuityMode::Corner
                };
                touched = true;
            }
        }
        touched
    }

    /// Click a curve: exact De Casteljau split inserts a junction anchor
    /// that inherits the start anchor's mode. The drawn shape is unchanged
    /// to the pixel; the chain continues from its live tip.
    fn insert_pen_anchor(&mut self, sid: SegmentId, at: Point2) -> bool {
        use crate::core::document::ContinuityMode;
        let seg = self.doc.segment(sid).filter(|s| {
            s.kind == crate::core::document::SegmentKind::Bezier
        });
        let Some(seg) = seg else { return false };
        let Some(path_id) = self.path_containing(sid) else {
            return false;
        };
        let Some((p0, p1, p2, p3)) = self.doc.bezier_geom(sid) else {
            return false;
        };
        let (t, s) = crate::editor::bezier::param_of_closest(at, p0, p1, p2, p3);
        if t <= 1e-3 || t >= 1. - 1e-3 {
            return false;
        }
        let ((_, q0, r0, _), (_, r1, q2, _)) =
            crate::editor::bezier::split(p0, p1, p2, p3, t);
        let layer_id = self.doc.layers[0].id;
        // Existing handles slide to their subdivision positions; the three
        // new points are the junction anchor and the inner pair.
        if let Some(h) = seg.handle_out {
            self.doc.move_point(h, q0);
        }
        if let Some(h) = seg.handle_in {
            self.doc.move_point(h, q2);
        }
        let ns = self.doc.add_point(s);
        let nr0 = self.doc.add_point(r0);
        let nr1 = self.doc.add_point(r1);
        // Foreign beziers might lack stored handles; collapse replacements
        // onto the endpoints so the split stays total.
        let outer_out = seg.handle_out.unwrap_or_else(|| self.doc.add_point(p0));
        let outer_in = seg.handle_in.unwrap_or_else(|| self.doc.add_point(p3));
        let left = self.doc.add_bezier_segment(seg.start, outer_out, nr0, ns);
        let right = self.doc.add_bezier_segment(ns, nr1, outer_in, seg.end);
        for el in [
            ElementRef::Point(ns),
            ElementRef::Point(nr0),
            ElementRef::Point(nr1),
            ElementRef::Segment(left),
            ElementRef::Segment(right),
        ] {
            self.push_layer_once(layer_id, el);
        }
        // Splice: swap the segment for the halves, inherit the start mode.
        if let Some(path) = self.doc.path_mut(path_id) {
            if let Some(pos) = path.segments.iter().position(|&x| x == sid) {
                path.segments.splice(pos..=pos, [left, right]);
                let mode = path
                    .continuity
                    .get(pos)
                    .copied()
                    .unwrap_or(ContinuityMode::Corner);
                // Junction anchor index = pos + 1 in anchor order.
                if path.continuity.len() >= pos + 1 {
                    path.continuity.insert(pos + 1, mode);
                }
                path.handles.insert(pos + 1, (nr0, nr1));
            }
        }
        self.doc.remove_segment(sid);
        self.doc.detach_from_layers(ElementRef::Segment(sid));
        self.selection = vec![ElementRef::Point(ns)];
        true
    }

    /// Deletes an element from the document and clears it from selection.
    pub fn delete_element(&mut self, el: ElementRef) {
        match el {
            ElementRef::Point(p) => {
                self.doc.remove_point(p);
            }
            ElementRef::Segment(s) => {
                // Dimensions measuring the deleted segment die with it; a
                // length dim whose BOTH endpoints lost every segment is
                // measuring gone geometry and dies too. (Without this the
                // dims pin their points alive and deleting a rectangle
                // leaves its corners + dims floating.)
                use crate::core::constraints::DimTarget as _DT;
                self.doc
                    .dimensions
                    .retain(|d| match &d.target {
                        _DT::PointLine { line, .. } | _DT::Radius { seg: line } => *line != s,
                        _DT::Lines { a, b } | _DT::Angle { a, b } => *a != s && *b != s,
                        _DT::Points { .. } => true,
                    });
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
                        if let Some(h) = seg.handle_out {
                            v.push(h);
                        }
                        if let Some(h) = seg.handle_in {
                            v.push(h);
                        }
                        v
                    })
                    .unwrap_or_default();
                self.doc.remove_segment(s);
                for pid in ends {
                    let still_used = self.doc.all_segments().any(|(_, seg)| {
                        seg.start == pid
                            || seg.end == pid
                            || seg.handle_out == Some(pid)
                            || seg.handle_in == Some(pid)
                    })
                        || self.doc.constraints.iter().any(|c| c.a == pid || c.b == pid)
                        || self.doc.dimensions.iter().any(|d| {
                            matches!(
                                d.target,
                                crate::core::constraints::DimTarget::Points { a, b, .. }
                                    if a == pid || b == pid
                            ) || matches!(
                                d.target,
                                crate::core::constraints::DimTarget::PointLine { p, .. } if p == pid
                            )
                        })
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
                // Length dims whose endpoints lost every segment measured
                // gone geometry - drop them now that the endpoint cleanup
                // has run.
                let segless: Vec<PointId> = self
                    .doc
                    .all_points()
                    .filter(|(pid, _)| {
                        !self.doc.all_segments().any(|(_, seg)| {
                            seg.start == *pid || seg.end == *pid
                        })
                    })
                    .map(|(pid, _)| pid)
                    .collect();
                self.doc.dimensions.retain(|d| match &d.target {
                    crate::core::constraints::DimTarget::Points { a, b, .. } => {
                        !(segless.contains(a) && segless.contains(b))
                    }
                    _ => true,
                });
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
            ElementRef::Path(p) => {
                // Deleting a path takes its segments, anchors and handles
                // with it. Points shared with surviving geometry (welded
                // paths, coincident joins) stay alive: the segment cleanup
                // below only removes truly orphaned points.
                let seg_ids: Vec<crate::core::ids::SegmentId> = self
                    .doc
                    .path(p)
                    .map(|path| path.segments.clone())
                    .unwrap_or_default();
                let pts = self.doc.element_points(ElementRef::Path(p));
                self.doc.remove_path(p);
                self.doc
                    .constraints
                    .retain(|c| !(pts.contains(&c.a) && pts.contains(&c.b)));
                for s in seg_ids {
                    // The path record is already gone, so the splice inside
                    // remove_segment finds nothing to do.
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
            ElementRef::Fill(f) => self.doc.fill(f).is_some(),
            ElementRef::Path(p) => self.doc.path(p).is_some(),
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
            ElementRef::Path(p) => self.doc.path(p).is_some(),
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
        // The document was swapped wholesale; live chain ids may dangle.
        // No geometry cleanup: the snapshot already owns the truth.
        self.pending_pen = None;
        self.pending_via_click = false;
        self.dragging = None;
        self.marquee = None;
        self.deferred_pick = None;
        self.group_drag_last = None;
        self.arc_center_reveal.clear();
    }

    // True when the element itself, or anything SELECTED that contains it,
    // covers it — a corner shared by selected edges counts as selected.
    fn element_selected(&self, el: ElementRef) -> bool {
        if self.selection.contains(&el) {
            return true;
        }
        match el {
            ElementRef::Segment(sid) => {
                self.fill_containing(sid)
                    .is_some_and(|f| self.selection.contains(&ElementRef::Fill(f)))
                    || self.path_containing(sid)
                        .is_some_and(|p| self.selection.contains(&ElementRef::Path(p)))
            }
            ElementRef::Point(pid) => self.selection.iter().any(|sel| match *sel {
                ElementRef::Segment(s) => self
                    .doc
                    .segment(s)
                    .is_some_and(|seg| seg.start == pid || seg.end == pid),
                ElementRef::Fill(f) => {
                    self.doc.element_points(ElementRef::Fill(f)).contains(&pid)
                }
                ElementRef::Path(p) => {
                    self.doc.element_points(ElementRef::Path(p)).contains(&pid)
                }
                _ => false,
            }),
            ElementRef::Path(pid) => self.selection.iter().any(|sel| match *sel {
                ElementRef::Point(p) => self
                    .doc
                    .element_points(ElementRef::Path(pid))
                    .contains(&p),
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

    fn path_containing(&self, sid: crate::core::ids::SegmentId) -> Option<crate::core::ids::PathId> {
        self.doc
            .all_paths()
            .find(|(_, p)| p.segments.contains(&sid))
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
