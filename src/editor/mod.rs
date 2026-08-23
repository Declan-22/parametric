mod camera;

pub use camera::Camera;

use crate::core::document::{Document, Layer, ShapeKind};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::ShapeId;

// The session: the permanent design plus view/editing state.
// Owns nothing about GPUI widgets; the UI layer drives it.

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

// Active canvas tool. Move/Pan are modes; the shape tools create on drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Move,
    Pan,
    Rectangle,
    Ellipse,
}

// In-progress shape being dragged out.
#[derive(Clone, Copy, Debug)]
pub struct PendingShape {
    pub kind: ShapeKind,
    pub start: Point2,
    pub cursor: Point2,
    // Shift held: keep width == height (perfect square/circle).
    pub proportional: bool,
}

// The eight resize zones around a selected shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

impl Handle {
    pub const CORNERS: [Handle; 4] = [Handle::Nw, Handle::Ne, Handle::Se, Handle::Sw];

    fn moves_x(self) -> bool {
        matches!(self, Handle::Nw | Handle::W | Handle::Sw)
    }

    fn moves_y(self) -> bool {
        matches!(self, Handle::Nw | Handle::N | Handle::Ne)
    }
}

// Active corner/side resize of the selected shape.
#[derive(Clone, Copy, Debug)]
pub struct ResizeState {
    pub id: ShapeId,
    pub handle: Handle,
    orig: Rect,
}

