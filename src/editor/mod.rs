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
    dragging: Option<SelectionDrag>,
    next_layer_id: u64,
    pan_start: Option<(gpui::Pixels, gpui::Pixels, Camera)>,
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
        Self {
            doc,
            camera: Camera::new(),
            tool: Tool::Move,
            pending_shape: None,
            selection: None,
            dragging: None,
            next_layer_id: 2,
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
                    let at = self.cursor_doc(cursor);
                    self.pending_shape = Some(PendingShape {
                        kind,
                        start: at,
                        cursor: at,
                        proportional: false,
                    });
                    true
                }
                Tool::Move => {
                    // Hit-test topmost shape under the cursor; start a drag
                    // if one is hit, otherwise deselect.
                    let p = self.cursor_doc(cursor);
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
                            changed || true
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
        if self.pan_delta(cursor) {
            return true;
        }
        if let Some(pending) = &mut self.pending_shape {
            pending.cursor = self.cursor_doc(cursor);
            pending.proportional = shift;
            return true;
        }
        if let Some(drag) = self.dragging {
            let p = self.cursor_doc(cursor);
            let Some(b) = self.doc.shape_bounds(drag.id) else {
                return false;
            };
            // Snap the grab offset to whole document units for stability.
            let target_x = (p.x - drag.grab_offset.x).round();
            let target_y = (p.y - drag.grab_offset.y).round();
            let delta = Point2::new(target_x - b.origin.x, target_y - b.origin.y);
            if delta.x == 0. && delta.y == 0. {
                return false;
            }
            return self.doc.translate_shape(drag.id, delta);
        }
        false
    }

    pub fn canvas_up(&mut self, button: gpui::MouseButton) -> bool {
        if button == gpui::MouseButton::Middle && self.end_pan() {
            return true;
        }
        if button != gpui::MouseButton::Left {
            return false;
        }
        self.dragging = None;
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
