pub mod arc;
pub mod bezier;
mod clipboard;
mod camera;
pub mod dims;
pub(crate) mod constraints_apply;
pub(crate) mod drag;
pub(crate) mod events;
pub(crate) mod pen;
pub mod fillet;
pub(crate) mod gates;
pub mod grid;
pub mod joints;
mod clay;
pub mod overconstraint;
pub mod pick;
pub mod ruler;
pub mod spans;
mod snapping;
mod tools;

use std::cell::RefCell;

pub use camera::Camera;

pub use snapping::SnapGuide;
pub use tools::{ClayPath, DimInput, DimPick, EditScope, InteractionMode, PenMode, PendingBezier, PendingCircle, PendingLine, PendingPen, PendingRuler, PendingShape, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorField { X, Y, Width, Height, Opacity, StrokeHex, FillHex, Dimension(usize), LayerName(u64) }

#[derive(Clone, Debug)]
pub struct InspectorInput { pub field: InspectorField, pub value: String }

use crate::core::constraints::{ConstraintKind, DimTarget, ElementRef};
use crate::core::document::{Document, Layer, LayerKind, StrokeDash};
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
//   gates    - constraint-gating candidates + feasibility (pure)
//   drag     - live drag system (solve_drag, arc kinematics, DragState)
//   constraints_apply - menu/tool creation, trial solves, exact passes
//   pen - chained line/arc/bezier path tool
//   events - canvas down/drag/up/hover + chip hit-testing

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

/// A non-blocking UI notice produced by the editor (over-constraint
/// rejections today, anything tomorrow). Shell drains `toast_requests`
/// into the reusable toast stack — same component, same animations.
#[derive(Clone, Debug)]
pub struct ToastRequest {
    /// Short bold headline, e.g. "Can't add Horizontal".
    pub title: String,
    /// One short summary sentence (the "why").
    pub body: String,
    /// One short fix sentence (the "what to do"), rendered muted below
    /// the blocker chips.
    pub hint: String,
    /// The exact existing locks blocking the attempt, rendered as icon
    /// chips so they scan at a glance instead of hiding in a sentence.
    pub blocks: Vec<BlockChip>,
    /// True = error styling + longer dwell; false = plain info.
    pub error: bool,
}

/// Which lock a blocker chip names (drives the chip icon in the toast
/// card; the editor itself stays free of UI assets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Horizontal,
    Vertical,
    Coincident,
    Tangent,
    Parallel,
    Perpendicular,
    Dimension,
}

/// One blocking lock: its kind (icon) plus a compact label.
#[derive(Clone, Debug)]
pub struct BlockChip {
    pub kind: LockKind,
    pub label: String,
}

impl ToastRequest {
    pub fn error(
        title: impl Into<String>,
        body: impl Into<String>,
        hint: impl Into<String>,
        blocks: Vec<BlockChip>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            hint: hint.into(),
            blocks,
            error: true,
        }
    }

    pub fn info(
        title: impl Into<String>,
        body: impl Into<String>,
        hint: impl Into<String>,
        blocks: Vec<BlockChip>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            hint: hint.into(),
            blocks,
            error: false,
        }
    }
}

// An active drag: see `drag.rs` (`DragState`). The live drag system
// (`solve_drag`, kinematic arc plans, arc consistency) lives there too.
pub(crate) use drag::DragState;

