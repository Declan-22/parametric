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

use std::cell::RefCell;

pub use camera::Camera;

pub use snapping::SnapGuide;
pub use tools::{DimInput, DimPick, PenMode, PendingBezier, PendingCircle, PendingLine, PendingPen, PendingRuler, PendingShape, Tool};

use crate::core::constraints::{ConstraintKind, DimTarget, ElementRef};
use crate::core::document::{Document, Layer};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{FillId, PointId, SegmentId};

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
    // Arc body grab: the whole arc translates rigidly (see plan_arc_drag).
    // None for every other gesture (solver path).
    pub arc_body_scale: Option<SegmentId>,
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
    // Persistent tessellation cache used by the canvas paint pass. Interior
    // mutability keeps rendering read-only with respect to editor state while
    // allowing unchanged geometry to skip resampling.
    pub(crate) render_cache: RefCell<crate::ui::canvas::paint::RenderCache>,
    pub camera: Camera,
    pub tool: Tool,
    pub pending_shape: Option<PendingShape>,
    pub pending_ruler: Option<PendingRuler>,
    pub pending_line: Option<PendingLine>,
    // Existing edge and prospective line geometry for the creation preview.
    pub perpendicular_preview: Option<(SegmentId, Point2, Point2)>,
    pub pending_circle: Option<PendingCircle>,
    // Pen tool: ONE unified path tool. `pen_anchor` is the live chain point
    // shared by every sub-mode — switching Line/Bezier/Arc never breaks the
    // path, it only changes what the NEXT span draws. `pending_pen` stages
    // the in-progress span for the active mode. `pen_down_at` tracks the
    // press point so a second-press drag can shape bezier handles.
    pub pen_mode: PenMode,
    pub pending_pen: Option<PendingPen>,
    pub pending_bezier: Option<PendingBezier>,
    pub pen_anchor: Option<Point2>,
    /// Document id of the chain anchor: chained spans MERGE their touching
    /// endpoints into this id (one shared point per joint), so lines,
    /// arcs and beziers chain as a single path and joints show both
    /// handles (prev far handle + next near handle).
    pub pen_anchor_id: Option<PointId>,
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
    /// Explicit dimension-type lock from the floating menu:
    /// width/height/displacement/distance. When Some, `dim_placement`
    /// honors it instead of the cursor-zone auto-pick.
    pub dim_mode_lock: Option<String>,
    // Per-frame angle-dimension render data (dashed arc + container).
    pub angle_dim_renders: Vec<dims::AngleDimRender>,
    // Per-frame curve-length render data (offset replica polyline +
    // container). Bezier-only.
    pub curve_dim_renders: Vec<dims::CurveDimRender>,
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
pub(crate) const SNAP_TOL_PX: f64 = 10.0;

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
            render_cache: RefCell::new(Default::default()),
            camera: Camera::new(),
            tool: Tool::Move,
            pending_shape: None,
            pending_ruler: None,
            pending_line: None,
            perpendicular_preview: None,
            pending_circle: None,
            pen_mode: PenMode::Line,
            pending_pen: None,
            pending_bezier: None,
            pen_anchor: None,
            pen_anchor_id: None,
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
            dim_mode_lock: None,
            angle_dim_renders: Vec::new(),
            curve_dim_renders: Vec::new(),
            doc_gen: 0,
            dim_hitboxes: Vec::new(),
            hovered_dim: None,
            selected_dim: None,
            dim_drag: None,
            dim_caret_visible: true,
            overconstrained: false,
            arc_center_reveal: Vec::new(),
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
        self.perpendicular_preview = None;
        self.pending_circle = None;
        self.pending_pen = None;
        self.pending_bezier = None;
        self.pen_anchor = None;
        self.pen_anchor_id = None;
        self.pending_via_click = false;
        self.selection.clear();
        self.constraint_picks.clear();
        self.constraint_point_picks.clear();
        self.selected_constraints.clear();
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

    /// Switches the pen sub-mode (explicit only — drag never changes it).
    /// The chain anchor is PRESERVED so the path never breaks: the new
    /// mode stages its next span from the same live endpoint.
    pub fn set_pen_mode(&mut self, mode: PenMode) -> bool {
        if self.pen_mode == mode {
            return false;
        }
        self.pen_mode = mode;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        // Re-stage the new mode from the live chain point (if any) so
        // Line -> Bezier -> Arc flows as one path. A half-finished arc
        // chord is dropped — its two clicks belong to the old mode.
        // Bezier restaging pre-mirrors the near handle (smooth preview).
        self.pending_pen = self.pen_anchor.map(|a| {
            let mut pen = PendingPen::for_mode(mode, a);
            if mode == PenMode::Bezier
                && let Some(pb) = pen.bezier.as_mut()
            {
                pb.h1 = self.pen_mirror_handle();
            }
            pen
        });
        self.pending_bezier = None;
        true
    }

    /// Cancels the live pen path entirely (Esc).
    pub fn cancel_pen_path(&mut self) -> bool {
        if self.pending_pen.is_none() && self.pen_anchor.is_none() {
            return false;
        }
        self.pending_pen = None;
        self.pending_bezier = None;
        self.pen_anchor = None;
        self.pen_anchor_id = None;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        true
    }

    /// Merges a freshly committed span's start point into the live chain
    /// anchor id (when one exists) and advances the anchor to the span's
    /// end point. Every span type chains through shared ids — no stacked
    /// duplicate points at joints.
    fn pen_chain_end(&mut self, seg: SegmentId) {
        let Some(s) = self.doc.segment(seg) else { return };
        let (fresh_start, end) = (s.start, s.end);
        if let Some(aid) = self.pen_anchor_id {
            if self.doc.point(aid).is_some() && fresh_start != aid {
                self.doc.merge_point(aid, fresh_start);
            }
        }
        self.pen_anchor_id = Some(end);
        if let Some(p) = self.doc.point(end) {
            self.pen_anchor = Some(p);
        }
    }

    /// Mirror of the previous bezier's far handle across the chain anchor:
    /// the default near handle for a chained bezier span (smooth joints
    /// out of the box). Prefers the span ARRIVING at the anchor (its far
    /// handle) over one leaving it. None when no bezier touches it.
    fn pen_mirror_handle(&self) -> Option<Point2> {
        let aid = self.pen_anchor_id?;
        let anchor = self.doc.point(aid)?;
        let mut fallback: Option<Point2> = None;
        for (_, s) in self.doc.all_segments() {
            if s.kind != crate::core::document::SegmentKind::Bezier {
                continue;
            }
            // Arriving span first: its far handle mirrors into our near one.
            if s.end == aid
                && let Some(hp) = s.center.and_then(|h| self.doc.point(h))
                && pick::distance(hp, anchor) > 1e-6
            {
                return Some(Point2::new(
                    2. * anchor.x - hp.x,
                    2. * anchor.y - hp.y,
                ));
            }
            if s.start == aid && fallback.is_none() {
                fallback = s.ctrl.and_then(|h| self.doc.point(h)).and_then(|hp| {
                    (pick::distance(hp, anchor) > 1e-6).then_some(Point2::new(
                        2. * anchor.x - hp.x,
                        2. * anchor.y - hp.y,
                    ))
                });
            }
        }
        fallback
    }

    /// Explicit dimension-type lock from the floating menu.
    pub fn set_dim_mode_lock(&mut self, lock: Option<String>) -> bool {
        if self.dim_mode_lock == lock {
            return false;
        }
        self.dim_mode_lock = lock;
        self.refresh_dim_target();
        true
    }

    /// Drops the lock when the armed target can't honor it, so the menu
    /// never shows a stale restriction.
    fn drop_stale_dim_lock(&mut self) {
        if !Self::dim_lock_keeps(&self.dim_target, &self.dim_mode_lock) {
            self.dim_mode_lock = None;
        }
    }

    /// Whether the current lock can apply to an armed target. Radius,
    /// angle, gap and point-line spans imply their own type and lift any
    /// lock; CurveLength only honors an explicit distance lock; EdgeMid
    /// honors the matching width/height/displacement lock.
    fn dim_lock_keeps(target: &Option<crate::core::constraints::DimTarget>, lock: &Option<String>) -> bool {
        use crate::core::constraints::{DimMode, DimTarget};
        match target {
            Some(DimTarget::Points { .. }) => true,
            Some(DimTarget::CurveLength { .. }) => {
                lock.as_deref() == Some("distance")
            }
            Some(DimTarget::EdgeMid { mode, .. }) => {
                let want = match mode {
                    DimMode::X => "width",
                    DimMode::Y => "height",
                    DimMode::Aligned => "displacement",
                };
                lock.as_deref() == Some(want)
            }
            Some(_) => false,
            None => true,
        }
    }

    /// Whether two segments cross (sine of their directions above epsilon).
    /// Shared by pick resolution and the Angle/Distance kind switch.
    fn geoms_cross(
        ga: (crate::core::geometry::Point2, crate::core::geometry::Point2),
        gb: (crate::core::geometry::Point2, crate::core::geometry::Point2),
    ) -> bool {
        let (u1, _) = dims::dim_axes(ga.1.x - ga.0.x, ga.1.y - ga.0.y);
        let (u2, _) = dims::dim_axes(gb.1.x - gb.0.x, gb.1.y - gb.0.y);
        (u1.0 * u2.1 - u1.1 * u2.0).abs() >= 1e-3
    }

    /// Whether two segments' directions cross (false for parallel pairs
    /// and missing geometry).
    pub(crate) fn lines_cross(
        &self,
        a: crate::core::ids::SegmentId,
        b: crate::core::ids::SegmentId,
    ) -> bool {
        match (self.doc.segment_geom(a), self.doc.segment_geom(b)) {
            (Some(ga), Some(gb)) => Self::geoms_cross(ga, gb),
            _ => false,
        }
    }

    /// Angle <-> midpoint-displacement switch for a picked edge pair.
    /// `to_angle=true` arms Angle{a,b} (crossing pairs only); false arms
    /// EdgeMid{a,b,Aligned} — the midpoint displacement between the edges.
    /// Type locks are cleared (neither target honors them). There is no
    /// gap dimension: parallel pairs measure midpoint to midpoint.
    /// Returns true on change.
    pub fn set_dim_edge_kind(&mut self, to_angle: bool) -> bool {
        use crate::core::constraints::{DimMode, DimTarget};
        let pair = match self.dim_target {
            Some(DimTarget::Lines { a, b })
            | Some(DimTarget::Angle { a, b })
            | Some(DimTarget::EdgeMid { a, b, .. }) => Some((a, b)),
            _ => None,
        };
        let Some((a, b)) = pair else {
            return false;
        };
        let is_angle = matches!(self.dim_target, Some(DimTarget::Angle { .. }));
        if to_angle == is_angle {
            return false;
        }
        if to_angle && !self.lines_cross(a, b) {
            return false;
        }
        self.dim_target = Some(if to_angle {
            DimTarget::Angle { a, b }
        } else {
            DimTarget::EdgeMid {
                a,
                b,
                mode: DimMode::Aligned,
            }
        });
        self.dim_mode_lock = None;
        true
    }

    /// Toggles the armed edge-pair dimension between Angle and midpoint
    /// displacement. Parallel pairs stay displacement (no angle exists).
    pub fn toggle_dim_edge_kind(&mut self) -> bool {
        use crate::core::constraints::DimTarget;
        match self.dim_target {
            Some(DimTarget::Angle { .. }) => self.set_dim_edge_kind(false),
            Some(DimTarget::Lines { .. }) | Some(DimTarget::EdgeMid { .. }) => {
                self.set_dim_edge_kind(true)
            }
            _ => false,
        }
    }

    /// Re-derives the armed dimension target after a lock change so the
    /// pending type follows the new restriction instead of sticking to
    /// whatever was armed before (e.g. Distance -> Width on one bezier).
    /// A lock the picks can't satisfy (e.g. Width on an arc or an armed
    /// edge pair) is dropped immediately so the menu never shows a stale
    /// restriction. No-op while a value input is open or nothing is
    /// picked yet.
    fn refresh_dim_target(&mut self) {
        if self.dim_input.is_some() || self.dim_picks.is_empty() {
            return;
        }
        self.dim_target = self.resolve_dim_target(&self.dim_picks);
        self.drop_stale_dim_lock();
        // Mirror the picks as selection so they stay highlighted.
        self.selection = self
            .dim_picks
            .iter()
            .map(|p| match p {
                DimPick::Point(id) => ElementRef::Point(*id),
                DimPick::Line(id) => ElementRef::Segment(*id),
            })
            .collect();
    }

    // -- floating-menu constraint gating (single source of truth) --
    //
    // The associated functions below decide which constraint rows a
    // selection can actually take. The menu calls them (as
    // `Editor::gate_*`) to decide what to SHOW; the apply arms call the
    // same functions, so a shown row can never be a dead click. Each
    // encodes real feasibility, not shape counting:
    //   - same-segment point pairs are ineligible (collapsing an edge),
    //   - merge needs close, unglued points (the old bond-popup rule),
    //   - tangent/parallel/perpendicular consult existing H/V locks,
    //   - coincident checks forced-coordinate conflicts.

    /// Valid bare-point selections (live positions only).
    pub(crate) fn gate_points(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Vec<PointId> {
        selection
            .iter()
            .filter_map(|el| el.as_point())
            .filter(|id| doc.point(*id).is_some())
            .collect()
    }

    /// Valid explicitly-selected segments.
    pub(crate) fn gate_segs(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Vec<SegmentId> {
        selection
            .iter()
            .filter_map(|el| el.as_segment())
            .filter(|id| doc.segment(*id).is_some())
            .collect()
    }

    /// First segment owning pid as an endpoint (arena order).
    pub(crate) fn owner_segment(doc: &Document, pid: PointId) -> Option<SegmentId> {
        doc.all_segments()
            .find(|(_, s)| s.start == pid || s.end == pid)
            .map(|(id, _)| id)
    }

    /// True when one segment owns both points as its endpoints (gluing
    /// them would collapse that edge — never offered).
    pub(crate) fn same_segment_owner(doc: &Document, a: PointId, b: PointId) -> bool {
        doc.all_segments().any(|(_, s)| {
            (s.start == a && s.end == b) || (s.start == b && s.end == a)
        })
    }

    /// Coordinates forced onto pid by the H/V/coincident network, if any
    /// (first found per axis; point-on-edge relations don't fix a coord).
    /// Used to reject pairs whose existing constraints already force
    /// different positions on the constrained axis.
    pub(crate) fn required_coords(doc: &Document, pid: PointId) -> (Option<f64>, Option<f64>) {
        use std::collections::HashSet;
        let mut seen: HashSet<PointId> = HashSet::new();
        let mut stack = vec![pid];
        let (mut rx, mut ry) = (None, None);
        while let Some(p) = stack.pop() {
            if !seen.insert(p) {
                continue;
            }
            for c in &doc.constraints {
                let other = if c.a == p {
                    Some(c.b)
                } else if c.b == p {
                    Some(c.a)
                } else {
                    None
                };
                let Some(o) = other else { continue };
                let Some(op) = doc.point(o) else { continue };
                match c.kind {
                    ConstraintKind::Horizontal => {
                        if ry.is_none() {
                            ry = Some(op.y);
                        }
                        stack.push(o);
                    }
                    ConstraintKind::Vertical => {
                        if rx.is_none() {
                            rx = Some(op.x);
                        }
                        stack.push(o);
                    }
                    ConstraintKind::Coincident if c.point_on_segment.is_none() => {
                        if rx.is_none() {
                            rx = Some(op.x);
                        }
                        if ry.is_none() {
                            ry = Some(op.y);
                        }
                        stack.push(o);
                    }
                    _ => {}
                }
            }
            if rx.is_some() && ry.is_some() {
                break;
            }
        }
        (rx, ry)
    }

    /// Locked direction of a line whose endpoints carry a matching H/V
    /// constraint, if any.
    pub(crate) fn locked_dir(doc: &Document, sid: SegmentId) -> Option<(f64, f64)> {
        let s = doc.segment(sid)?;
        if s.kind != crate::core::document::SegmentKind::Line {
            return None;
        }
        doc.constraints.iter().find_map(|c| {
            let pair = (c.a == s.start && c.b == s.end) || (c.a == s.end && c.b == s.start);
            if !pair {
                return None;
            }
            match c.kind {
                ConstraintKind::Horizontal => Some((1.0, 0.0)),
                ConstraintKind::Vertical => Some((0.0, 1.0)),
                _ => None,
            }
        })
    }

    /// Curve tangent direction at the document location nearest `near`.
    /// Orientation-agnostic (callers compare with abs dot).
    pub(crate) fn curve_tangent_at(
        doc: &Document,
        sid: SegmentId,
        near: Point2,
    ) -> Option<(f64, f64)> {
        use crate::core::document::SegmentKind as SK;
        let s = doc.segment(sid)?;
        match s.kind {
            SK::Arc => {
                let (Some(a), Some(b), Some(c)) = (
                    doc.point(s.start),
                    doc.point(s.end),
                    s.ctrl.and_then(|id| doc.point(id)),
                ) else {
                    return None;
                };
                let (o, r) = crate::editor::arc::circumcircle(a, b, c)?;
                if r < 1e-9 {
                    return None;
                }
                // Project `near` onto the circle, then perpendicular.
                let (dx, dy) = (near.x - o.x, near.y - o.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                Some((-dy / l, dx / l))
            }
            SK::Bezier => {
                let (h1, h2) = s.bezier_handles();
                let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                    doc.point(s.start),
                    h1.and_then(|id| doc.point(id)),
                    h2.and_then(|id| doc.point(id)),
                    doc.point(s.end),
                ) else {
                    return None;
                };
                Some(crate::editor::bezier::nearest_on_curve(p0, c1, c2, p1, near, 48).2)
            }
            _ => None,
        }
    }

    /// HV candidate: (a, b, segment-if-from-line). Explicit line segments
    /// win; else two bare points that don't share one segment and whose
    /// inferred axis isn't already forced apart.
    pub(crate) fn hv_candidates(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(PointId, PointId, Option<SegmentId>)> {
        use crate::core::document::SegmentKind as SK;
        if let Some(sid) = selection.iter().find_map(|el| el.as_segment()) {
            if let Some(s) = doc.segment(sid) {
                if s.kind == SK::Line
                    && doc.point(s.start).is_some()
                    && doc.point(s.end).is_some()
                {
                    return Some((s.start, s.end, Some(sid)));
                }
            }
        }
        let pts = Self::gate_points(doc, selection);
        if pts.len() < 2 {
            return None;
        }
        let (a, b) = (pts[0], pts[1]);
        if Self::same_segment_owner(doc, a, b) {
            return None;
        }
        let (Some(pa), Some(pb)) = (doc.point(a), doc.point(b)) else {
            return None;
        };
        // Same dominant-axis inference as the apply path.
        let horizontal = (pa.y - pb.y).abs() <= (pa.x - pb.x).abs();
        let (ra, rb) = (
            Self::required_coords(doc, a),
            Self::required_coords(doc, b),
        );
        let conflict = if horizontal {
            matches!((ra.1, rb.1), (Some(x), Some(y)) if (x - y).abs() > 1e-6)
        } else {
            matches!((ra.0, rb.0), (Some(x), Some(y)) if (x - y).abs() > 1e-6)
        };
        if conflict {
            return None;
        }
        Some((a, b, None))
    }

    /// Whether gluing a and b is compatible with forced coordinates.
    fn coincident_feasible(doc: &Document, a: PointId, b: PointId) -> bool {
        let (ra, rb) = (
            Self::required_coords(doc, a),
            Self::required_coords(doc, b),
        );
        let x_ok = match (ra.0, rb.0) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-6,
            _ => true,
        };
        let y_ok = match (ra.1, rb.1) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-6,
            _ => true,
        };
        x_ok && y_ok
    }

    /// Coincident candidate: explicit point pairs (never same-segment),
    /// else a lone point to its nearest selected-segment endpoint (never
    /// itself), else the nearest cross-segment endpoint pair. All pairs
    /// feasibility-checked.
    pub(crate) fn coincident_candidates(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(PointId, PointId)> {
        let points = Self::gate_points(doc, selection);
        let segs = Self::gate_segs(doc, selection);
        if points.len() >= 2 {
            let (a, b) = (points[0], points[1]);
            if a != b
                && !Self::same_segment_owner(doc, a, b)
                && Self::coincident_feasible(doc, a, b)
            {
                return Some((a, b));
            }
            return None;
        }
        let mut ends: Vec<(usize, PointId, Point2)> = Vec::new();
        for (si, sid) in segs.iter().enumerate() {
            if let Some(s) = doc.segment(*sid) {
                if let Some(p) = doc.point(s.start) {
                    ends.push((si, s.start, p));
                }
                if let Some(p) = doc.point(s.end) {
                    ends.push((si, s.end, p));
                }
            }
        }
        if points.len() == 1 && !ends.is_empty() {
            let Some(p) = doc.point(points[0]) else {
                return None;
            };
            let best = ends
                .iter()
                .filter(|(_, id, _)| *id != points[0])
                .min_by(|(_, _, a), (_, _, b)| {
                    pick::distance(*a, p)
                        .partial_cmp(&pick::distance(*b, p))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            if let Some(&(_, id, _)) = best {
                if Self::coincident_feasible(doc, points[0], id) {
                    return Some((points[0], id));
                }
            }
            return None;
        }
        if ends.len() >= 2 {
            let mut best: Option<(f64, PointId, PointId)> = None;
            for (i, &(sa, a, pa)) in ends.iter().enumerate() {
                for &(sb, b, pb) in &ends[i + 1..] {
                    if sa == sb || a == b {
                        continue;
                    }
                    let d = pick::distance(pa, pb);
                    if best.map_or(true, |(bd, _, _)| d < bd)
                        && Self::coincident_feasible(doc, a, b)
                    {
                        best = Some((d, a, b));
                    }
                }
            }
            return best.map(|(_, a, b)| (a, b));
        }
        None
    }

    /// Merge pairs: close (within tol), unglued bare-point pairs — the old
    /// bond-popup condition. Only these may merge; mass selections never
    /// collapse to one point.
    pub(crate) fn merge_candidate_pairs(
        doc: &Document,
        selection: &[ElementRef],
        tol: f64,
    ) -> Vec<(PointId, PointId)> {
        let points = Self::gate_points(doc, selection);
        let mut out = Vec::new();
        for (i, &a) in points.iter().enumerate() {
            let Some(pa) = doc.point(a) else { continue };
            for &b in &points[i + 1..] {
                if a == b {
                    continue;
                }
                let Some(pb) = doc.point(b) else { continue };
                if pick::distance(pa, pb) > tol {
                    continue;
                }
                let glued = doc.constraints.iter().any(|c| {
                    c.kind == ConstraintKind::Coincident
                        && ((c.a == a && c.b == b) || (c.a == b && c.b == a))
                });
                if glued {
                    continue;
                }
                out.push((a, b));
            }
        }
        out
    }

    /// Tangent candidate: (line, other, contact, collinear). Explicit
    /// segments first, then segments implied by bare-point owners, so two
    /// bare points on two lines can still go tangent-collinear. Ids are
    /// always distinct; contact is the joint (nearest line end to the
    /// other's ends).
    pub(crate) fn tangent_candidate(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(SegmentId, SegmentId, PointId, bool)> {
        use crate::core::document::SegmentKind as SK;
        let mut lines: Vec<SegmentId> = Vec::new();
        let mut curves: Vec<SegmentId> = Vec::new();
        let mut push_seg = |sid: SegmentId| {
            let Some(s) = doc.segment(sid) else { return };
            if s.kind == SK::Line {
                if !lines.contains(&sid) && lines.len() < 2 {
                    lines.push(sid);
                }
            } else if s.is_curve() {
                if !curves.contains(&sid) && curves.len() < 2 {
                    curves.push(sid);
                }
            }
        };
        for el in selection {
            if let Some(sid) = el.as_segment() {
                push_seg(sid);
            }
        }
        for pid in Self::gate_points(doc, selection) {
            if let Some(oid) = Self::owner_segment(doc, pid) {
                push_seg(oid);
            }
        }
        let (line, other, collinear) = if let Some(&line) = lines.first() {
            if let Some(&c) = curves.first() {
                (line, c, false)
            } else if lines.len() >= 2 && lines[1] != line {
                (line, lines[1], true)
            } else {
                return None;
            }
        } else if curves.len() >= 2 {
            (curves[0], curves[1], false)
        } else {
            return None;
        };
        let (Some(lseg), Some(cseg)) = (doc.segment(line), doc.segment(other)) else {
            return None;
        };
        let mut best: Option<(f64, PointId)> = None;
        for lid in [lseg.start, lseg.end] {
            let Some(lp) = doc.point(lid) else { continue };
            for cid in [cseg.start, cseg.end] {
                let Some(cp) = doc.point(cid) else { continue };
                let d = pick::distance(lp, cp);
                if best.map_or(true, |(bd, _)| d < bd) {
                    best = Some((d, lid));
                }
            }
        }
        let (_, contact) = best?;
        Some((line, other, contact, collinear))
    }

    /// Tangent feasibility against existing locks. Free lines always pass
    /// (the solver swings them into place). Locked lines must already ride
    /// the curve tangent (~12°); collinear pairs need parallel lock dirs.
    pub(crate) fn tangent_feasible(
        doc: &Document,
        line: SegmentId,
        other: SegmentId,
        contact: PointId,
    ) -> bool {
        use crate::core::document::SegmentKind as SK;
        let Some(oseg) = doc.segment(other) else {
            return false;
        };
        let parallel_dirs = |a: (f64, f64), b: (f64, f64)| {
            (a.0 * b.1 - a.1 * b.0).abs() < 0.05
        };
        if oseg.kind == SK::Line {
            match (Self::locked_dir(doc, line), Self::locked_dir(doc, other)) {
                (Some(a), Some(b)) => parallel_dirs(a, b),
                _ => true,
            }
        } else if oseg.kind == SK::Bezier || doc.segment(line).is_some_and(|s| s.kind == SK::Bezier) {
            true
        } else {
            let Some(d) = Self::locked_dir(doc, line) else {
                return true;
            };
            let Some(cp) = doc.point(contact) else {
                return false;
            };
            let Some(t) = Self::curve_tangent_at(doc, other, cp) else {
                return true;
            };
            (d.0 * t.0 + d.1 * t.1).abs() > 0.978
        }
    }

    /// Two distinct explicitly-selected lines, if present.
    pub(crate) fn line_pair(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(SegmentId, SegmentId)> {
        use crate::core::document::SegmentKind as SK;
        let mut lines = Vec::new();
        for el in selection {
            if let Some(sid) = el.as_segment()
                && let Some(s) = doc.segment(sid)
                && s.kind == SK::Line
                && !lines.contains(&sid)
            {
                lines.push(sid);
                if lines.len() == 2 {
                    break;
                }
            }
        }
        if lines.len() == 2 {
            Some((lines[0], lines[1]))
        } else {
            None
        }
    }

    /// Parallel/perpendicular feasibility against H/V locks. A free side
    /// always passes; two locked sides must already satisfy the relation.
    pub(crate) fn line_pair_feasible(
        doc: &Document,
        a: SegmentId,
        b: SegmentId,
        want_parallel: bool,
    ) -> bool {
        match (Self::locked_dir(doc, a), Self::locked_dir(doc, b)) {
            (Some(x), Some(y)) => {
                let cross = (x.0 * y.1 - x.1 * y.0).abs();
                let dot = (x.0 * y.0 + x.1 * y.1).abs();
                if want_parallel { cross < 0.05 } else { dot < 0.05 }
            }
            _ => true,
        }
    }

    /// Applies a constraint from the floating menu based on selection.
    /// Returns true when the document changed.
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
                }
                ok
            }
        }
    }

    /// Emits a standalone cubic bezier: 4 points + 1 stroked segment.
    /// Handles are free; only endpoints participate in constraints.
    pub fn create_bezier(
        &mut self,
        layer_id: u64,
        p0: Point2,
        c1: Point2,
        c2: Point2,
        p1: Point2,
    ) -> SegmentId {
        const BEZIER_STROKE_PX: f64 = 1.0;
        let a = self.doc.add_point(p0);
        let h1 = self.doc.add_point(c1);
        let h2 = self.doc.add_point(c2);
        let b = self.doc.add_point(p1);
        let seg = self.doc.add_bezier_segment(a, h1, h2, b);
        // Stamp stroke width directly (no dedicated setter on Document).
        if let Some(slot) = self.doc.segment_mut(seg) {
            slot.stroke_width = BEZIER_STROKE_PX;
        }
        self.doc.push_to_layer(layer_id, ElementRef::Point(a));
        self.doc.push_to_layer(layer_id, ElementRef::Point(h1));
        self.doc.push_to_layer(layer_id, ElementRef::Point(h2));
        self.doc.push_to_layer(layer_id, ElementRef::Point(b));
        self.doc.push_to_layer(layer_id, ElementRef::Segment(seg));
        seg
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

    /// Pen clicks by sub-mode. Stays in Pen (chains like the legacy line
    /// tool); `Esc` / tool switch cancels via `set_tool`.
    fn pen_tool_click(&mut self, cursor: gpui::Point<gpui::Pixels>, shift: bool) -> bool {
        let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
        self.snap_guides = guides;
        match self.pen_mode {
            PenMode::Line => self.pen_line_click(at, shift),
            PenMode::Arc => self.pen_arc_click(at, shift),
            PenMode::Bezier => self.pen_bezier_click(at),
        }
    }

    /// Commits one pen line span anchor -> b. Shared by press-commit and
    /// release-commit; re-stages the next span from b.
    fn commit_pen_line(&mut self, b: Point2, shift: bool) {
        let Some(anchor) = self.pen_anchor else { return };
        self.snap_guides.clear();
        let layer_id = self.doc.layers[0].id;
        let seg = self.create_line(layer_id, anchor, b);
        // Merge FIRST: constraint creation below must reference the live
        // (post-merge) point ids, or the constraints dangle on a destroyed
        // point and their chips never render.
        self.pen_chain_end(seg);
        if let Some((source, _, _)) = self.perpendicular_preview.take() {
            self.doc.add_perpendicular_constraint(source, seg);
        }
        if shift {
            self.maybe_add_tangent(seg, b);
            self.auto_tangent_for_pen(seg);
        }
        self.selection = vec![ElementRef::Segment(seg)];
        let chained = self.pen_anchor.unwrap_or(b);
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Line,
            line: Some(tools::PendingLine { start: chained, cursor: chained }),
            bezier: None,
            circle: None,
        });
    }

    fn pen_line_click(&mut self, at: Point2, shift: bool) -> bool {
        // Press commits the LIVE preview (mirror the legacy line tool):
        // the preview has been tracking the cursor via moves, so commit
        // anchor -> preview end. A fresh/zero-length staging re-anchors.
        if let Some(pending) = self.pending_pen.take() {
            if let Some(line) = pending.line {
                let (_, mut b) = line.snapped(shift);
                if shift {
                    if let Some(q) = self.tangent_snap_for_line(line.start, line.cursor)
                        .or_else(|| self.bezier_tangent_snap(line.start, line.cursor))
                    {
                        b = q;
                    }
                }
                if pick::distance(b, line.start) > 1e-6 {
                    self.pending_pen = Some(pending);
                    self.commit_pen_line(b, shift);
                } else {
                    // Fresh link: re-anchor at the press point.
                    let (nat, g) = self.snap_creation_point(at);
                    self.snap_guides = g;
                    self.pen_anchor = Some(nat);
                    self.pending_pen = Some(PendingPen {
                        mode: PenMode::Line,
                        line: Some(tools::PendingLine { start: nat, cursor: nat }),
                        bezier: None,
                        circle: None,
                    });
                }
                self.pending_via_click = true;
                return true;
            }
            self.pending_pen = Some(pending);
        }
        // No staging (fresh tool or just switched with no anchor): anchor.
        let (nat, g) = self.snap_creation_point(at);
        self.snap_guides = g;
        self.pen_anchor = Some(nat);
        self.pending_pen = Some(PendingPen::for_mode(PenMode::Line, nat));
        self.pending_via_click = true;
        true
    }

    fn pen_arc_click(&mut self, at: Point2, shift: bool) -> bool {
        // 3-press arc through the shared chain anchor: press1 fixes the
        // chord start at the anchor, press2 fixes the chord end, press3
        // (on-arc point) commits and chains from the chord end.
        if let Some(pending) = self.pending_pen.take() {
            if let Some(mut pc) = pending.circle {
                if pc.a.is_some() && pc.b.is_some() {
                    self.pending_via_click = false;
                    self.snap_guides.clear();
                    if let (Some(a), Some(b)) = (pc.a, pc.b) {
                        let c = pc.cursor;
                        let layer_id = self.doc.layers[0].id;
                        let seg = self.create_arc(layer_id, a, b, c);
                        // Merge before constraining (see commit_pen_line).
                        self.pen_chain_end(seg);
                        self.auto_tangent_for_pen(seg);
                        self.selection = vec![ElementRef::Segment(seg)];
                        let chained = self.pen_anchor.unwrap_or(b);
                        self.pending_pen =
                            Some(PendingPen::for_mode(PenMode::Arc, chained));
                    } else {
                        self.pending_pen = Some(pending);
                    }
                    return true;
                }
                match (&pc.a, &pc.b) {
                    (Some(_), None) => {
                        let mut nat = at;
                        if shift && let Some(a) = pc.a {
                            nat = tools::snap_angle(a, nat);
                        }
                        pc.b = Some(nat);
                        pc.cursor = nat;
                        self.pending_via_click = true;
                    }
                    _ => {
                        // Anchor the chord start (anchor wins when set).
                        let start = self.pen_anchor.unwrap_or(at);
                        pc.a = Some(start);
                        pc.cursor = at;
                        self.pen_anchor = Some(start);
                        self.pending_via_click = true;
                    }
                }
                self.pending_pen = Some(PendingPen {
                    mode: PenMode::Arc,
                    line: None,
                    bezier: None,
                    circle: Some(pc),
                });
                return true;
            }
            self.pending_pen = Some(pending);
        }
        // Fresh staging starts at the live anchor when chaining.
        let start = self.pen_anchor.unwrap_or(at);
        self.pen_anchor = Some(start);
        let mut pc = PendingCircle { a: Some(start), b: None, cursor: at };
        // Single-press anchor set: cursor stays on the press.
        if self.pen_anchor == Some(at) {
            pc.cursor = at;
        }
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Arc,
            line: None,
            bezier: None,
            circle: Some(pc),
        });
        self.pending_via_click = true;
        true
    }

    fn pen_bezier_click(&mut self, at: Point2) -> bool {
        // Press fixes geometry, release commits:
        //  - press 1 (no staging): anchor p0, await the second press;
        //  - press 2: fix the endpoint p1 = press point; the drag that
        //    follows shapes the far handle, release commits the span.
        if let Some(pending) = self.pending_pen.take() {
            if let Some(mut pb) = pending.bezier {
                // (Re-)fix the endpoint at the press point; the drag that
                // follows shapes the nearest handle, release commits.
                // The near side defaults to the smooth mirror of the
                // previous span (a later drag near that side overrides it).
                let (nat, g) = self.snap_creation_point(at);
                self.snap_guides = g;
                let mirror = self.pen_mirror_handle();
                pb.p1 = Some(nat);
                pb.h1 = mirror;
                pb.h2 = None;
                pb.cursor = nat;
                self.pending_pen = Some(PendingPen {
                    mode: PenMode::Bezier,
                    line: None,
                    bezier: Some(pb),
                    circle: None,
                });
                self.pending_via_click = true;
                return true;
            }
            self.pending_pen = Some(pending);
        }
        // Fresh staging starts at the live anchor when chaining, with the
        // near handle pre-mirrored so the preview already guesses the
        // smooth continuation curve (never a straight chord).
        let start = self.pen_anchor.unwrap_or(at);
        let (nat, g) = if self.pen_anchor.is_some() {
            (start, Vec::new())
        } else {
            self.snap_creation_point(at)
        };
        self.snap_guides = g;
        self.pen_anchor = Some(nat);
        let h1 = self.pen_mirror_handle();
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Bezier,
            line: None,
            bezier: Some(PendingBezier {
                p0: nat,
                p1: None,
                h1,
                h2: None,
                cursor: at,
                active_handle: None,
            }),
            circle: None,
        });
        self.pending_via_click = true;
        true
    }

    /// Bezier release-commit: straight span on click-click, handled span
    /// when the second press dragged (handle = release point).
    fn pen_bezier_release(&mut self) -> bool {
        let Some(pending) = self.pending_pen.take() else {
            return false;
        };
        let Some(pb) = pending.bezier else {
            self.pending_pen = Some(pending);
            return false;
        };
        let Some(p1) = pb.p1 else {
            // First click only anchored p0 — await the second press.
            self.pending_pen = Some(PendingPen {
                mode: PenMode::Bezier,
                line: None,
                bezier: Some(pb),
                circle: None,
            });
            return true;
        };
        let end = p1;
        if pick::distance(end, pb.p0) <= 1e-6 {
            // Degenerate: keep staging, await a real endpoint.
            self.pending_pen = Some(PendingPen {
                mode: PenMode::Bezier,
                line: None,
                bezier: Some(PendingBezier {
                    p0: pb.p0,
                    p1: None,
                    h1: None,
                    h2: None,
                    cursor: pb.cursor,
                    active_handle: None,
                }),
                circle: None,
            });
            return true;
        }
        let lerp = |t: f64| {
            Point2::new(
                pb.p0.x + (end.x - pb.p0.x) * t,
                pb.p0.y + (end.y - pb.p0.y) * t,
            )
        };
        // Dragged handles win per side (the far drag rides opposite, via
        // `effective`); an untouched near side mirrors the previous span's
        // far handle (smooth joints); otherwise thirds (straight).
        let mirror = self.pen_mirror_handle();
        let c1 = pb.h1.or(mirror).unwrap_or_else(|| lerp(1. / 3.));
        let c2 = pb
            .h2
            .map(|h| Point2::new(2. * end.x - h.x, 2. * end.y - h.y))
            .unwrap_or_else(|| lerp(2. / 3.));
        self.snap_guides.clear();
        let layer_id = self.doc.layers[0].id;
        let seg = self.create_bezier(layer_id, pb.p0, c1, c2, end);
        // Merge before constraining (see commit_pen_line).
        self.pen_chain_end(seg);
        self.auto_tangent_for_pen(seg);
        self.selection = vec![ElementRef::Segment(seg)];
        let chained = self.pen_anchor.unwrap_or(end);
        // Restage with the near handle already reflecting the span we
        // just laid (smooth continuation preview from the first move).
        let next_h1 = Point2::new(2. * end.x - c2.x, 2. * end.y - c2.y);
        let next_h1 = (pick::distance(next_h1, end) > 1e-6).then_some(next_h1);
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Bezier,
            line: None,
            bezier: Some(PendingBezier {
                p0: chained,
                p1: None,
                h1: next_h1,
                h2: None,
                cursor: chained,
                active_handle: None,
            }),
            circle: None,
        });
        self.pending_via_click = true;
        true
    }

    /// Release-commit for the pen (mirrors the legacy line release):
    /// a dragged preview commits, a motionless click keeps staging.
    fn pen_release_commit(&mut self, shift: bool) -> bool {
        let Some(pending) = self.pending_pen.take() else {
            return false;
        };
        match pending.mode {
            PenMode::Bezier => {
                self.pending_pen = Some(pending);
                return self.pen_bezier_release();
            }
            PenMode::Line => {
                let Some(line) = pending.line else {
                    self.pending_pen = Some(pending);
                    return false;
                };
                let (_, mut b) = line.snapped(shift);
                if shift {
                    if let Some(q) = self.tangent_snap_for_line(line.start, line.cursor)
                        .or_else(|| self.bezier_tangent_snap(line.start, line.cursor))
                    {
                        b = q;
                    }
                }
                if self.pending_via_click && pick::distance(b, line.start) <= 1e-6 {
                    self.pending_pen = Some(pending);
                    return true;
                }
                self.pending_via_click = true;
                if pick::distance(b, line.start) > 1e-6 {
                    self.pending_pen = Some(pending);
                    self.commit_pen_line(b, shift);
                } else {
                    self.pending_pen = Some(pending);
                }
                return true;
            }
            // Arcs commit on the third press, never on release.
            PenMode::Arc => {
                self.pending_pen = Some(pending);
                return false;
            }
        }
    }

    /// Auto-tangent for chained pen spans: when the new span starts where
    /// the previous span ended (within snap tolerance) and directions align
    /// (~5°), bond them with a Tangent constraint + coincident endpoints.
    fn auto_tangent_for_pen(&mut self, fresh: SegmentId) {
        let Some(cur) = self.doc.segment(fresh) else { return };
        let (Some(cp0), Some(cp1)) = (self.doc.point(cur.start), self.doc.point(cur.end)) else {
            return;
        };
        let cur_tan = self.span_start_tangent(fresh);
        let tol = self.snap_tol_doc() * 1.5;
        // Find a prior span sharing the fresh start point.
        let mut prev_id: Option<SegmentId> = None;
        for (sid, _) in self.doc.all_segments() {
            if sid == fresh {
                continue;
            }
            let Some(s) = self.doc.segment(sid) else { continue };
            if !matches!(
                s.kind,
                crate::core::document::SegmentKind::Line
                    | crate::core::document::SegmentKind::Arc
                    | crate::core::document::SegmentKind::Bezier
            ) {
                continue;
            }
            let touches = [s.start, s.end].iter().any(|&p| {
                self.doc.point(p).is_some_and(|q| pick::distance(q, cp0) <= tol)
            });
            if touches {
                prev_id = Some(sid);
                break;
            }
        }
        let Some(prev) = prev_id else { return };
        let prev_tan = self.span_end_tangent(prev);
        let (Some(a), Some(b)) = (cur_tan, prev_tan) else { return };
        // Both tangents point AWAY from the joint; incoming must oppose.
        let dot = a.0 * b.0 + a.1 * b.1;
        if dot < -0.996 {
            let prev_seg = self.doc.segment(prev);
            // Line-on-line tangency is collinearity: Parallel (+ the joint
            // is already shared by chaining). A raw Tangent constraint has
            // no line-line equation and would sit dead.
            if cur.kind == crate::core::document::SegmentKind::Line
                && prev_seg.is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line)
            {
                self.doc.add_parallel_constraint(prev, fresh);
                let _ = self.solve_constraint_now(&[
                    ElementRef::Segment(prev),
                    ElementRef::Segment(fresh),
                ]);
                return;
            }
            // Coincident joint (distinct ids from chaining).
            let pj = self.doc.segment(prev).map(|s| s.end);
            let contact = if let Some(pj) = pj
                && pj != cur.start
                && self.doc.point(pj).zip(self.doc.point(cur.start))
                    .is_some_and(|(a, b)| pick::distance(a, b) <= tol)
            {
                // Chained geometry shares a topological vertex. Merge the
                // ids instead of layering a solver Coincident constraint on
                // top of an already-connected joint.
                self.doc.merge_point(pj, cur.start);
                pj
            } else {
                cur.start
            };
            self.doc.add_tangent_constraint(prev, fresh, contact);
            let _ = self.solve_constraint_now(&[ElementRef::Segment(prev), ElementRef::Segment(fresh)]);
            let _ = cp1;
        }
    }

    fn span_start_tangent(&self, sid: SegmentId) -> Option<(f64, f64)> {
        let s = self.doc.segment(sid)?;
        let (p0, p1) = (self.doc.point(s.start)?, self.doc.point(s.end)?);
        match s.kind {
            crate::core::document::SegmentKind::Line => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                Some((dx / l, dy / l))
            }
            crate::core::document::SegmentKind::Arc => {
                let c = s.ctrl.and_then(|id| self.doc.point(id))?;
                let (o, _) = crate::editor::arc::circumcircle(p0, p1, c)?;
                let (dx, dy) = (p0.x - o.x, p0.y - o.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                // Tangent at p0, oriented away (p0 -> along sweep). Sign from
                // sweep side: use perpendicular, pick the one leaving p0.
                let (tx, ty) = (-dy / l, dx / l);
                // Orient along the arc: dot with (c - p0) tangent side.
                let mx = c.x - p0.x;
                let my = c.y - p0.y;
                let s = tx * mx + ty * my;
                Some(if s >= 0. { (tx, ty) } else { (-tx, -ty) })
            }
            crate::core::document::SegmentKind::Bezier => {
                let (h1, h2) = s.bezier_handles();
                let c1 = h1.and_then(|id| self.doc.point(id)).unwrap_or(p1);
                let c2 = h2.and_then(|id| self.doc.point(id)).unwrap_or(p0);
                Some(bezier::end_tangent(p0, c1, c2, p1, true))
            }
            _ => None,
        }
    }

    fn span_end_tangent(&self, sid: SegmentId) -> Option<(f64, f64)> {
        let s = self.doc.segment(sid)?;
        let (p0, p1) = (self.doc.point(s.start)?, self.doc.point(s.end)?);
        match s.kind {
            crate::core::document::SegmentKind::Line => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                Some((dx / l, dy / l))
            }
            crate::core::document::SegmentKind::Arc => {
                let c = s.ctrl.and_then(|id| self.doc.point(id))?;
                let (o, _) = crate::editor::arc::circumcircle(p0, p1, c)?;
                let (dx, dy) = (p1.x - o.x, p1.y - o.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                let (tx, ty) = (-dy / l, dx / l);
                let mx = c.x - p1.x;
                let my = c.y - p1.y;
                let sg = tx * mx + ty * my;
                Some(if sg >= 0. { (tx, ty) } else { (-tx, -ty) })
            }
            crate::core::document::SegmentKind::Bezier => {
                let (h1, h2) = s.bezier_handles();
                let c1 = h1.and_then(|id| self.doc.point(id)).unwrap_or(p1);
                let c2 = h2.and_then(|id| self.doc.point(id)).unwrap_or(p0);
                Some(bezier::end_tangent(p0, c1, c2, p1, false))
            }
            _ => None,
        }
    }

    /// Live pen preview tracking (called from `canvas_drag`).
    fn pen_drag_update(&mut self, cursor: gpui::Point<gpui::Pixels>, shift: bool) -> bool {
        let Some(pending) = self.pending_pen else {
            return false;
        };
        let at = self.cursor_doc(cursor);
        let (at, guides) = self.snap_creation_point(at);
        self.snap_guides = guides;
        match pending.mode {
            PenMode::Line => {
                let Some(line) = pending.line else {
                    return true;
                };
                // Free: plain object/grid snap only. Shift arms the whole
                // constraint layer — 45° lock, arc-tangent lock and the
                // perpendicular snap. Never yank the preview otherwise.
                let (tangent_at, perpendicular) = if shift {
                    let tan = self.tangent_snap_for_line(line.start, at)
                        .or_else(|| self.bezier_tangent_snap(line.start, at));
                    let perp =
                        self.perpendicular_snap_for_line(line.start, tan.unwrap_or(at));
                    (tan, perp)
                } else {
                    (None, None)
                };
                let final_at =
                    perpendicular.map(|(p, _)| p).unwrap_or(tangent_at.unwrap_or(at));
                self.perpendicular_preview =
                    perpendicular.map(|(p, sid)| (sid, line.start, p));
                if let Some(dst) = self.pending_pen.as_mut().and_then(|p| p.line.as_mut()) {
                    dst.cursor = final_at;
                }
            }
            PenMode::Arc => {
                let Some(mut pc) = pending.circle else {
                    return true;
                };
                let (nat, shifted) =
                    Self::arc_creation_shift(pc.stage(), pc.a, pc.b, at, shift);
                if shifted {
                    self.snap_guides.clear();
                }
                pc.cursor = nat;
                if let Some(dst) = self.pending_pen.as_mut().and_then(|p| p.circle.as_mut()) {
                    *dst = pc;
                }
            }
            PenMode::Bezier => {
                let Some(mut pb) = pending.bezier else {
                    return true;
                };
                // Once grabbed, stay on the chosen handle throughout the drag gesture.
                // Do not switch handles mid-drag even if cursor moves closer to the other point.
                if pb.p1.is_some() {
                    let handle = pb.active_handle.unwrap_or_else(|| {
                        let (d0, d1) = (
                            pick::distance(at, pb.p0),
                            pb.p1.map(|p| pick::distance(at, p)).unwrap_or(f64::MAX),
                        );
                        if d1 <= d0 { 2 } else { 1 }
                    });
                    pb.active_handle = Some(handle);
                    if handle == 2 {
                        pb.h2 = Some(at);
                    } else {
                        pb.h1 = Some(at);
                    }
                }
                pb.cursor = at;
                if let Some(dst) = self.pending_pen.as_mut().and_then(|p| p.bezier.as_mut()) {
                    *dst = pb;
                }
            }
        }
        // Tangent preview: the preview span leaves the anchor along an
        // existing span's tangent — dashed ray off the anchor. Commit
        // applies the real constraint (chip appears then).
        if let (Some(anchor), Some((_, out))) = (self.pen_anchor, self.pen_preview_tangent()) {
            let ray = 28. / self.camera.zoom;
            self.snap_guides.push(snapping::SnapGuide {
                vertical: false,
                from: anchor,
                to: Point2::new(anchor.x + out.0 * ray, anchor.y + out.1 * ray),
                kind: snapping::SnapKind::Edge,
                solid: true,
                linked: false,
                span_is_x: false,
                span_lo: 0.,
                span_hi: 0.,
            });
        }
        true
    }

    /// Preview tangent alignment for the live pen span: the existing span
    /// id when the preview leaves the chain anchor along that span's own
    /// tangent (~5°). The drag layer draws a dashed guide off the anchor;
    /// commit applies the real Tangent constraint via
    /// `auto_tangent_for_pen` (whose chip then appears).
    fn pen_preview_tangent(&self) -> Option<(SegmentId, (f64, f64))> {
        let aid = self.pen_anchor_id?;
        if self.doc.point(aid).is_none() {
            return None;
        }
        let pending = self.pending_pen?;
        let unit = |v: (f64, f64)| {
            let l = (v.0 * v.0 + v.1 * v.1).sqrt();
            if l < 1e-6 { None } else { Some((v.0 / l, v.1 / l)) }
        };
        let out: Option<(f64, f64)> = match pending.mode {
            PenMode::Line => {
                let l = pending.line?;
                unit((l.cursor.x - l.start.x, l.cursor.y - l.start.y))
            }
            PenMode::Bezier => {
                let pb = pending.bezier?;
                let end = pb.p1.unwrap_or(pb.cursor);
                if pick::distance(end, pb.p0) < 1e-6 {
                    return None;
                }
                let (c1, c2) = pb.effective(end);
                Some(bezier::end_tangent(pb.p0, c1, c2, end, true))
            }
            PenMode::Arc => {
                let pc = pending.circle?;
                let a = pc.a?;
                let b = pc.b.unwrap_or(pc.cursor);
                unit((b.x - a.x, b.y - a.y))
            }
        };
        let out = out?;
        for (sid, s) in self.doc.all_segments() {
            if !matches!(
                s.kind,
                crate::core::document::SegmentKind::Line
                    | crate::core::document::SegmentKind::Arc
                    | crate::core::document::SegmentKind::Bezier
            ) {
                continue;
            }
            // Prev-span direction AWAY from the joint.
            let prev = if s.end == aid {
                self.span_end_tangent(sid)
            } else if s.start == aid {
                self.span_start_tangent(sid)
            } else {
                continue;
            };
            if let Some(p) = prev
                && out.0 * p.0 + out.1 * p.1 < -0.996
            {
                return Some((sid, out));
            }
        }
        None
    }

    /// Free (no-shift) pen tangent: ONLY continues a tangent when the span
    /// starts ON an existing bezier (smooth chaining). Never touches arcs
    /// so the preview is never yanked onto a distant external tangent.
    fn bezier_tangent_snap(&self, start: Point2, cursor: Point2) -> Option<Point2> {
        // Bezier targets: nearest-sample tangent ray through `start`.
        let mut best: Option<(f64, Point2)> = None;
        let tol = self.snap_tol_doc();
        for (sid, seg) in self.doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Bezier {
                continue;
            }
            let (h1, h2) = seg.bezier_handles();
            let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                self.doc.point(seg.start),
                h1.and_then(|id| self.doc.point(id)),
                h2.and_then(|id| self.doc.point(id)),
                self.doc.point(seg.end),
            ) else {
                continue;
            };
            // Only snap when starting ON the curve (chaining / G1).
            let (near, d, tan) = bezier::nearest_on_curve(p0, c1, c2, p1, start, 48);
            if d > tol {
                continue;
            }
            let len = pick::distance(start, cursor).max(tol);
            for side in [-1.0, 1.0] {
                let q = Point2::new(
                    near.x + tan.0 * len * side,
                    near.y + tan.1 * len * side,
                );
                let score = pick::distance(q, cursor);
                if best.map_or(true, |(s, _)| score < s) {
                    best = Some((score, q));
                }
            }
            let _ = sid;
        }
        best.map(|(_, q)| q)
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
                    // SOLO segment -> edge-stretch / translation.
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
                let (line, curve) = match (first.kind, second.kind) {
                    (crate::core::document::SegmentKind::Line, crate::core::document::SegmentKind::Arc) => (first, second),
                    (crate::core::document::SegmentKind::Arc, crate::core::document::SegmentKind::Line) => (second, first),
                    _ => continue,
                };
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
        let solution = solver.solve();
        // A live drag may request an impossible step, but applying a partial
        // LM iterate would visibly break a locked constraint. Keep the last
        // valid geometry until a later cursor position becomes solvable.
        if !solution.constraints_satisfied() {
            self.snap_guides.clear();
            return true;
        }
        let mut moved: std::collections::HashSet<PointId> = std::collections::HashSet::new();
        for (id, pos) in solution.positions {
            moved.insert(id);
            self.doc.move_point(id, pos);
        }
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

    /// If `p` is near a selected arc's circumcenter, return a drag that
    /// moves the whole arc (all points including center).
    // -- dimensions (delegates to the dims subsystem) --

    pub fn update_dim_geom(&mut self) {
        dims::update(self);
    }

    /// Snaps a prospective line to a nearby right-angle direction. The
    /// source segment is returned for preview/commit of the relation.
    fn perpendicular_snap_for_line(&self, start: Point2, cursor: Point2) -> Option<(Point2, SegmentId)> {
        let length = pick::distance(start, cursor);
        if length <= 1e-6 { return None; }
        let cursor_angle = (cursor.y - start.y).atan2(cursor.x - start.x);
        let mut best: Option<(f64, Point2, SegmentId)> = None;
        for (sid, seg) in self.doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Line { continue; }
            let (Some(a), Some(b)) = (self.doc.point(seg.start), self.doc.point(seg.end)) else { continue };
            let angle = (b.y - a.y).atan2(b.x - a.x);
            let mut error = (cursor_angle - angle).abs();
            while error > std::f64::consts::PI { error -= std::f64::consts::TAU; }
            let right_angle_error = (error.abs() - std::f64::consts::FRAC_PI_2).abs();
            if right_angle_error > 8f64.to_radians() { continue; }
            let perpendicular = angle + std::f64::consts::FRAC_PI_2;
            let alternate = angle - std::f64::consts::FRAC_PI_2;
            let q1 = Point2::new(start.x + length * perpendicular.cos(), start.y + length * perpendicular.sin());
            let q2 = Point2::new(start.x + length * alternate.cos(), start.y + length * alternate.sin());
            let (q, score) = if pick::distance(q1, cursor) <= pick::distance(q2, cursor) {
                (q1, pick::distance(q1, cursor))
            } else { (q2, pick::distance(q2, cursor)) };
            // A perpendicular relation is only meaningful when the new
            // segment is actually joined to the existing one. Never turn a
            // merely similarly-oriented line elsewhere on the canvas into a
            // constraint candidate.
            let connected = [a, b].iter().any(|&point| {
                pick::distance(start, point) <= self.snap_tol_doc()
                    || pick::distance(q, point) <= self.snap_tol_doc()
            });
            if !connected { continue; }
            if best.as_ref().map_or(true, |(old, _, _)| score < *old) {
                best = Some((score, q, sid));
            }
        }
        best.map(|(_, point, sid)| (point, sid))
    }

    // -- hover --

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

    fn normalize_constraint_hit(&self, element: ElementRef) -> Option<ElementRef> {
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
        let is_bezier = |l: crate::core::ids::SegmentId| {
            self.doc
                .segment(l)
                .is_some_and(|s| s.kind == SegmentKind::Bezier)
        };
        // An arc's circumcenter is a REAL document point — distance dims
        // to an arc run to its center (Fusion-style), not to a chord.
        let arc_center = |l: crate::core::ids::SegmentId| -> Option<PointId> {
            self.doc.segment(l)?.center
        };
        match picks {
            // A single pick already completes: an edge measures its own
            // length; an arc measures its radius; a BEZIER measures its
            // total curve length (the Distance dim — never the chord)
            // UNLESS a width/height/displacement lock restricts it to the
            // span between its two endpoints.
            [DimPick::Line(l)] => {
                if is_arc(*l) {
                    Some(DimTarget::Radius { seg: *l })
                } else if is_bezier(*l) {
                    match self.dim_mode_lock.as_deref() {
                        Some("width") | Some("height") | Some("displacement") => {
                            let seg = self.doc.segment(*l)?;
                            Some(DimTarget::Points {
                                a: seg.start,
                                b: seg.end,
                                mode: DimMode::Aligned,
                            })
                        }
                        _ => Some(DimTarget::CurveLength { seg: *l }),
                    }
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
                // A width/height/displacement lock restricts the pair to a
                // midpoint span; otherwise angle for crossing pairs and
                // midpoint displacement for parallel ones (measured
                // between chord midpoints — no gap dimension is offered).
                match self.dim_mode_lock.as_deref() {
                    Some("width") => Some(DimTarget::EdgeMid {
                        a: *a,
                        b: *b,
                        mode: DimMode::X,
                    }),
                    Some("height") => Some(DimTarget::EdgeMid {
                        a: *a,
                        b: *b,
                        mode: DimMode::Y,
                    }),
                    Some("displacement") => Some(DimTarget::EdgeMid {
                        a: *a,
                        b: *b,
                        mode: DimMode::Aligned,
                    }),
                    _ => {
                        let (ga, gb) =
                            (self.doc.segment_geom(*a)?, self.doc.segment_geom(*b)?);
                        if Self::geoms_cross(ga, gb) {
                            Some(DimTarget::Angle { a: *a, b: *b })
                        } else {
                            Some(DimTarget::EdgeMid {
                                a: *a,
                                b: *b,
                                mode: DimMode::Aligned,
                            })
                        }
                    }
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
    /// Shared placement math for a measured position pair — point-pair
    /// dims and edge-midpoint spans alike. Cursor-zone auto mode unless
    /// `forced` carries an explicit row/lock choice. Returns
    /// (mode, offset, slide, measured).
    fn place_point_pair(
        pa: Point2,
        pb: Point2,
        cursor: Point2,
        forced: Option<crate::core::constraints::DimMode>,
    ) -> (
        crate::core::constraints::DimMode,
        f64,
        f64,
        f64,
    ) {
        use crate::core::constraints::DimMode;
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
        let is_vertical = dy.abs() > 1e-9 && dx.abs() <= dy.abs() * 0.0875;
        let is_horizontal = dx.abs() > 1e-9 && dy.abs() <= dx.abs() * 0.0875;
        let auto = if is_vertical {
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
        let mode = forced.unwrap_or(auto);
        let along = rel.0 * u.0 + rel.1 * u.1;
        let perp = rel.0 * n.0 + rel.1 * n.1;
        let (offset, slide, measured) = match mode {
            DimMode::Aligned => (perp, along.clamp(0., len), len),
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

    fn dim_placement(
        &self,
        target: crate::core::constraints::DimTarget,
        cursor: Point2,
    ) -> Option<(crate::core::constraints::DimMode, f64, f64, f64)> {
        use crate::core::constraints::{DimMode, DimTarget};
        Some(match target {
            DimTarget::Points { a, b, .. } => {
                let (pa, pb) = (self.doc.point(a)?, self.doc.point(b)?);
                // Floating-menu lock overrides the cursor-zone auto-pick.
                let forced = match self.dim_mode_lock.as_deref() {
                    Some("width") => Some(DimMode::X),
                    Some("height") => Some(DimMode::Y),
                    Some("displacement") => Some(DimMode::Aligned),
                    _ => None,
                };
                Self::place_point_pair(pa, pb, cursor, forced)
            }
            DimTarget::EdgeMid { a, b, mode } => {
                // Width/height/displacement between chord midpoints. The
                // mode rides in the target itself (set by the kind rows),
                // so placement never second-guesses it.
                let (aa, ab) = self.doc.segment_geom(a)?;
                let (ba, bb) = self.doc.segment_geom(b)?;
                let ma = Point2::new((aa.x + ab.x) / 2., (aa.y + ab.y) / 2.);
                let mb = Point2::new((ba.x + bb.x) / 2., (ba.y + bb.y) / 2.);
                Self::place_point_pair(ma, mb, cursor, Some(mode))
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
                // Container rides a center->arc ray at the cursor's radial
                // fraction. The legacy bend point is only needed to recover
                // the arc's sweep branch.
                let frac = if r > 1e-9 {
                    (pick::distance(cursor, center) / r).clamp(0.25, 1.0)
                } else {
                    1.0
                };
                (DimMode::Aligned, r, frac, r)
            }
            DimTarget::CurveLength { seg } => {
                // BEZIER ONLY. Offset = cursor's signed distance from the
                // curve, slide = continuous arclength projection (segment
                // interpolation, never vertex-quantized — the label glides
                // instead of vibrating). Measured = total length OF THE
                // CURVE.
                let Some(seg_d) = self.doc.segment(seg) else {
                    return None;
                };
                if seg_d.kind != crate::core::document::SegmentKind::Bezier {
                    return None;
                }
                let (h1, h2) = seg_d.bezier_handles();
                let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                    self.doc.point(seg_d.start),
                    h1.and_then(|id| self.doc.point(id)),
                    h2.and_then(|id| self.doc.point(id)),
                    self.doc.point(seg_d.end),
                ) else {
                    return None;
                };
                let pts: Vec<Point2> = crate::editor::bezier::samples(
                    p0,
                    c1,
                    c2,
                    p1,
                    dims::bezier_sample_count(self.camera.zoom, p0, c1, c2, p1),
                );
                let (pos, total, signed) = dims::project_polyline(&pts, cursor);
                if total < 1e-9 {
                    return None;
                }
                (DimMode::Aligned, signed, (pos / total).clamp(0., 1.), total)
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
                    angle_value_for_input(input.measured, mag)
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

    fn is_constraint_tool(&self) -> bool {
        matches!(
            self.tool,
            Tool::ConstraintHorizontalVertical
                | Tool::ConstraintTangent
                | Tool::ConstraintCoincident
                | Tool::ConstraintParallel
                | Tool::ConstraintPerpendicular
        )
    }

    fn constraint_tool_click(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
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

    fn solve_constraint_now(&mut self, elements: &[ElementRef]) -> bool {
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
    /// Shared container-drag math for a measured position pair. Returns
    /// the (offset, slide) the container follows the cursor with.
    fn drag_point_pair(
        pa: Point2,
        pb: Point2,
        mode: crate::core::constraints::DimMode,
        cur: Point2,
    ) -> (f64, f64) {
        let (u, n) = dims::dim_axes(pb.x - pa.x, pb.y - pa.y);
        let rel = (cur.x - pa.x, cur.y - pa.y);
        let len = pick::distance(pa, pb);
        let dx = pb.x - pa.x;
        let dy = pb.y - pa.y;
        match mode {
            crate::core::constraints::DimMode::Aligned => (
                rel.0 * n.0 + rel.1 * n.1,
                (rel.0 * u.0 + rel.1 * u.1).clamp(0., len),
            ),
            crate::core::constraints::DimMode::X => {
                (rel.1, (rel.0 * dx.signum()).clamp(0., dx.abs()))
            }
            crate::core::constraints::DimMode::Y => {
                (rel.0, (rel.1 * dy.signum()).clamp(0., dy.abs()))
            }
        }
    }

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
                // The stored mode is kept on drag (flipping modes of a live
                // constraint would re-solve the geometry mid-gesture); the
                // placement follows the cursor within that mode's frame.
                let (offset, slide) = Self::drag_point_pair(pa, pb, mode, cur);
                let dim = &mut self.doc.dimensions[idx];
                dim.offset = offset;
                dim.slide = slide;
            }
            DimTarget::EdgeMid { a, b, mode } => {
                let (Some((aa, ab)), Some((ba, bb))) =
                    (self.doc.segment_geom(a), self.doc.segment_geom(b))
                else {
                    return;
                };
                let ma = Point2::new((aa.x + ab.x) / 2., (aa.y + ab.y) / 2.);
                let mb = Point2::new((ba.x + bb.x) / 2., (ba.y + bb.y) / 2.);
                let (offset, slide) = Self::drag_point_pair(ma, mb, mode, cur);
                let dim = &mut self.doc.dimensions[idx];
                dim.offset = offset;
                dim.slide = slide;
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
                // Container slides along a center->arc ray. The legacy bend
                // point only recovers the arc's sweep branch.
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
            DimTarget::CurveLength { seg } => {
                // Bezier-only: offset + slide follow the cursor through a
                // continuous segment projection (no vertex stepping).
                let Some(seg_d) = self.doc.segment(seg) else {
                    return;
                };
                if seg_d.kind != crate::core::document::SegmentKind::Bezier {
                    return;
                }
                let (h1, h2) = seg_d.bezier_handles();
                let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                    self.doc.point(seg_d.start),
                    h1.and_then(|id| self.doc.point(id)),
                    h2.and_then(|id| self.doc.point(id)),
                    self.doc.point(seg_d.end),
                ) else {
                    return;
                };
                let pts: Vec<Point2> = crate::editor::bezier::samples(
                    p0,
                    c1,
                    c2,
                    p1,
                    dims::bezier_sample_count(self.camera.zoom, p0, c1, c2, p1),
                );
                let (pos, total, signed) = dims::project_polyline(&pts, cur);
                if total < 1e-9 {
                    return;
                }
                self.doc.dimensions[idx].offset = signed;
                self.doc.dimensions[idx].slide = (pos / total).clamp(0., 1.);
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
                // At an arc endpoint the new line has a separate point id;
                // merge it into the arc endpoint so later radius edits carry
                // the tangent contact with no duplicate Coincident row.
                let endpoint = [arc.start, arc.end].into_iter()
                    .min_by(|x, y| {
                        let anchor = self.doc.point(point).unwrap_or(contact);
                        let px = self.doc.point(*x).unwrap_or(anchor);
                        let py = self.doc.point(*y).unwrap_or(anchor);
                        pick::distance(px, anchor).partial_cmp(&pick::distance(py, anchor)).unwrap_or(std::cmp::Ordering::Equal)
                    });
                let contact_id = if let Some(endpoint) = endpoint
                    && self.doc.point(point).zip(self.doc.point(endpoint)).is_some_and(|(p, e)| pick::distance(p, e) <= self.snap_tol_doc() * 1.5)
                    && endpoint != point
                {
                    self.doc.merge_point(endpoint, point);
                    endpoint
                } else { point };
                self.doc.add_tangent_constraint(line, arc_id, contact_id);
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
                        _DT::PointLine { line, .. }
                        | _DT::Radius { seg: line }
                        | _DT::CurveLength { seg: line } => *line != s,
                        _DT::Lines { a, b }
                        | _DT::Angle { a, b }
                        | _DT::EdgeMid { a, b, .. } => *a != s && *b != s,
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
                        v
                    })
                    .unwrap_or_default();
                self.doc.remove_segment(s);
                for pid in ends {
                    let still_used = self.doc.all_segments().any(|(_, seg)| seg.start == pid || seg.end == pid)
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
        }
        self.selection.retain(|&e| e != el);
    }

    /// Drag ended with points dropped onto points: instead of the old
    /// bond-choice popup, select the overlapping points so the floating
    /// menu offers Coincident + Merge right where the choice used to be.
    /// Returns true when overlapping points were found (and selected).
    fn queue_bond_menu(&mut self) -> bool {
        let tol = self.snap_tol_doc();
        let Some(drag) = &self.dragging else { return false };
        let dragged: Vec<PointId> =
            drag.points.iter().chain(drag.aux.iter()).map(|&(id, _)| id).collect();
        let mut found: Vec<PointId> = Vec::new();
        for &pid in &dragged {
            let Some(p) = self.doc.point(pid) else { continue };
            for (qid, q) in self.doc.all_points() {
                if qid == pid || dragged.contains(&qid) {
                    continue;
                }
                if pick::distance(p, q) > tol {
                    continue;
                }
                // Skip pairs already glued in either order.
                if self.doc.constraints.iter().any(|c| {
                    c.kind == ConstraintKind::Coincident
                        && ((c.a == pid && c.b == qid) || (c.a == qid && c.b == pid))
                }) {
                    continue;
                }
                if !found.contains(&pid) {
                    found.push(pid);
                }
                if !found.contains(&qid) {
                    found.push(qid);
                }
            }
        }
        if found.is_empty() {
            return false;
        }
        self.selection = found.into_iter().map(ElementRef::Point).collect();
        true
    }

    /// Merges the selected bare points into the first one (floating menu
    /// "Merge points" row — the old bond menu's combine action).
    /// Merges only close, unglued point pairs (same pairs the menu gates
    /// on) — never a mass collapse of the whole selection into one point.
    pub fn merge_selected_points(&mut self) -> bool {
        let pairs = Self::merge_candidate_pairs(
            &self.doc,
            &self.selection,
            self.snap_tol_doc(),
        );
        if pairs.is_empty() {
            return false;
        }
        self.history_begin();
        let mut merged_any = false;
        for (a, b) in pairs {
            if a == b {
                continue;
            }
            // Either side may have vanished into an earlier pair's merge.
            if self.doc.point(a).is_none() || self.doc.point(b).is_none() {
                continue;
            }
            self.doc.merge_point(a, b);
            merged_any = true;
        }
        if !merged_any {
            self.gesture_snapshot = None;
            return false;
        }
        // Drop ids that no longer exist; keeps survive in place.
        self.selection.retain(|el| match *el {
            ElementRef::Point(p) => self.doc.point(p).is_some(),
            ElementRef::Segment(s) => self.doc.segment(s).is_some(),
            ElementRef::Fill(f) => self.doc.fill(f).is_some(),
        });
        self.flush_pending_history();
        true
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
        self.hovered_constraint = None;
        self.snap_guides.clear();
        self.pending_shape = None;
        self.pending_ruler = None;
        self.pending_line = None;
        self.perpendicular_preview = None;
        self.pending_circle = None;
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

/// Converts an angle entry while preserving the sector selected during
/// placement. A reflex preview is stored as (for example) -270 degrees;
/// reducing that to -90 degrees changes the constraint branch instead of
/// expressing the same 90-degree corner on the selected side.
fn angle_value_for_input(measured: f64, magnitude: f64) -> f64 {
    let magnitude = magnitude.abs().clamp(0., 360.);
    let magnitude = if measured.abs() > 180. {
        360. - magnitude
    } else {
        magnitude
    };
    measured.signum() * magnitude
}

fn equivalent_angle_vertex(doc: &Document, a: PointId, b: PointId) -> bool {
    a == b
        || doc.constraints.iter().any(|constraint| {
            constraint.kind == ConstraintKind::Coincident
                && ((constraint.a == a && constraint.b == b)
                    || (constraint.a == b && constraint.b == a))
        })
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
mod angle_tests {
    use super::*;

    #[test]
    fn angle_input_keeps_reflex_sector() {
        assert_eq!(angle_value_for_input(-270., 90.), -270.);
        assert_eq!(angle_value_for_input(-240., 120.), -240.);
        assert_eq!(angle_value_for_input(90., 90.), 90.);
    }

    #[test]
    fn coincident_endpoints_are_angle_pivots() {
        let mut doc = Document::new();
        let a = doc.add_point(Point2::new(0., 0.));
        let b = doc.add_point(Point2::new(0., 0.));
        doc.add_constraint(ConstraintKind::Coincident, a, b);
        assert!(equivalent_angle_vertex(&doc, a, b));
    }

    fn bezier_test_editor() -> (Editor, crate::core::ids::SegmentId) {
        let mut doc = Document::new();
        let p0 = doc.add_point(Point2::new(0., 0.));
        let c1 = doc.add_point(Point2::new(100., 0.));
        let c2 = doc.add_point(Point2::new(100., 100.));
        let p1 = doc.add_point(Point2::new(200., 100.));
        let seg = doc.add_bezier_segment(p0, c1, c2, p1);
        let mut ed = Editor::from_document(doc);
        ed.tool = Tool::Dimension;
        (ed, seg)
    }

    #[test]
    fn bezier_single_pick_arms_curve_length() {
        let (mut ed, seg) = bezier_test_editor();
        ed.dim_picks.push(DimPick::Line(seg));
        let t = ed.resolve_dim_target(&ed.dim_picks);
        assert!(
            matches!(
                t,
                Some(crate::core::constraints::DimTarget::CurveLength { .. })
            ),
            "single bezier pick must arm CurveLength, got {t:?}"
        );
        let placed = ed.dim_placement(t.unwrap(), Point2::new(100., 250.));
        assert!(placed.is_some(), "CurveLength placement returned None");
        let (_, _, _, measured) = placed.unwrap();
        assert!(measured > 200., "measured curve length implausible: {measured}");
    }

    #[test]
    fn bezier_curve_length_commits_and_renders() {
        let (mut ed, seg) = bezier_test_editor();
        ed.dim_picks.push(DimPick::Line(seg));
        let target = ed.resolve_dim_target(&ed.dim_picks).unwrap();
        let (_, offset, slide, measured) = ed
            .dim_placement(target, Point2::new(100., 250.))
            .expect("placement");
        let dim = crate::core::constraints::Dimension {
            target,
            value: measured,
            offset,
            slide,
            sweep: 0.,
        };
        assert!(ed.try_apply_dimension(dim), "CurveLength commit failed");
        assert_eq!(ed.doc.dimensions.len(), 1);
        ed.update_dim_geom();
        assert!(
            !ed.curve_dim_renders.is_empty(),
            "no curve replica render data after commit"
        );
    }
}
