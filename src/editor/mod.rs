mod camera;
pub mod pick;

pub use camera::Camera;

use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::core::document::{Document, Layer};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{FillId, PointId};

// The session: the permanent design plus view/editing state.
// Owns nothing about GPUI widgets; the UI layer drives it.

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

// Active canvas tool. Move/Pan are modes; shape tools emit element
// composites (the document has no "rectangle" object).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Move,
    Pan,
    Rectangle,
}

// In-progress rectangle being dragged out (tool-side preview only).
#[derive(Clone, Copy, Debug)]
pub struct PendingShape {
    pub start: Point2,
    pub cursor: Point2,
    // Shift held: keep width == height (perfect square).
    pub proportional: bool,
}

impl PendingShape {
    pub fn bounds(&self) -> Rect {
        if !self.proportional {
            return Rect::from_points(self.start, self.cursor);
        }
        let dx = self.cursor.x - self.start.x;
        let dy = self.cursor.y - self.start.y;
        let d = dx.abs().max(dy.abs());
        let constrained = Point2::new(
            self.start.x + d * dx.signum(),
            self.start.y + d * dy.signum(),
        );
        Rect::from_points(self.start, constrained)
    }
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

// Screen-space render data for one dimension: measured endpoints, the
// parallel dim line, extension stubs, and the label anchor. Computed once
// per frame; paint draws the lines, the DOM layer draws labels.
#[derive(Clone, Debug)]
pub struct DimRender {
    pub ax: f32,
    pub ay: f32,
    pub bx: f32,
    pub by: f32,
    pub lax: f32,
    pub lay: f32,
    pub lbx: f32,
    pub lby: f32,
    pub label_cx: f32,
    pub label_cy: f32,
    pub text: String,
    // Additional extension lines (screen x1,y1,x2,y2) reaching from other
    // objects' extremes to this dim line.
    pub extra_ext: Vec<[f32; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Edge,
}

// A visual snap connection: what locked onto what. Edge snaps carry the
// target's full span so rendering can trace it.
#[derive(Clone, Copy, Debug)]
pub struct SnapGuide {
    pub vertical: bool,
    pub from: Point2,
    pub to: Point2,
    pub kind: SnapKind,
    pub span_is_x: bool,
    pub span_lo: f64,
    pub span_hi: f64,
}

// One candidate location other geometry exposes for snapping. Points offer
// both axes; edges snap only their normal axis within their span.
#[derive(Clone, Copy, Debug)]
struct SnapTarget {
    x: f64,
    y: f64,
    kind: SnapKind,
    snap_x: bool,
    snap_y: bool,
    span_lo: f64,
    span_hi: f64,
    span_is_x: bool,
}

pub struct Editor {
    pub doc: Document,
    pub camera: Camera,
    pub tool: Tool,
    pub pending_shape: Option<PendingShape>,
    // Pending rectangle created by a single click (commit on next click).
    pub pending_via_click: bool,
    pub selection: Vec<ElementRef>,
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
    // Per-frame dimension render data (preview + stored dims).
    pub dim_renders: Vec<DimRender>,
    // Last known cursor + modifier state so changes can re-derive drags.
    pub last_cursor: Option<gpui::Point<gpui::Pixels>>,
    pub shift: bool,
    pub alt_down: bool,
    pub(crate) dragging: Option<DragState>,
    next_layer_id: u64,
    pan_start: Option<(gpui::Pixels, gpui::Pixels, Camera)>,
}

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
            pending_via_click: false,
            selection: Vec::new(),
            hover: None,
            marquee: None,
            marquee_add: false,
            deferred_pick: None,
            group_drag_last: None,
            snap_guides: Vec::new(),
            dim_renders: Vec::new(),
            last_cursor: None,
            shift: false,
            alt_down: false,
            dragging: None,
            next_layer_id: next_layer_id.max(2),
            pan_start: None,
        }
    }

    pub fn set_tool(&mut self, tool: Tool) -> bool {
        if self.tool == tool {
            return false;
        }
        self.tool = tool;
        self.pending_shape = None;
        self.pending_via_click = false;
        self.selection.clear();
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
                self.begin_pan(cursor);
                true
            }
            gpui::MouseButton::Left => match self.tool {
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
                    let (at, guides) = self.snap_point(self.cursor_doc(cursor));
                    self.snap_guides = guides;
                    self.pending_shape =
                        Some(PendingShape { start: at, cursor: at, proportional: false });
                    self.pending_via_click = true;
                    true
                }
                Tool::Move => {
                    let p = self.cursor_doc(cursor);
                    let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
                    // Shift extends the selection instead of replacing it.
                    self.marquee_add = shift;

                    // Exact hit (tight tolerance) grabs immediately. A
                    // TOLERANT-only hit near geometry stays a marquee — the
                    // grab zone around points/edges must not create dead
                    // zones for band selection. The deferred pick resolves
                    // on mouse-up as a click if the band never grew.
                    let exact = pick::Picker::new(&self.doc, &self.camera, EXACT_TOL_PX).element(p);
                    match picker.element(p) {
                        Some(mut el) => {
                            if exact.is_none() {
                                // Tolerant-only hit: marquee wins; the pick
                                // resolves as a click on mouse-up if the
                                // band never grows.
                                self.deferred_pick = Some(el);
                                self.marquee = Some((p, p));
                                return true;
                            }
                            // Double-click on an edge escalates to its
                            // containing object.
                            if click_count >= 2
                                && let Some(sid) = el.as_segment()
                                && let Some(fid) = self.fill_containing(sid)
                            {
                                el = ElementRef::Fill(fid);
                            }
                            // Pressing part of an ALREADY-SELECTED object
                            // keeps the whole selection; pressing something
                            // unselected replaces it — unless shift adds.
                            if !self.element_selected(el) {
                                if self.marquee_add {
                                    self.selection.push(el);
                                } else {
                                    self.selection = vec![el];
                                }
                            }

                            // Grab semantics by what was pressed:
                            //  - POINT -> resize mode: the corner chases the
                            //    cursor; constraint-neighbors join as soft
                            //    followers so they slide along their edges.
                            //  - SEGMENT/FILL -> selection-wide drag; every
                            //    unselected involved point is hard-fixed, so
                            //    the solver stretches the selected sub-shape
                            //    instead of translating the whole object.
                            let (drag_pts, aux_pts) = if let ElementRef::Point(pid) = el {
                                let mut ring: Vec<PointId> = Vec::new();
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
                                        && !ring.contains(&o)
                                        && self.doc.point(o).is_some()
                                    {
                                        ring.push(o);
                                    }
                                }
                                let start = self.doc.point(pid).unwrap();
                                let aux = ring
                                    .iter()
                                    .map(|&o| (o, self.doc.point(o).unwrap()))
                                    .collect();
                                (vec![(pid, start)], aux)
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
            },
            _ => false,
        }
    }

    pub fn canvas_drag(&mut self, cursor: gpui::Point<gpui::Pixels>, shift: bool) -> bool {
        self.last_cursor = Some(cursor);
        self.shift = shift;
        if self.pan_delta(cursor) {
            self.snap_guides.clear();
            return true;
        }

        // Rectangle rubber band.
        if self.pending_shape.is_some() {
            let at = self.cursor_doc(cursor);
            let (at, guides) = self.snap_point(at);
            self.snap_guides = guides;
            if let Some(pending) = self.pending_shape.as_mut() {
                pending.cursor = at;
                pending.proportional = shift;
            }
            return true;
        }

        // Group move (drag started on an already-selected element without
        // reselecting): plain translate, no per-point clamping.
        if self.dragging.is_none() && self.group_drag_last.is_some() {
            return false;
        }

        // Live constraint-solve drag: cursor targets go in, solved
        // positions come out — geometry satisfies H/V/dimension
        // constraints continuously while dragging.
        if self.dragging.is_some() {
            let drag = self.dragging.as_ref().unwrap();
            let p = self.cursor_doc(cursor);
            let delta = Point2::new(p.x - drag.start_cursor.x, p.y - drag.start_cursor.y);
            if delta.x == 0. && delta.y == 0. {
                return false;
            }
            // Snap exclusion: everything belonging to the dragged system —
            // dragged points, aux followers, and any point of the selected
            // objects. Snapping is for OTHER objects only; snapping to the
            // shape you're resizing is what made corner drags feel jumpy.
            let mut exclude: Vec<PointId> = drag.points.iter().map(|(id, _)| *id).collect();
            for &(pid, _) in &drag.aux {
                if !exclude.contains(&pid) {
                    exclude.push(pid);
                }
            }
            for pid in self.doc.selection_points(&self.selection) {
                if !exclude.contains(&pid) {
                    exclude.push(pid);
                }
            }

            // Endpoint-only snapping keeps drags fluid; the smallest
            // correction across dragged points wins.
            let mut snapped_delta = delta;
            if self.snapping_active() {
                let mut best: Option<(f64, Point2)> = None;
                for &(_pid, start) in &drag.points {
                    let target = Point2::new(start.x + delta.x, start.y + delta.y);
                    let (adj, _) = self.best_snap(target, &exclude, true);
                    let score = adj.x.abs() + adj.y.abs();
                    if best.map_or(true, |(s, _)| score < s) {
                        best = Some((score, adj));
                    }
                }
                if let Some((_, adj)) = best {
                    snapped_delta = Point2::new(delta.x + adj.x, delta.y + adj.y);
                }
            }

            let targets: Vec<(PointId, Point2)> = drag
                .points
                .iter()
                .map(|&(pid, start)| {
                    (
                        pid,
                        Point2::new(start.x + snapped_delta.x, start.y + snapped_delta.y),
                    )
                })
                .collect();

            let solver =
                crate::core::solver::Solver::build(&self.doc, &targets, &drag.aux);
            let solution = solver.solve();
            for (id, pos) in solution.positions {
                self.doc.move_point(id, pos);
            }
            return true;
        }

        // Marquee band update.
        if let Some((start, _)) = self.marquee {
            let cur = self.cursor_doc(cursor);
            self.marquee = Some((start, cur));
            return true;
        }
        self.snap_guides.clear();
        false
    }

    // -- snapping --

    fn snapping_active(&self) -> bool {
        !self.alt_down
    }

    // All snap locations exposed by the geometry. `endpoints_only` keeps
    // drags fluid — midpoints and edge spans apply only to precise
    // placement (rectangle tool), never while dragging geometry.
    fn snap_targets(&self, exclude: &[PointId], endpoints_only: bool) -> Vec<SnapTarget> {
        let mut out = Vec::new();
        for (pid, p) in self.doc.all_points() {
            if exclude.contains(&pid) {
                continue;
            }
            out.push(SnapTarget {
                x: p.x,
                y: p.y,
                kind: SnapKind::Endpoint,
                snap_x: true,
                snap_y: true,
                span_lo: 0.,
                span_hi: 0.,
                span_is_x: false,
            });
        }
        if endpoints_only {
            return out;
        }
        for (sid, seg) in self.doc.all_segments() {
            let Some((a, b)) = self.doc.segment_geom(sid) else { continue };
            let m = pick::mid(a, b);
            out.push(SnapTarget {
                x: m.x,
                y: m.y,
                kind: SnapKind::Midpoint,
                snap_x: true,
                snap_y: true,
                span_lo: 0.,
                span_hi: 0.,
                span_is_x: false,
            });
            let _ = seg;
            // Edge spans: horizontal edge snaps Y within X range, vertical
            // edge snaps X within Y range.
            let horizontal = (a.y - b.y).abs() < 1e-9;
            let vertical = (a.x - b.x).abs() < 1e-9;
            if horizontal {
                let (lo, hi) = (a.x.min(b.x), a.x.max(b.x));
                out.push(SnapTarget {
                    x: m.x,
                    y: a.y,
                    kind: SnapKind::Edge,
                    snap_x: false,
                    snap_y: true,
                    span_lo: lo,
                    span_hi: hi,
                    span_is_x: true,
                });
            } else if vertical {
                let (lo, hi) = (a.y.min(b.y), a.y.max(b.y));
                out.push(SnapTarget {
                    x: a.x,
                    y: m.y,
                    kind: SnapKind::Edge,
                    snap_x: true,
                    snap_y: false,
                    span_lo: lo,
                    span_hi: hi,
                    span_is_x: false,
                });
            }
        }
        out
    }

    fn span_ok(t: &SnapTarget, p: Point2) -> bool {
        if t.kind != SnapKind::Edge {
            return true;
        }
        let (lo, hi) = (t.span_lo.min(t.span_hi), t.span_lo.max(t.span_hi));
        if t.span_is_x {
            p.x >= lo && p.x <= hi
        } else {
            p.y >= lo && p.y <= hi
        }
    }

    // Best single correction for a point against all targets. Returns
    // (adjustment delta, guides).
    fn best_snap(&self, p: Point2, exclude: &[PointId], endpoints_only: bool) -> (Point2, Vec<SnapGuide>) {
        if !self.snapping_active() {
            return (Point2::new(0., 0.), Vec::new());
        }
        let tol = SNAP_TOL_PX / self.camera.zoom;
        let mut best: Option<(f64, f64, f64, bool, bool, SnapTarget)> = None;
        for tgt in self.snap_targets(exclude, endpoints_only) {
            let dx = tgt.x - p.x;
            let dy = tgt.y - p.y;
            let hit_x = tgt.snap_x && dx.abs() <= tol && Self::span_ok(&tgt, p);
            let hit_y = tgt.snap_y && dy.abs() <= tol && Self::span_ok(&tgt, p);
            if !hit_x && !hit_y {
                continue;
            }
            let score = dx.abs() + dy.abs();
            if best.as_ref().map_or(true, |(s, _, _, _, _, _)| score < *s) {
                best = Some((score, dx, dy, hit_x, hit_y, tgt));
            }
        }
        let Some((_, dx, dy, hit_x, hit_y, tgt)) = best else {
            return (Point2::new(0., 0.), Vec::new());
        };
        let mut adj = Point2::new(0., 0.);
        let mut guides = Vec::new();
        if hit_x {
            adj.x = dx;
            guides.push(SnapGuide {
                vertical: true,
                from: p,
                to: Point2::new(tgt.x, p.y),
                kind: tgt.kind,
                span_is_x: tgt.span_is_x,
                span_lo: tgt.span_lo,
                span_hi: tgt.span_hi,
            });
        }
        if hit_y {
            adj.y = dy;
            guides.push(SnapGuide {
                vertical: false,
                from: p,
                to: Point2::new(p.x, tgt.y),
                kind: tgt.kind,
                span_is_x: tgt.span_is_x,
                span_lo: tgt.span_lo,
                span_hi: tgt.span_hi,
            });
        }
        (adj, guides)
    }

    // Snaps a free point (rectangle tool placement) to the best target.
    fn snap_point(&self, p: Point2) -> (Point2, Vec<SnapGuide>) {
        let (adj, guides) = self.best_snap(p, &[], false);
        (Point2::new(p.x + adj.x, p.y + adj.y), guides)
    }

    // -- dimensions (per-frame render data) --

    // Recomputes dimension geometry. Called once per frame before painting;
    // lines and labels both read this.
    pub fn update_dim_geom(&mut self) {
        self.dim_renders.clear();

        // Multi-object selection: every object shows its OWN W+H dims, plus
        // a TOTAL pair for the whole selection whose extension lines run
        // all the way to each object's nearest extreme.
        let sel_fills: Vec<FillId> = self
            .selection
            .iter()
            .filter_map(|el| el.as_fill())
            .filter(|fid| self.doc.fill(*fid).is_some())
            .collect();
        if !sel_fills.is_empty() {
            for fid in &sel_fills {
                if let Some(b) = self.doc.fill_bounds(*fid) {
                    self.push_wh_dims(b);
                }
            }
            if sel_fills.len() > 1 {
                let mut total: Option<Rect> = None;
                for fid in &sel_fills {
                    if let Some(b) = self.doc.fill_bounds(*fid) {
                        total = Some(match total {
                            Some(t) => t.union(&b),
                            None => b,
                        });
                    }
                }
                if let Some(u) = total {
                    let mut extras: Vec<(Point2, Point2)> = Vec::new();
                    let off = PREVIEW_DIM_OFFSET_DOC;
                    // W dim (bottom): verticals from each object's bottom
                    // corners down to the dim line's y.
                    let dim_y = u.origin.y + u.size.h + off;
                    for fid in &sel_fills {
                        if let Some(b) = self.doc.fill_bounds(*fid) {
                            let by = b.origin.y + b.size.h;
                            extras.push((
                                Point2::new(b.origin.x, by),
                                Point2::new(b.origin.x, dim_y),
                            ));
                            extras.push((
                                Point2::new(b.origin.x + b.size.w, by),
                                Point2::new(b.origin.x + b.size.w, dim_y),
                            ));
                        }
                    }
                    let bl = Point2::new(u.origin.x, u.origin.y + u.size.h);
                    let br = Point2::new(u.origin.x + u.size.w, u.origin.y + u.size.h);
                    self.dim_renders.push(self.linear_dim_extras(bl, br, off, u.size.w, &extras));
                    // H dim (right): horizontals from each object's right
                    // edge across to the dim line's x.
                    let mut extras: Vec<(Point2, Point2)> = Vec::new();
                    let dim_x = u.origin.x + u.size.w + off;
                    for fid in &sel_fills {
                        if let Some(b) = self.doc.fill_bounds(*fid) {
                            let rx = b.origin.x + b.size.w;
                            extras.push((
                                Point2::new(rx, b.origin.y),
                                Point2::new(dim_x, b.origin.y),
                            ));
                            extras.push((
                                Point2::new(rx, b.origin.y + b.size.h),
                                Point2::new(dim_x, b.origin.y + b.size.h),
                            ));
                        }
                    }
                    let br = Point2::new(u.origin.x + u.size.w, u.origin.y + u.size.h);
                    let tr = Point2::new(u.origin.x + u.size.w, u.origin.y);
                    self.dim_renders.push(self.linear_dim_extras(br, tr, off, u.size.h, &extras));
                }
            }
            return;
        }

        // A lone selected edge shows the dim of the axis being resized:
        // left/right edges -> WIDTH dim under the shape; top/bottom edges
        // -> HEIGHT dim right of the shape. Applies WHILE dragging too.
        if self.pending_shape.is_none()
            && self.selection.len() == 1
            && let Some(sid) = self.selection[0].as_segment()
            && let Some((a, b)) = self.doc.segment_geom(sid)
        {
            for (fid, f) in self.doc.all_fills() {
                if !f.segments.contains(&sid) {
                    continue;
                }
                let Some(bounds) = self.doc.fill_bounds(fid) else { break };
                let vertical_edge = (b.x - a.x).abs() <= 1e-9;
                if vertical_edge {
                    // Width: along the bottom, offset downward.
                    let bl = Point2::new(bounds.origin.x, bounds.origin.y + bounds.size.h);
                    let br = Point2::new(bounds.origin.x + bounds.size.w, bounds.origin.y + bounds.size.h);
                    self.dim_renders.push(self.linear_dim(bl, br, PREVIEW_DIM_OFFSET_DOC, bounds.size.w));
                } else {
                    // Height: along the right, offset rightward.
                    let tr = Point2::new(bounds.origin.x + bounds.size.w, bounds.origin.y);
                    let br = Point2::new(bounds.origin.x + bounds.size.w, bounds.origin.y + bounds.size.h);
                    self.dim_renders.push(self.linear_dim(br, tr, PREVIEW_DIM_OFFSET_DOC, bounds.size.h));
                }
                return;
            }
        }

        // Dragging a single point that belongs to a closed loop: show the
        // loop's W+H dims (bottom + right) while it resizes.
        if self.pending_shape.is_none()
            && self.selection.len() == 1
            && let Some(pid) = self.selection[0].as_point()
            && let Some(fid) = self.fill_containing_point(pid)
            && let Some(b) = self.doc.fill_bounds(fid)
        {
            if b.size.w > 0. {
                let bl = Point2::new(b.origin.x, b.origin.y + b.size.h);
                let br = Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h);
                self.dim_renders.push(self.linear_dim(bl, br, PREVIEW_DIM_OFFSET_DOC, b.size.w));
            }
            if b.size.h > 0. {
                let br = Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h);
                let tr = Point2::new(b.origin.x + b.size.w, b.origin.y);
                self.dim_renders.push(self.linear_dim(br, tr, PREVIEW_DIM_OFFSET_DOC, b.size.h));
            }
            return;
        }

        // Live preview: bounding box W/H while creating or interacting.
        let preview_box = self.preview_bounds();
        if let Some(b) = preview_box {
            if b.size.w > 0. {
                self.dim_renders.push(self.linear_dim(
                    Point2::new(b.origin.x, b.origin.y + b.size.h),
                    Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h),
                    PREVIEW_DIM_OFFSET_DOC,
                    b.size.w,
                ));
            }
            if b.size.h > 0. {
                // Bottom-right -> top-right so the LEFT normal points right
                // (outside the shape).
                self.dim_renders.push(self.linear_dim(
                    Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h),
                    Point2::new(b.origin.x + b.size.w, b.origin.y),
                    PREVIEW_DIM_OFFSET_DOC,
                    b.size.h,
                ));
            }
        }

        // Stored dimensions: rendered at their own angle and offset.
        for d in &self.doc.dimensions {
            let (Some(a), Some(b)) = (self.doc.point(d.a), self.doc.point(d.b)) else {
                continue;
            };
            let len = d.value.unwrap_or_else(|| pick::distance(a, b));
            self.dim_renders.push(self.linear_dim(a, b, d.offset, len));
        }
    }

    // Bounds shown by preview dims: pending rubber band, else active drag
    // points, else the selection.
    fn preview_bounds(&self) -> Option<Rect> {
        if let Some(p) = &self.pending_shape {
            let b = p.bounds();
            return (b.size.w > 0. && b.size.h > 0.).then_some(b);
        }
        let ids: Vec<PointId> = if let Some(drag) = &self.dragging {
            drag.points.iter().map(|(id, _)| *id).collect()
        } else if !self.selection.is_empty() {
            self.doc.selection_points(&self.selection)
        } else {
            return None;
        };
        self.doc.bounds_of_points(&ids)
    }

    // W+H dims for one object's bounds (bottom + right).
    fn push_wh_dims(&mut self, b: Rect) {
        if b.size.w > 0. {
            let bl = Point2::new(b.origin.x, b.origin.y + b.size.h);
            let br = Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h);
            self.dim_renders.push(self.linear_dim(bl, br, PREVIEW_DIM_OFFSET_DOC, b.size.w));
        }
        if b.size.h > 0. {
            let br = Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h);
            let tr = Point2::new(b.origin.x + b.size.w, b.origin.y);
            self.dim_renders.push(self.linear_dim(br, tr, PREVIEW_DIM_OFFSET_DOC, b.size.h));
        }
    }

    /// linear_dim plus extra extension segments (doc coords), used by total
    /// selection dims so witness lines reach every object's nearest point.
    fn linear_dim_extras(
        &self,
        a: Point2,
        b: Point2,
        offset_doc: f64,
        value: f64,
        extras: &[(Point2, Point2)],
    ) -> DimRender {
        let mut d = self.linear_dim(a, b, offset_doc, value);
        for &(p, q) in extras {
            let sp = self.camera.unit_to_screen(p);
            let sq = self.camera.unit_to_screen(q);
            d.extra_ext.push([sp.x as f32, sp.y as f32, sq.x as f32, sq.y as f32]);
        }
        d
    }

    /// Builds screen-space render data for a dimension between two doc
    /// points. `offset_doc` shifts the dim line along the LEFT normal of
    /// b-a; `value` is the displayed measurement.
    fn linear_dim(&self, a: Point2, b: Point2, offset_doc: f64, value: f64) -> DimRender {
        let scr = |p: Point2| self.camera.unit_to_screen(p);
        let sa = scr(a);
        let sb = scr(b);
        let dx = sb.x - sa.x;
        let dy = sb.y - sa.y;
        let len = (dx * dx + dy * dy).sqrt().max(1e-9);
        // Left normal in screen space (y down).
        let nx = -dy / len;
        let ny = dx / len;
        let off = offset_doc * self.camera.zoom;
        let lax = sa.x + nx * off;
        let lay = sa.y + ny * off;
        let lbx = sb.x + nx * off;
        let lby = sb.y + ny * off;
        DimRender {
            ax: sa.x as f32,
            ay: sa.y as f32,
            bx: sb.x as f32,
            by: sb.y as f32,
            lax: lax as f32,
            lay: lay as f32,
            lbx: lbx as f32,
            lby: lby as f32,
            label_cx: ((lax + lbx) / 2.) as f32,
            label_cy: ((lay + lby) / 2.) as f32,
            text: crate::ui::canvas::fmt_dim(value),
            extra_ext: Vec::new(),
        }
    }

    // -- hover --

    pub fn canvas_hover(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        if self.tool != Tool::Move || self.dragging.is_some() || self.pan_start.is_some() {
            return false;
        }
        let p = self.cursor_doc(cursor);
        let picker = pick::Picker::new(&self.doc, &self.camera, HANDLE_TOL_PX);
        let info = picker.element(p);
        if self.hover != info {
            self.hover = info;
            return true;
        }
        false
    }

    pub fn cursor_style(&self) -> gpui::CursorStyle {
        use gpui::CursorStyle;
        if self.pan_start.is_some() {
            return CursorStyle::ClosedHand;
        }
        if self.tool == Tool::Pan {
            return CursorStyle::OpenHand;
        }
        if self.tool == Tool::Rectangle {
            return CursorStyle::Crosshair;
        }
        CursorStyle::Arrow
    }

    pub fn canvas_up(&mut self, button: gpui::MouseButton) -> bool {
        if button == gpui::MouseButton::Middle && self.end_pan() {
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

        // Click-created pending rectangles survive mouse-up; they commit on
        // the next click instead. Only drag-created ones commit here.
        if self.pending_via_click {
            return true;
        }
        let Some(pending) = self.pending_shape.take() else {
            return false;
        };
        // Ignore click-without-drag.
        if (pending.cursor.x - pending.start.x).abs() < 1e-9
            && (pending.cursor.y - pending.start.y).abs() < 1e-9
        {
            return true;
        }
        let b = pending.bounds();
        self.tool = Tool::Move;
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
                // A corner owned by a selected edge belongs to the selection.
                ElementRef::Segment(s) => self
                    .doc
                    .segment(s)
                    .is_some_and(|seg| seg.start == pid || seg.end == pid),
                ElementRef::Fill(f) => self.doc.element_points(ElementRef::Fill(f)).contains(&pid),
                _ => false,
            }),
            _ => false,
        }
    }

    // The fill whose loop passes through this segment, if any.
    fn fill_containing(&self, sid: crate::core::ids::SegmentId) -> Option<FillId> {
        self.doc
            .all_fills()
            .find(|(_, f)| f.segments.contains(&sid))
            .map(|(id, _)| id)
    }

    // The fill whose loop references this point, if any.
    fn fill_containing_point(&self, pid: crate::core::ids::PointId) -> Option<FillId> {
        self.doc.all_fills().find(|(_, f)| f.segments.iter().any(|&s| {
            self.doc.segment(s).is_some_and(|seg| seg.start == pid || seg.end == pid)
        })).map(|(id, _)| id)
    }

    /// Deletes an element from the document and clears it from selection.
    pub fn delete_element(&mut self, el: ElementRef) {
        match el {
            ElementRef::Point(p) => {
                self.doc.remove_point(p);
            }
            ElementRef::Segment(s) => {
                self.doc.remove_segment(s);
            }
            ElementRef::Fill(f) => {
                self.doc.remove_fill(f);
            }
        }
        self.selection.retain(|&e| e != el);
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


const HANDLE_TOL_PX: f64 = 11.0;
// Tight tolerance for press-to-grab: inside this, a drag moves geometry;
// outside it (but within HANDLE_TOL_PX), a drag is a marquee.
const EXACT_TOL_PX: f64 = 4.5;
const SNAP_TOL_PX: f64 = 6.0;
const PREVIEW_DIM_OFFSET_DOC: f64 = 18.0;