pub struct Editor {
    pub doc: Document,
    // Persistent tessellation cache used by the canvas paint pass. Interior
    // mutability keeps rendering read-only with respect to editor state while
    // allowing unchanged geometry to skip resampling.
    pub(crate) render_cache: RefCell<crate::ui::canvas::paint::RenderCache>,
    pub camera: Camera,
    pub tool: Tool,
    pub interaction_mode: InteractionMode,
    // Clay joint sidecar (Phase 1): grade/fullness/angles per point.
    // Presence = managed (derived handles); absence = legacy.
    pub joint_data: std::collections::HashMap<crate::core::ids::PointId, joints::JointData>,
    // Clay active path (Phase 2.9). Persists across tool switches so a
    // path can be resumed; ended explicitly (Enter/Esc/close).
    pub clay_path: Option<ClayPath>,
    // Edit isolation scope (None = everything editable).
    pub edit_scope: Option<EditScope>,
    // Last Clay action readout for the modebar redo region:
    // (what happened, key hints). Narration teaches the grammar.
    pub clay_status: Option<(String, String)>,
    // Clay view flags: combs default ON in Edit (spec §8).
    pub show_combs: bool,
    pub comb_scale: f32,
    pub comb_density: usize,
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
    pub inspector_input: Option<InspectorInput>,
    pub color_picker_open: bool,
    // Elements picked by an active constraint tool before its constraint is
    // committed.
    pub constraint_picks: Vec<ElementRef>,
    pub constraint_point_picks: Vec<PointId>,
    pub fillet_picks: Vec<SegmentId>,
    pub fillet_preview: Option<fillet::FilletPreview>,
    // Live fillet radius-drag gesture (grabbed center, radius free).
    pub fillet_radius_drag: Option<fillet::FilletRadiusDrag>,
    // Live fillet corner-drag gesture (grabbed center with a committed
    // radius dimension: resizes from the adjacent edges instead).
    pub fillet_corner_drag: Option<fillet::FilletCornerDrag>,
    // Staged fillet handle press, pre-click-threshold (converts to a
    // gesture on real movement; clean release emulates the click).
    pub fillet_press: Option<fillet::FilletPress>,
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
    // New geometry lands here (panel selection). Falls back to the
    // first layer when the id goes stale (deleted/loaded docs).
    pub active_layer: u64,
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
    // Non-blocking notices for the Shell toast stack (replaces the old
    // over-constrained modal). The editor pushes structured requests;
    // Shell drains them into `Shell::push_toast` each frame so the same
    // component serves over-constraints and every future notice.
    pub toast_requests: Vec<ToastRequest>,
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
            visible: true,
            parent: None,
            kind: Some(LayerKind::Body),
        });
        Self::from_document(doc)
    }

    pub fn from_document(doc: Document) -> Self {
        let settings = doc.settings;
        let next_layer_id = doc.layers.iter().map(|l| l.id + 1).max().unwrap_or(1);
        let active_layer = doc.layers.first().map(|l| l.id).unwrap_or(1);
        Self {
            doc,
            render_cache: RefCell::new(Default::default()),
            camera: Camera::new(),
            tool: Tool::Move,
            interaction_mode: InteractionMode::Object,
            joint_data: std::collections::HashMap::new(),
            clay_path: None,
            edit_scope: None,
            clay_status: None,
            // Combs disabled for now (kept as code; Shift+C re-enables).
            show_combs: false,
            comb_scale: 400.0,
            comb_density: 12,
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
            inspector_input: None,
            color_picker_open: false,
            constraint_picks: Vec::new(),
            constraint_point_picks: Vec::new(),
            fillet_picks: Vec::new(),
            fillet_preview: None,
            fillet_radius_drag: None,
            fillet_corner_drag: None,
            fillet_press: None,
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
            active_layer,
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
            toast_requests: Vec::new(),
            arc_center_reveal: Vec::new(),
        }
    }

    /// Flip Object/Edit. Entering Edit strips managed-handle point refs
    /// (they don't exist there) and FREEZES the isolation scope (islands
    /// of the selection; empty selection = everything). Entering Object
    /// clears the scope and expands the selection to whole islands
    /// (components → wholes carry rule).
    pub fn set_interaction_mode(&mut self, mode: InteractionMode) -> bool {
        if self.interaction_mode == mode {
            return false;
        }
        self.interaction_mode = mode;
        if mode == InteractionMode::Edit {
            let dead = self.managed_handles();
            self.selection.retain(|el| match el {
                ElementRef::Point(pid) => !dead.contains(pid),
                _ => true,
            });
            self.edit_scope = self.freeze_scope();
            // Out-of-scope annotations die with the transition (hover
            // recomputes on next move; an open value input is left alone).
            if let Some(sel) = self.selected_dim {
                if !self.dim_in_scope(sel) {
                    self.selected_dim = None;
                }
            }
            self.hovered_dim = self.hovered_dim.filter(|&i| self.dim_in_scope(i));
            let sel = std::mem::take(&mut self.selected_constraints);
            self.selected_constraints = sel
                .into_iter()
                .filter(|c| self.marker_in_scope(c))
                .collect();
        } else {
            self.edit_scope = None;
            // A fillet press staged in Edit must not convert after the
            // flip (staging is Edit-gated; the flip drops it).
            self.fillet_press = None;
            // Expand AFTER the flip so the Object gate inside passes.
            let sel = std::mem::take(&mut self.selection);
            self.selection = self.expand_to_islands(&sel);
        }
        true
    }

    /// Snapshot the isolation scope from the current selection. None =
    /// everything (empty selection, or selection yielding nothing scoped).
    fn freeze_scope(&self) -> Option<EditScope> {
        if self.selection.is_empty() {
            return None;
        }
        let islands = self.doc.islands();
        let mut scope = EditScope::default();
        for el in &self.selection {
            match *el {
                ElementRef::Segment(sid) => {
                    if let Some(isl) = islands.iter().find(|v| v.contains(&sid)) {
                        for s in isl {
                            if !scope.segments.contains(s) {
                                scope.segments.push(*s);
                            }
                        }
                    }
                }
                ElementRef::Point(pid) => {
                    let mut touched = false;
                    for isl in &islands {
                        let hits = isl.iter().any(|s| {
                            self.doc.segment(*s).is_some_and(|g| {
                                g.start == pid || g.end == pid
                            })
                        });
                        if hits {
                            touched = true;
                            for s in isl {
                                if !scope.segments.contains(s) {
                                    scope.segments.push(*s);
                                }
                            }
                        }
                    }
                    if !touched && !scope.points.contains(&pid) {
                        scope.points.push(pid);
                    }
                }
                ElementRef::Fill(fid) => {
                    if let Some(f) = self.doc.fill(fid) {
                        for sid in &f.segments.clone() {
                            if let Some(isl) = islands.iter().find(|v| v.contains(sid)) {
                                for s in isl {
                                    if !scope.segments.contains(s) {
                                        scope.segments.push(*s);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if scope.segments.is_empty() && scope.points.is_empty() {
            None
        } else {
            Some(scope)
        }
    }

    pub fn in_scope_segment(&self, sid: crate::core::ids::SegmentId) -> bool {
        self.edit_scope
            .as_ref()
            .map_or(true, |s| s.segments.contains(&sid))
    }

    pub fn in_scope_point(&self, pid: crate::core::ids::PointId) -> bool {
        match &self.edit_scope {
            None => true,
            Some(s) => {
                s.points.contains(&pid)
                    || s.segments.iter().any(|id| {
                        self.doc.segment(*id).is_some_and(|g| {
                            g.start == pid || g.end == pid
                        })
                    })
            }
        }
    }

    pub fn in_scope_element(&self, el: ElementRef) -> bool {
        match el {
            ElementRef::Point(pid) => self.in_scope_point(pid),
            ElementRef::Segment(sid) => self.in_scope_segment(sid),
            ElementRef::Fill(fid) => self.doc.fill(fid).is_some_and(|f| {
                f.segments.iter().all(|sid| self.in_scope_segment(*sid))
            }),
        }
    }

    /// In-scope point set for hit-testing and dim/chip life (None =
    /// everything — no isolation).
    pub fn scoped_points(&self) -> Option<std::collections::HashSet<crate::core::ids::PointId>> {
        crate::editor::joints::scope_point_set(&self.doc, self.edit_scope.as_ref())
    }

    /// A dimension lives iff every referenced point is in scope.
    pub fn dim_in_scope(&self, idx: usize) -> bool {
        match self.doc.dimensions.get(idx) {
            Some(d) => match self.scoped_points() {
                None => true,
                Some(set) => crate::editor::joints::dim_target_points(&self.doc, &d.target)
                    .iter()
                    .all(|p| set.contains(p)),
            },
            None => false,
        }
    }

    /// A constraint chip lives iff every touched point is in scope.
    pub fn marker_in_scope(&self, c: &crate::core::constraints::Constraint) -> bool {
        match self.scoped_points() {
            None => true,
            Some(set) => crate::editor::joints::constraint_points(&self.doc, c)
                .iter()
                .all(|p| set.contains(p)),
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
        self.fillet_picks.clear();
        self.fillet_preview = None;
        self.fillet_radius_drag = None;
        self.fillet_corner_drag = None;
        self.fillet_press = None;
        self.selected_constraints.clear();
        self.marquee = None;
        self.group_drag_last = None;
        self.dragging = None;
        // Dimension tool picks + value input reset on every tool switch.
        // Toasts intentionally persist across switches (non-modal).
        self.dim_picks.clear();
        self.dim_target = None;
        self.dim_input = None;
        true
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
    /// Implementation lives in `gates.rs`.
    fn geoms_cross(
        ga: (crate::core::geometry::Point2, crate::core::geometry::Point2),
        gb: (crate::core::geometry::Point2, crate::core::geometry::Point2),
    ) -> bool {
        gates::geoms_cross(ga, gb)
    }

    /// Whether two segments' directions cross (false for parallel pairs
    /// and missing geometry).
    pub(crate) fn lines_cross(
        &self,
        a: crate::core::ids::SegmentId,
        b: crate::core::ids::SegmentId,
    ) -> bool {
        gates::lines_cross(&self.doc, a, b)
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
    // Implementations live in `gates.rs`; the associated functions below
    // are thin delegates so existing `Editor::gate_*` call sites
    // (floating menu, overconstraint, apply arms) keep working unchanged.

    /// Valid bare-point selections (live positions only).
    pub(crate) fn gate_points(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Vec<PointId> {
        gates::gate_points(doc, selection)
    }

    /// Valid explicitly-selected segments.
    pub(crate) fn gate_segs(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Vec<SegmentId> {
        gates::gate_segs(doc, selection)
    }

    /// First segment owning pid as an endpoint (arena order).
    pub(crate) fn owner_segment(doc: &Document, pid: PointId) -> Option<SegmentId> {
        gates::owner_segment(doc, pid)
    }

    /// True when one segment owns both points as its endpoints (gluing
    /// them would collapse that edge — never offered).
    pub(crate) fn same_segment_owner(doc: &Document, a: PointId, b: PointId) -> bool {
        gates::same_segment_owner(doc, a, b)
    }

    /// Coordinates forced onto pid by the H/V/coincident network, if any
    /// (first found per axis; point-on-edge relations don't fix a coord).
    /// Used to reject pairs whose existing constraints already force
    /// different positions on the constrained axis.
    pub(crate) fn required_coords(doc: &Document, pid: PointId) -> (Option<f64>, Option<f64>) {
        gates::required_coords(doc, pid)
    }

    /// Locked direction of a line whose endpoints carry a matching H/V
    /// constraint, if any.
    pub(crate) fn locked_dir(doc: &Document, sid: SegmentId) -> Option<(f64, f64)> {
        gates::locked_dir(doc, sid)
    }

    /// Curve tangent direction at the document location nearest `near`.
    /// Orientation-agnostic (callers compare with abs dot).
    pub(crate) fn curve_tangent_at(
        doc: &Document,
        sid: SegmentId,
        near: Point2,
    ) -> Option<(f64, f64)> {
        gates::curve_tangent_at(doc, sid, near)
    }

    /// HV candidate: (a, b, segment-if-from-line). Explicit line segments
    /// win; else two bare points that don't share one segment and whose
    /// inferred axis isn't already forced apart.
    pub(crate) fn hv_candidates(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(PointId, PointId, Option<SegmentId>)> {
        gates::hv_candidates(doc, selection)
    }

    /// Whether gluing a and b is compatible with forced coordinates.
    fn coincident_feasible(doc: &Document, a: PointId, b: PointId) -> bool {
        gates::coincident_feasible(doc, a, b)
    }

    /// Coincident candidate: explicit point pairs (never same-segment),
    /// else a lone point to its nearest selected-segment endpoint (never
    /// itself), else the nearest cross-segment endpoint pair. All pairs
    /// feasibility-checked.
    pub(crate) fn coincident_candidates(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(PointId, PointId)> {
        gates::coincident_candidates(doc, selection)
    }

    /// Merge pairs: close (within tol), unglued bare-point pairs — the old
    /// bond-popup condition. Only these may merge; mass selections never
    /// collapse to one point.
    pub(crate) fn merge_candidate_pairs(
        doc: &Document,
        selection: &[ElementRef],
        tol: f64,
    ) -> Vec<(PointId, PointId)> {
        gates::merge_candidate_pairs(doc, selection, tol)
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
        gates::tangent_candidate(doc, selection)
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
        gates::tangent_feasible(doc, line, other, contact)
    }

    /// Two distinct explicitly-selected lines, if present.
    pub(crate) fn line_pair(
        doc: &Document,
        selection: &[ElementRef],
    ) -> Option<(SegmentId, SegmentId)> {
        gates::line_pair(doc, selection)
    }

    /// Parallel/perpendicular feasibility against H/V locks. A free side
    /// always passes; two locked sides must already satisfy the relation.
    pub(crate) fn line_pair_feasible(
        doc: &Document,
        a: SegmentId,
        b: SegmentId,
        want_parallel: bool,
    ) -> bool {
        gates::line_pair_feasible(doc, a, b, want_parallel)
    }

    /// Applies a constraint from the floating menu based on selection.
    /// Returns true when the document changed.
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

    pub fn begin_inspector_input(&mut self, field: InspectorField, value: impl Into<String>, cx: &mut gpui::Context<Self>) {
        self.inspector_input = Some(InspectorInput { field, value: value.into() });
        cx.notify();
    }

    pub fn toggle_color_picker(&mut self, cx: &mut gpui::Context<Self>) {
        self.color_picker_open = !self.color_picker_open;
        cx.notify();
    }

    pub fn inspector_input_key(&mut self, key: &str, cx: &mut gpui::Context<Self>) -> bool {
        let Some(input) = self.inspector_input.as_mut() else { return false; };
        match key {
            "backspace" => { input.value.pop(); cx.notify(); true }
            "escape" => { self.inspector_input = None; cx.notify(); true }
            "enter" => { let input = self.inspector_input.take().unwrap(); self.commit_inspector_input(input.field, &input.value, cx); true }
            k if k.len() == 1 => { input.value.push_str(k); cx.notify(); true }
            _ => false,
        }
    }

    fn commit_inspector_input(&mut self, field: InspectorField, raw: &str, cx: &mut gpui::Context<Self>) {
        match field {
            InspectorField::StrokeHex | InspectorField::FillHex => {
                let Ok(color) = u32::from_str_radix(raw.trim().trim_start_matches('#'), 16) else { return; };
                self.history_begin();
                if field == InspectorField::StrokeHex { for id in self.selection.iter().filter_map(|e| e.as_segment()) { if let Some(s)=self.doc.segment_mut(id) { s.stroke_color=color & 0x00ff_ffff; } } }
                else { for id in self.selection.iter().filter_map(|e| e.as_fill()) { if let Some(f)=self.doc.fill_mut(id) { f.fill_color=color & 0x00ff_ffff; } } }
                self.doc_gen += 1; self.flush_pending_history(); cx.notify();
            }
            InspectorField::Opacity => if let Ok(v)=raw.parse::<f32>() { self.history_begin(); for id in self.selection.iter().filter_map(|e|e.as_segment()) { if let Some(s)=self.doc.segment_mut(id) { s.opacity=(v/100.).clamp(0.,1.); } } self.doc_gen+=1; self.flush_pending_history(); cx.notify(); },
            InspectorField::Dimension(i) => if let Ok(v)=raw.parse::<f64>() { if i < self.doc.dimensions.len() { self.history_begin(); if let Some(d)=self.doc.dimensions.get_mut(i) { d.value=v; } self.doc_gen+=1; self.flush_pending_history(); cx.notify(); } },
            InspectorField::X | InspectorField::Y => if let Ok(v)=raw.parse::<f64>() { let pts=self.doc.selection_points(&self.selection); if let Some(b)=self.doc.bounds_of_points(pts.iter()) { let delta=if field==InspectorField::X { Point2::new(v-b.origin.x,0.) } else { Point2::new(0.,v-b.origin.y) }; self.history_begin(); self.doc.move_points(&pts,delta); self.doc_gen+=1; self.flush_pending_history(); cx.notify(); } },
            InspectorField::Width => if let Ok(v)=raw.parse::<f64>() { self.inspector_scale_selection(Some(v),None,cx); },
            InspectorField::Height => if let Ok(v)=raw.parse::<f64>() { self.inspector_scale_selection(None,Some(v),cx); },
            InspectorField::LayerName(id) => { let name = raw.trim(); if !name.is_empty() && self.doc.layer(id).is_some() { self.history_begin(); self.doc.rename_layer(id, name); self.doc_gen+=1; self.flush_pending_history(); } cx.notify(); },
        }
    }

    /// Small, inspector-friendly mutations. They deliberately operate on
    /// the current selection so every control remains useful for points,
    /// segments, and fills without introducing a second selection model.
    pub fn inspector_nudge(&mut self, dx: f64, dy: f64, cx: &mut gpui::Context<Self>) {
        let points = self.doc.selection_points(&self.selection);
        if points.is_empty() { return; }
        self.history_begin();
        self.doc.move_points(&points, Point2::new(dx, dy));
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_scale_selection(&mut self, width: Option<f64>, height: Option<f64>, cx: &mut gpui::Context<Self>) {
        let points = self.doc.selection_points(&self.selection);
        let Some(bounds) = self.doc.bounds_of_points(points.iter()) else { return; };
        let sx = width.map(|v| if bounds.size.w.abs() < f64::EPSILON { 1. } else { v / bounds.size.w }).unwrap_or(1.);
        let sy = height.map(|v| if bounds.size.h.abs() < f64::EPSILON { 1. } else { v / bounds.size.h }).unwrap_or(1.);
        self.history_begin();
        for id in points { if let Some(p) = self.doc.point(id) { self.doc.move_point(id, Point2::new(bounds.origin.x + (p.x - bounds.origin.x) * sx, bounds.origin.y + (p.y - bounds.origin.y) * sy)); } }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_remove_constraint(&mut self, constraint: crate::core::constraints::Constraint, cx: &mut gpui::Context<Self>) {
        self.history_begin();
        self.doc.constraints.retain(|c| *c != constraint);
        self.selected_constraints.retain(|c| *c != constraint);
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_remove_dimension(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        if index >= self.doc.dimensions.len() { return; }
        self.history_begin();
        self.doc.dimensions.remove(index);
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_remove_selected_fills(&mut self, cx: &mut gpui::Context<Self>) {
        let fills: Vec<_> = self.selection.iter().filter_map(|e| e.as_fill()).collect();
        if fills.is_empty() { return; }
        self.history_begin();
        for id in fills { self.doc.remove_fill(id); }
        self.selection.retain(|e| !matches!(e, ElementRef::Fill(_)));
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_flip(&mut self, horizontal: bool, cx: &mut gpui::Context<Self>) {
        let points = self.doc.selection_points(&self.selection);
        let Some(bounds) = self.doc.bounds_of_points(points.iter()) else { return; };
        let center = if horizontal { bounds.origin.x + bounds.size.w / 2. } else { bounds.origin.y + bounds.size.h / 2. };
        self.history_begin();
        for id in points {
            if let Some(p) = self.doc.point(id) {
                let to = if horizontal { Point2::new(2. * center - p.x, p.y) } else { Point2::new(p.x, 2. * center - p.y) };
                self.doc.move_point(id, to);
            }
        }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_stroke_width(&mut self, delta: f64, cx: &mut gpui::Context<Self>) {
        let ids: Vec<_> = self.selection.iter().filter_map(|e| e.as_segment()).collect();
        if ids.is_empty() { return; }
        self.history_begin();
        for id in ids { if let Some(s) = self.doc.segment_mut(id) { s.stroke_width = (s.stroke_width + delta).max(0.25); } }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_cycle_stroke(&mut self, cx: &mut gpui::Context<Self>) {
        let ids: Vec<_> = self.selection.iter().filter_map(|e| e.as_segment()).collect();
        if ids.is_empty() { return; }
        self.history_begin();
        for id in ids { if let Some(s) = self.doc.segment_mut(id) { s.dash = match s.dash { StrokeDash::Solid => StrokeDash::Dashed, StrokeDash::Dashed => StrokeDash::Solid }; } }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_cycle_color(&mut self, cx: &mut gpui::Context<Self>) {
        let ids: Vec<_> = self.selection.iter().filter_map(|e| e.as_segment()).collect();
        if ids.is_empty() { return; }
        self.history_begin();
        for id in ids { if let Some(s) = self.doc.segment_mut(id) { s.stroke_color = match s.stroke_color { 0x202124 => 0x2563eb, 0x2563eb => 0xdc2626, _ => 0x202124 }; } }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn inspector_set_color(&mut self, color: u32, cx: &mut gpui::Context<Self>) {
        self.history_begin();
        for id in self.selection.iter().filter_map(|e| e.as_segment()) { if let Some(s)=self.doc.segment_mut(id) { s.stroke_color=color & 0x00ff_ffff; } }
        for id in self.selection.iter().filter_map(|e| e.as_fill()) { if let Some(f)=self.doc.fill_mut(id) { f.fill_color=color & 0x00ff_ffff; } }
        self.doc_gen += 1; self.flush_pending_history(); cx.notify();
    }

    pub fn inspector_cycle_opacity(&mut self, cx: &mut gpui::Context<Self>) {
        let ids: Vec<_> = self.selection.iter().filter_map(|e| e.as_segment()).collect();
        if ids.is_empty() { return; }
        self.history_begin();
        for id in ids { if let Some(s) = self.doc.segment_mut(id) { s.opacity = if s.opacity > 0.75 { 0.5 } else if s.opacity > 0.25 { 1.0 } else { 0.5 }; } }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
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
                        if let DimTarget::Radius { seg } = input.target
                            && let Some((applied, dim_idx)) = self.update_fillet_radius(seg, value)
                        {
                            // Write only the update-resolved row: the input's
                            // index may be stale if its dimension was deleted
                            // mid-edit, and must never corrupt another row.
                            if let Some(i) = dim_idx {
                                if let Some(dim) = self.doc.dimensions.get_mut(i) {
                                    dim.value = applied;
                                }
                            }
                            self.doc_gen += 1;
                            return true;
                        }
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
                // The rejection toast was already queued by
                // `solve_and_apply`; keep the input alive so the user can
                // correct the number or cancel with Esc.
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
    /// satisfy the new equation. Infeasible placements queue a toast
    /// explaining the exact conflict and leave the document untouched.
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
            .find(|(idx, [rx, ry, rw, rh])| {
                x >= *rx && x <= *rx + *rw && y >= *ry && y <= *ry + *rh && self.dim_in_scope(*idx)
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
                // Fillet arcs use the dedicated leader layout (slide rides
                // the arc span, offset is the free leader length).
                if self.update_fillet_dim_placement(seg, cur) {
                    return;
                }
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
    // Wheel-forward reads positive delta: forward zooms IN.
    pub fn zoom_at(&mut self, cursor: gpui::Point<gpui::Pixels>, delta: f32) {
        let cursor_doc = |cam: &Camera, c: gpui::Point<gpui::Pixels>| {
            cam.screen_to_unit(Point2::new(f64::from(c.x), f64::from(c.y)))
        };
        let before = cursor_doc(&self.camera, cursor);
        let factor = f64::from((delta / 400.).exp());
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

    /// Bounding rect of the current selection, if any. Hidden elements
    /// never anchor boxes, buttons, or drag math.
    pub fn selection_bounds(&self) -> Option<Rect> {
        if self.selection.is_empty() {
            return None;
        }
        let shown: Vec<ElementRef> =
            self.selection.iter().copied().filter(|&s| self.doc.element_visible(s)).collect();
        self.doc.elements_bounds(&shown)
    }

    /// Object-world box: the selection grown to whole islands, arcs by
    /// curve extent. The box the canvas draws AND the Edit-button anchor —
    /// one source so they can never disagree.
    pub fn object_box(&self) -> Option<Rect> {
        if self.selection.is_empty() {
            return None;
        }
        let shown: Vec<ElementRef> =
            self.selection.iter().copied().filter(|&s| self.doc.element_visible(s)).collect();
        self.doc.elements_bounds(&self.doc.island_elements(&shown))
    }

    pub fn zoom_to_fit(&mut self) -> bool {
        let mut acc: Option<Rect> = None;
        for layer in &self.doc.layers {
            if !self.doc.layer_effective_visible(layer.id) {
                continue;
            }
            if let Some(b) = self.doc.elements_bounds(&layer.elements) {
                acc = Some(match acc {
                    Some(a) => a.union(&b),
                    None => b,
                });
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
        self.add_layer_at(name, None, LayerKind::Body)
    }

    /// New layers nest under `parent` (groups are just layers with
    /// children) and become the active layer for new geometry.
    pub fn add_layer_at(&mut self, name: &str, parent: Option<u64>, kind: LayerKind) -> u64 {
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        self.doc.layers.push(Layer {
            id,
            name: name.into(),
            elements: Vec::new(),
            visible: true,
            parent,
            kind: Some(kind),
        });
        self.active_layer = id;
        id
    }

    /// The layer new geometry lands in: active when it exists, else
    /// the first layer. Callers must never index `layers[0]` directly
    /// (deleted/loaded docs go stale).
    pub fn active_layer_id(&self) -> u64 {
        if self.doc.layer(self.active_layer).is_some() {
            self.active_layer
        } else {
            self.doc.layers.first().map(|l| l.id).unwrap_or(1)
        }
    }

    pub fn rename_layer(&mut self, id: u64, name: &str, cx: &mut gpui::Context<Self>) {
        let name = name.trim();
        if name.is_empty() || self.doc.layer(id).is_none() {
            return;
        }
        self.history_begin();
        self.doc.rename_layer(id, name);
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn toggle_layer_visible(&mut self, id: u64, cx: &mut gpui::Context<Self>) {
        let Some(layer) = self.doc.layer(id) else { return };
        let next = !layer.visible;
        self.history_begin();
        self.doc.set_layer_visible(id, next);
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    pub fn move_layer_sibling(&mut self, id: u64, dir: i8, cx: &mut gpui::Context<Self>) {
        self.history_begin();
        if self.doc.move_layer(id, dir) {
            self.doc_gen += 1;
        }
        self.flush_pending_history();
        cx.notify();
    }

    pub fn indent_layer(&mut self, id: u64, cx: &mut gpui::Context<Self>) {
        self.history_begin();
        if self.doc.indent_layer(id) {
            self.doc_gen += 1;
        }
        self.flush_pending_history();
        cx.notify();
    }

    pub fn outdent_layer(&mut self, id: u64, cx: &mut gpui::Context<Self>) {
        self.history_begin();
        if self.doc.outdent_layer(id) {
            self.doc_gen += 1;
        }
        self.flush_pending_history();
        cx.notify();
    }

    /// Splits flat/multi-island layers into one body layer per island
    /// (shape-named). Geometry ids are stable — only layer records are
    /// rewritten — so selection/constraints/dims survive untouched.
    pub fn organize_layers(&mut self, cx: &mut gpui::Context<Self>) {
        self.history_begin();
        let next = self.doc.organize_bodies(self.next_layer_id);
        self.next_layer_id = next.max(self.next_layer_id);
        if self.doc.layer(self.active_layer).is_none() {
            self.active_layer = self.doc.layers.first().map(|l| l.id).unwrap_or(1);
        }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
    }

    /// Deletes the layer subtree: every element dies through the
    /// standard `delete_element` path (constraints/dims/fillets follow),
    /// then the records go. The last layer standing refuses to die —
    /// it is emptied instead so creation always has a home.
    pub fn delete_layer(&mut self, id: u64, cx: &mut gpui::Context<Self>) {
        if self.doc.layer(id).is_none() {
            return;
        }
        self.history_begin();
        if self.doc.layers.len() <= 1 {
            let els: Vec<ElementRef> =
                self.doc.layer(id).map(|l| l.elements.clone()).unwrap_or_default();
            for el in els {
                self.delete_element(el);
            }
            if let Some(layer) = self.doc.layer_mut(id) {
                layer.elements.clear();
            }
        } else {
            let subtree = self.doc.layer_subtree_ids(id);
            let mut els = Vec::new();
            for sid in &subtree {
                if let Some(layer) = self.doc.layer(*sid) {
                    els.extend(layer.elements.iter().copied());
                }
            }
            for el in els {
                self.delete_element(el);
            }
            self.doc.remove_layer_records(id);
            if self.doc.layers.is_empty() {
                let nid = self.next_layer_id;
                self.next_layer_id += 1;
                self.doc.layers.push(Layer {
                    id: nid,
                    name: "Layer 1".into(),
                    elements: Vec::new(),
                    visible: true,
                    parent: None,
                    kind: Some(LayerKind::Body),
                });
            }
        }
        self.selection.clear();
        if self.doc.layer(self.active_layer).is_none() {
            self.active_layer = self.doc.layers.first().map(|l| l.id).unwrap_or(1);
        }
        self.doc_gen += 1;
        self.flush_pending_history();
        cx.notify();
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

#[cfg(test)]
mod zoom_tests {
    use super::*;

    #[test]
    fn wheel_forward_zooms_in() {
        let mut ed = Editor::new();
        let z0 = ed.camera.zoom;
        let c = gpui::Point {
            x: gpui::px(100.),
            y: gpui::px(100.),
        };
        ed.zoom_at(c, 120.);
        assert!(ed.camera.zoom > z0, "forward must zoom in");
        ed.zoom_at(c, -240.);
        assert!(ed.camera.zoom < z0, "backward must zoom out");
    }
}