// Screen-space selection rectangle plus extension offset for dimensions.
#[derive(Clone, Copy, Debug)]
pub struct DimGeom {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub ext: f32,
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

pub struct Editor {
    pub doc: Document,
    pub camera: Camera,
    pub tool: Tool,
    pub pending_shape: Option<PendingShape>,
    pub selection: Option<ShapeId>,
    pub hover_handle: Option<Handle>,
    // Selection dimension geometry, computed once per frame in screen space.
    // Single source of truth for both the painted lines and the label DOM.
    pub dim_geom: Option<DimGeom>,
    // Active snap connections: from the dragged shape's key point to the
    // matched target, rendered as segment + markers.
    pub snap_guides: Vec<SnapGuide>,
    // Last known cursor + shift state, so modifier changes can re-derive
    // the pending drag/resize instantly.
    pub last_cursor: Option<gpui::Point<gpui::Pixels>>,
    pub shift: bool,
    pub alt_down: bool,
    dragging: Option<SelectionDrag>,
    resizing: Option<ResizeState>,
    next_layer_id: u64,
    pan_start: Option<(gpui::Pixels, gpui::Pixels, Camera)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Corner,
    Midpoint,
    Center,
    Edge,
}

// A visual snap connection: what locked onto what. Edge snaps carry the
// target's full span so rendering can trace the entire side.
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

// One candidate location other shapes expose for snapping. Corners are
// points; edges carry a span along their axis so any position along the
// edge can snap (point-on-edge).
#[derive(Clone, Copy, Debug)]
pub struct SnapTarget {
    pub x: f64,
    pub y: f64,
    pub kind: SnapKind,
    pub snap_x: bool,
    pub snap_y: bool,
    pub has_span: bool,
    pub span_lo: f64,
    pub span_hi: f64,
    pub span_is_x: bool,
}

#[derive(Clone, Copy, Debug)]
struct SelectionDrag {
    id: ShapeId,
    // Cursor offset from the shape's bounds origin at grab time.
    grab_offset: Point2,
}

impl Editor {
    pub fn new() -> Self {
        let mut doc = Document::new();
        doc.layers.push(Layer {
            id: 1,
            name: "Layer 1".into(),
            shape_ids: Vec::new(),
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
            selection: None,
            hover_handle: None,
            dim_geom: None,
            snap_guides: Vec::new(),
            last_cursor: None,
            shift: false,
            alt_down: false,
            dragging: None,
            resizing: None,
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
        self.selection = None;
        self.dragging = None;
        true
    }

    // -- canvas input (called from the canvas view) --

    fn cursor_doc(&self, cursor: gpui::Point<gpui::Pixels>) -> Point2 {
        self.camera
            .screen_to_unit(Point2::new(f64::from(cursor.x), f64::from(cursor.y)))
    }

    // Mouse down on the canvas. Returns true if a repaint is needed.
    pub fn canvas_down(&mut self, button: gpui::MouseButton, cursor: gpui::Point<gpui::Pixels>) -> bool {
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
                Tool::Rectangle | Tool::Ellipse => {
                    let kind = match self.tool {
                        Tool::Rectangle => ShapeKind::Rectangle,
                        _ => ShapeKind::Ellipse,
                    };
                    let (at, guides) = self.snap_point(self.cursor_doc(cursor), None);
                    self.snap_guides = guides;
                    self.pending_shape = Some(PendingShape {
                        kind,
                        start: at,
                        cursor: at,
                        proportional: false,
                    });
                    true
                }
                Tool::Move => {
                    let p = self.cursor_doc(cursor);
                    let tol = self.handle_tolerance();

                    // 1) Resize handles of the current selection win.
                    if let Some(sel) = self.selection
                        && let Some(b) = self.doc.shape_bounds(sel)
                        && let Some(handle) = handle_at(b, p, tol)
                    {
                        self.resizing = Some(ResizeState { id: sel, handle, orig: b });
                        return true;
                    }

                    // 2) Body hit-test, topmost shape wins.
                    let mut hit = None;
                    for layer in self.doc.layers.iter().rev() {
                        for &sid in layer.shape_ids.iter().rev() {
                            if let Some(b) = self.doc.shape_bounds(sid)
                                && b.contains(p)
                            {
                                hit = Some(sid);
                                break;
                            }
                        }
                        if hit.is_some() {
                            break;
                        }
                    }
                    match hit {
                        Some(id) => {
                            let b = self.doc.shape_bounds(id).unwrap();
                            let changed = self.selection != Some(id);
                            self.selection = Some(id);
                            self.dragging = Some(SelectionDrag {
                                id,
                                grab_offset: Point2::new(
                                    p.x - b.origin.x,
                                    p.y - b.origin.y,
                                ),
                            });
                            true
                        }
                        None => {
                            let had = self.selection.is_some();
                            self.selection = None;
                            had
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
        if self.pending_shape.is_none() && self.dragging.is_none() && self.resizing.is_none() {
            self.snap_guides.clear();
            return false;
        }
        if self.pending_shape.is_some() {
            let (cursor_doc, guides) = self.snap_point(self.cursor_doc(cursor), None);
            self.snap_guides = guides;
            let pending = self.pending_shape.as_mut().unwrap();
            pending.cursor = cursor_doc;
            pending.proportional = shift;
            return true;
        }
        if let Some(drag) = self.dragging {
            let p = self.cursor_doc(cursor);
            let Some(b) = self.doc.shape_bounds(drag.id) else {
                return false;
            };
            // Free movement in document units — sub-pixel precision allowed.
            let target_x = p.x - drag.grab_offset.x;
            let target_y = p.y - drag.grab_offset.y;
            let delta = Point2::new(target_x - b.origin.x, target_y - b.origin.y);
            if delta.x == 0. && delta.y == 0. {
                return false;
            }
            // Snap: try aligning any key point of the moving shape with a
            // target; apply the smallest correction.
            let (delta, guides) = self.snap_move_delta(drag.id, b, delta);
            self.snap_guides = guides;
            return self.doc.translate_shape(drag.id, delta);
        }
        // Corner/side resize of the selection.
        if let Some(rs) = self.resizing {
            let p = self.cursor_doc(cursor);
            // Snap the moving corner/edge to nearby targets.
            let (sp, guides) = self.snap_point(p, Some(rs.id));
            self.snap_guides = guides;
            let mut left = rs.orig.origin.x;
            let mut top = rs.orig.origin.y;
            let mut right = left + rs.orig.size.w;
            let mut bottom = top + rs.orig.size.h;
            const EPS: f64 = 1e-6;
            let px_ = sp.x;
            let py_ = sp.y;
            match rs.handle {
                // West edge moves, east edge fixed.
                Handle::Nw | Handle::W | Handle::Sw => {
                    right = rs.orig.origin.x + rs.orig.size.w;
                    left = px_.min(right - EPS);
                }
                // East edge moves, west edge fixed.
                Handle::Ne | Handle::E | Handle::Se => {
                    left = rs.orig.origin.x;
                    right = px_.max(left + EPS);
                }
                _ => {}
            }
            match rs.handle {
                // North edge moves, south edge fixed.
                Handle::Nw | Handle::N | Handle::Ne => {
                    bottom = rs.orig.origin.y + rs.orig.size.h;
                    top = py_.min(bottom - EPS);
                }
                // South edge moves, north edge fixed.
                Handle::Sw | Handle::S | Handle::Se => {
                    top = rs.orig.origin.y;
                    bottom = py_.max(top + EPS);
                }
                _ => {}
            }
            // Shift: keep proportions (corners only — sides move one axis).
            if shift && matches!(rs.handle, Handle::Nw | Handle::Ne | Handle::Se | Handle::Sw) {
                // The fixed opposite corner anchors the scale.
                let ax = if rs.handle.moves_x() { right } else { left };
                let ay = if rs.handle.moves_y() { bottom } else { top };
                let ow = rs.orig.size.w.max(EPS);
                let oh = rs.orig.size.h.max(EPS);
                // Free corner = cursor mirrored around the anchor with a
                // uniform scale from the dominant axis.
                let dx = px_ - ax;
                let dy = py_ - ay;
                let scale = (dx.abs() / ow).max(dy.abs() / oh);
                let fx = ax + dx.signum() * ow * scale;
                let fy = ay + dy.signum() * oh * scale;
                return self.doc.set_shape_corners(
                    rs.id,
                    Rect::from_points(Point2::new(ax, ay), Point2::new(fx, fy)),
                );
            }
            return self.doc.set_shape_corners(
                rs.id,
                Rect::from_points(
                    Point2::new(left.min(right), top.min(bottom)),
                    Point2::new(left.max(right), top.max(bottom)),
                ),
            );
        }
        false
    }

    // Screen-space tolerance (6px) converted to document units.
    fn handle_tolerance(&self) -> f64 {
        6.0 / self.camera.zoom
    }

    // Document-space size of the current selection (for dimension text).
    pub fn selection_size(&self) -> Option<(f64, f64)> {
        let b = self
            .selection
            .and_then(|sel| self.doc.shape_bounds(sel))?;
        Some((b.size.w, b.size.h))
    }

    // -- snapping --

    fn snap_tolerance(&self) -> f64 {
        6.0 / self.camera.zoom
    }

    // Snapping is suppressed while Alt is held — the standard escape hatch
    // when you want free placement.
    fn snapping_active(&self) -> bool {
        !self.alt_down
    }

    // All snap locations exposed by shapes other than `exclude`.
    // Point targets (corners, center) offer both axes. Edge targets snap
    // only their normal axis and require the point to lie within the
    // edge's span on the other axis — that's what makes sides snappable.
    fn snap_targets(&self, exclude: Option<ShapeId>) -> Vec<SnapTarget> {
        let mut out = Vec::new();
        for layer in &self.doc.layers {
            for &sid in &layer.shape_ids {
                if Some(sid) == exclude {
                    continue;
                }
                let Some(b) = self.doc.shape_bounds(sid) else {
                    continue;
                };
                let (l, t) = (b.origin.x, b.origin.y);
                let (r, bm) = (l + b.size.w, t + b.size.h);
                // Representative marker positions on each edge.
                let mx = l + b.size.w / 2.;
                let my = t + b.size.h / 2.;

                // Point targets: corners.
                for (x, y, k) in [
                    (l, t, SnapKind::Corner),
                    (r, t, SnapKind::Corner),
                    (r, bm, SnapKind::Corner),
                    (l, bm, SnapKind::Corner),
                ] {
                    out.push(SnapTarget {
                        x,
                        y,
                        kind: k,
                        snap_x: true,
                        snap_y: true,
                        has_span: false,
                        span_lo: 0.,
                        span_hi: 0.,
                        span_is_x: false,
                    });
                }

                // Edge targets: top/bottom edges snap Y within an X span;
                // left/right edges snap X within a Y span.
                for (x, y, sx, sy, lo, hi, is_x) in [
                    (mx, t, false, true, l, r, true),
                    (mx, bm, false, true, l, r, true),
                    (l, my, true, false, t, bm, false),
                    (r, my, true, false, t, bm, false),
                ] {
                    out.push(SnapTarget {
                        x,
                        y,
                        kind: SnapKind::Edge,
                        snap_x: sx,
                        snap_y: sy,
                        has_span: true,
                        span_lo: lo,
                        span_hi: hi,
                        span_is_x: is_x,
                    });
                }
            }
        }
        out
    }

    // Span constraint: the point must lie along the edge's axis range.
    fn span_ok(t: &SnapTarget, p: Point2) -> bool {
        if !t.has_span {
            return true;
        }
        let (lo, hi) = (t.span_lo.min(t.span_hi), t.span_lo.max(t.span_hi));
        if t.span_is_x {
            p.x >= lo && p.x <= hi
        } else {
            p.y >= lo && p.y <= hi
        }
    }

    // Snaps a point to the SINGLE best target. Edge targets match only on
    // their normal axis while the point lies within their span; only axes
    // that are genuinely close adjust, so one alignment = one guide and
    // the other axis stays free.
    fn snap_point(&self, p: Point2, exclude: Option<ShapeId>) -> (Point2, Vec<SnapGuide>) {
        if !self.snapping_active() {
            return (p, Vec::new());
        }
        let tol = self.snap_tolerance();
        let targets = self.snap_targets(exclude);

        let mut best: Option<(f64, f64, f64, bool, bool)> = None; // (score, x, y, hit_x, hit_y)
        let mut best_kind = SnapKind::Corner;
        let mut best_span = (false, 0., 0.);
        for tgt in &targets {
            let dx = tgt.x - p.x;
            let dy = tgt.y - p.y;
            let hit_x = tgt.snap_x && dx.abs() <= tol && Self::span_ok(tgt, p);
            let hit_y = tgt.snap_y && dy.abs() <= tol && Self::span_ok(tgt, p);
            if !hit_x && !hit_y {
                continue;
            }
            let score = dx.abs() + dy.abs();
            if best.map_or(true, |(s, _, _, _, _)| score < s) {
                best = Some((score, tgt.x, tgt.y, hit_x, hit_y));
                best_kind = tgt.kind;
                best_span = (tgt.span_is_x, tgt.span_lo, tgt.span_hi);
            }
        }

        let Some((_, tx, ty, hit_x, hit_y)) = best else {
            return (p, Vec::new());
        };
        // Project the marker onto the actual snap location along edges.
        let mut tx = tx;
        let mut ty = ty;
        if best_kind == SnapKind::Edge {
            if !hit_x {
                tx = p.x;
            }
            if !hit_y {
                ty = p.y;
            }
        }
        let is_edge = best_kind == SnapKind::Edge;
        let mut out = p;
        let mut guides = Vec::new();
        if hit_x {
            out.x = tx;
            guides.push(SnapGuide {
                vertical: true,
                from: p,
                to: Point2::new(tx, p.y),
                kind: best_kind,
                span_is_x: best_span.0,
                span_lo: best_span.1,
                span_hi: best_span.2,
            });
        }
        if hit_y {
            out.y = ty;
            guides.push(SnapGuide {
                vertical: false,
                from: p,
                to: Point2::new(p.x, ty),
                kind: if is_edge { SnapKind::Edge } else { best_kind },
                span_is_x: best_span.0,
                span_lo: best_span.1,
                span_hi: best_span.2,
            });
        }
        (out, guides)
    }

    // Snaps a moving shape: tries each of its key points (corners + center)
    // against the target set and applies the smallest correction to the
    // motion delta. Guides run from the key point to the matched target.
    fn snap_move_delta(&self, id: ShapeId, b: Rect, delta: Point2) -> (Point2, Vec<SnapGuide>) {
        if !self.snapping_active() {
            return (delta, Vec::new());
        }
        let tol = self.snap_tolerance();
        let targets = self.snap_targets(Some(id));
        let (l, t) = (b.origin.x, b.origin.y);
        let keys = [
            (l, t),
            (l + b.size.w, t),
            (l + b.size.w, t + b.size.h),
            (l, t + b.size.h),
        ];

        let mut best: Option<(f64, usize, f64, f64)> = None; // (score, key_idx, dx, dy)
        let mut best_kind = SnapKind::Corner;
        let mut best_span = (false, 0., 0.);
        for (ki, (kx, ky)) in keys.iter().enumerate() {
            let px_ = kx + delta.x;
            let py_ = ky + delta.y;
            for tgt in &targets {
                let dx = tgt.x - px_;
                let dy = tgt.y - py_;
                let hit_x = tgt.snap_x && dx.abs() <= tol;
                let hit_y = tgt.snap_y && dy.abs() <= tol;
                if !hit_x && !hit_y {
                    continue;
                }
                if !Self::span_ok(tgt, Point2::new(px_, py_)) {
                    continue;
                }
                let score = dx.abs() + dy.abs();
                if best.map_or(true, |(s, _, _, _)| score < s) {
                    best = Some((score, ki, dx, dy));
                    best_kind = tgt.kind;
                    best_span = (tgt.span_is_x, tgt.span_lo, tgt.span_hi);
                }
            }
        }

        match best {
            Some((_, ki, dx, dy)) => {
                let (kx, ky) = keys[ki];
                let from = Point2::new(kx + delta.x, ky + delta.y);
                let mut guides = Vec::new();
                let mut adj_x = 0.;
                let mut adj_y = 0.;
                if dx.abs() <= tol {
                    adj_x = dx;
                    guides.push(SnapGuide {
                        vertical: true,
                        from,
                        to: Point2::new(from.x + dx, from.y),
                        kind: best_kind,
                        span_is_x: best_span.0,
                        span_lo: best_span.1,
                        span_hi: best_span.2,
                    });
                }
                if dy.abs() <= tol {
                    adj_y = dy;
                    guides.push(SnapGuide {
                        vertical: false,
                        from,
                        to: Point2::new(from.x, from.y + dy),
                        kind: best_kind,
                        span_is_x: best_span.0,
                        span_lo: best_span.1,
                        span_hi: best_span.2,
                    });
                }
                (Point2::new(delta.x + adj_x, delta.y + adj_y), guides)
            }
            None => (delta, Vec::new()),
        }
    }
    // Recomputes the selection's dimension geometry (screen space). Called
    // once per frame before painting; lines and labels both read this.
    pub fn update_dim_geom(&mut self) {
        self.dim_geom = self
            .selection
            .and_then(|sel| self.doc.shape_bounds(sel))
            .filter(|_| self.pending_shape.is_none())
            .map(|b| {
                let tl = self.camera.unit_to_screen(b.origin);
                let br = self
                    .camera
                    .unit_to_screen(Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h));
                DimGeom {
                    x: (tl.x as f32).min(br.x as f32),
                    y: (tl.y as f32).min(br.y as f32),
                    w: (tl.x - br.x).abs() as f32,
                    h: (tl.y - br.y).abs() as f32,
                    ext: crate::ui::canvas::paint::extension_offset(self.camera.zoom),
                }
            });
    }

    // Updates the hover handle for cursor styling. Returns true on change.
    pub fn canvas_hover(&mut self, cursor: gpui::Point<gpui::Pixels>) -> bool {
        if self.tool != Tool::Move || self.dragging.is_some() || self.resizing.is_some() {
            return false;
        }
        let p = self.cursor_doc(cursor);
        let handle = self
            .selection
            .and_then(|sel| self.doc.shape_bounds(sel))
            .and_then(|b| handle_at(b, p, self.handle_tolerance()));
        if self.hover_handle != handle {
            self.hover_handle = handle;
            return true;
        }
        false
    }

    pub fn cursor_style(&self) -> gpui::CursorStyle {
        use gpui::CursorStyle;
        match self.hover_handle {
            Some(Handle::Nw) | Some(Handle::Se) => CursorStyle::ResizeUpLeftDownRight,
            Some(Handle::Ne) | Some(Handle::Sw) => CursorStyle::ResizeUpRightDownLeft,
            Some(Handle::N) | Some(Handle::S) => CursorStyle::ResizeUpDown,
            Some(Handle::E) | Some(Handle::W) => CursorStyle::ResizeLeftRight,
            None => CursorStyle::Arrow,
        }
    }

    pub fn canvas_up(&mut self, button: gpui::MouseButton) -> bool {
        if button == gpui::MouseButton::Middle && self.end_pan() {
            return true;
        }
        if button != gpui::MouseButton::Left {
            return false;
        }
        self.dragging = None;
        self.resizing = None;
        self.snap_guides.clear();
        let Some(pending) = self.pending_shape.take() else {
            return false;
        };
        // Ignore click-without-drag.
        if (pending.cursor.x - pending.start.x).abs() < 1e-9
            && (pending.cursor.y - pending.start.y).abs() < 1e-9
        {
            return true;
        }
        let bounds = pending.bounds();
        let layer_id = self.doc.layers[0].id;
        self.create_shape(layer_id, pending.kind, bounds.origin, {
            Point2::new(bounds.origin.x + bounds.size.w, bounds.origin.y + bounds.size.h)
        });
        true
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

    // True while no pan drag is in progress (used to gate hover tracking).
    pub fn pan_start_none(&self) -> bool {
        self.pan_start.is_none()
            && self.dragging.is_none()
            && self.resizing.is_none()
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
            shape_ids: Vec::new(),
        });
        id
    }

    // Creates a shape from two opposite corners in document units,
    // generating its point entities. Returns the shape handle.
    pub fn create_shape(
        &mut self,
        layer_id: u64,
        kind: ShapeKind,
        a: Point2,
        b: Point2,
    ) -> ShapeId {
        let pa = self.doc.add_point(a);
        let pb = self.doc.add_point(b);
        self.doc.add_shape(layer_id, kind, [pa, pb])
    }

    // Visible region in document units — used for culling before paint.
    pub fn visible_bounds(&self, size: Size) -> Rect {
        let min = self.camera.screen_to_unit(Point2::new(0., 0.));
        let max = self.camera.screen_to_unit(Point2::new(size.w, size.h));
        Rect::from_points(min, max)
    }
}

// Which resize handle (if any) is under a document-space point, given a
// tolerance in document units. Corners win over sides.
pub fn handle_at(b: Rect, p: Point2, tol: f64) -> Option<Handle> {
    let right = b.origin.x + b.size.w;
    let bottom = b.origin.y + b.size.h;
    let near_left = (p.x - b.origin.x).abs() <= tol;
    let near_right = (p.x - right).abs() <= tol;
    let near_top = (p.y - b.origin.y).abs() <= tol;
    let near_bottom = (p.y - bottom).abs() <= tol;
    let inside_x = p.x >= b.origin.x - tol && p.x <= right + tol;
    let inside_y = p.y >= b.origin.y - tol && p.y <= bottom + tol;

    if near_left && near_top {
        Some(Handle::Nw)
    } else if near_right && near_top {
        Some(Handle::Ne)
    } else if near_right && near_bottom {
        Some(Handle::Se)
    } else if near_left && near_bottom {
        Some(Handle::Sw)
    } else if near_top && inside_x {
        Some(Handle::N)
    } else if near_right && inside_y {
        Some(Handle::E)
    } else if near_bottom && inside_x {
        Some(Handle::S)
    } else if near_left && inside_y {
        Some(Handle::W)
    } else {
        None
    }
}


