use gpui::{Bounds, Path, PathBuilder, Pixels, Point, Size, px, rgb, rgba};

use crate::core::document::{Document, ShapeKind};
use crate::core::geometry::{Point2, Rect};
use crate::editor::Camera;
use crate::theme::Theme;

// Screen-space draw list built during prepaint (culled to the viewport),
// consumed by the paint callback.

pub enum Primitive {
    Rect {
        bounds: Bounds<Pixels>,
        color: gpui::Background,
    },
    Ellipse {
        center: Point<Pixels>,
        radii: Size<Pixels>,
        color: gpui::Background,
    },
    // 1px outline used for the selection indicator.
    Outline {
        bounds: Bounds<Pixels>,
    },
}

const ELLIPSE_KAPPA: f32 = 0.552_284_75;

pub fn build_draw_list(
    doc: &Document,
    camera: &Camera,
    viewport: Size<Pixels>,
    t: Theme,
    pending: Option<(ShapeKind, Rect)>,
    selection: Option<crate::core::ids::ShapeId>,
) -> Vec<Primitive> {
    let min = camera.screen_to_unit(Point2::new(0., 0.));
    let max = camera.screen_to_unit(Point2::new(
        f64::from(viewport.width),
        f64::from(viewport.height),
    ));
    let visible = Rect::from_points(min, max);

    // Shape fill: translucent ink derived from the secondary text color so
    // it reads against any background; selection styling comes later.
    let color: gpui::Background = rgba((t.text_secondary << 8) | 0x5A).into();
    let mut list = Vec::new();

    for layer in &doc.layers {
        for &sid in &layer.shape_ids {
            let Some(unit) = doc.shape_bounds(sid) else {
                continue;
            };
            if !overlaps(unit, visible) {
                continue;
            }
            let Some(kind) = doc.shape_kind(sid) else {
                continue;
            };
            if let Some(prim) = to_primitive(kind, unit, camera, color) {
                list.push(prim);
            }
            // Selection indicator: outline the shape's bounds.
            if Some(sid) == selection {
                let (x, y, w, h) = screen_rect(unit, camera);
                list.push(Primitive::Outline {
                    bounds: Bounds {
                        origin: Point { x: px(x - 1.), y: px(y - 1.) },
                        size: Size { width: px(w + 2.), height: px(h + 2.) },
                    },
                });
            }
        }
    }
    // In-progress shape being dragged out, drawn last (on top).
    if let Some((kind, unit)) = pending {
        if overlaps(unit, visible) {
            if let Some(prim) = to_primitive(kind, unit, camera, color) {
                list.push(prim);
            }
        }
    }
    list
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.origin.x <= b.origin.x + b.size.w
        && b.origin.x <= a.origin.x + a.size.w
        && a.origin.y <= b.origin.y + b.size.h
        && b.origin.y <= a.origin.y + a.size.h
}

fn screen_rect(unit: Rect, cam: &Camera) -> (f32, f32, f32, f32) {
    let tl = cam.unit_to_screen(unit.origin);
    let br = cam.unit_to_screen(Point2::new(
        unit.origin.x + unit.size.w,
        unit.origin.y + unit.size.h,
    ));
    (
        tl.x.min(br.x) as f32,
        tl.y.min(br.y) as f32,
        (tl.x - br.x).abs() as f32,
        (tl.y - br.y).abs() as f32,
    )
}

fn to_primitive(
    kind: ShapeKind,
    unit: Rect,
    cam: &Camera,
    color: gpui::Background,
) -> Option<Primitive> {
    let (x, y, w, h) = screen_rect(unit, cam);
    // Sub-pixel shapes aren't worth painting.
    if w < 0.5 || h < 0.5 {
        return None;
    }
    Some(match kind {
        ShapeKind::Rectangle => Primitive::Rect {
            bounds: Bounds {
                origin: Point { x: px(x), y: px(y) },
                size: Size { width: px(w), height: px(h) },
            },
            color,
        },
        ShapeKind::Ellipse => Primitive::Ellipse {
            center: Point { x: px(x + w / 2.), y: px(y + h / 2.) },
            radii: Size { width: px(w / 2.), height: px(h / 2.) },
            color,
        },
    })
}

// Four-cubic-bezier ellipse approximation (standard kappa constant).
pub fn ellipse_path(center: Point<Pixels>, radii: Size<Pixels>) -> Option<Path<Pixels>> {
    let kx = radii.width * ELLIPSE_KAPPA;
    let ky = radii.height * ELLIPSE_KAPPA;
    let c = center;
    let rx = radii.width;
    let ry = radii.height;

    let mut b = PathBuilder::fill();
    b.move_to(Point { x: c.x + rx, y: c.y });
    // NOTE: cubic_bezier_to takes (to, control_a, control_b).
    b.cubic_bezier_to(
        Point { x: c.x, y: c.y + ry },
        Point { x: c.x + rx, y: c.y + ky },
        Point { x: c.x + kx, y: c.y + ry },
    );
    b.cubic_bezier_to(
        Point { x: c.x - rx, y: c.y },
        Point { x: c.x - kx, y: c.y + ry },
        Point { x: c.x - rx, y: c.y + ky },
    );
    b.cubic_bezier_to(
        Point { x: c.x, y: c.y - ry },
        Point { x: c.x - rx, y: c.y - ky },
        Point { x: c.x - kx, y: c.y - ry },
    );
    b.cubic_bezier_to(
        Point { x: c.x + rx, y: c.y },
        Point { x: c.x + kx, y: c.y - ry },
        Point { x: c.x + rx, y: c.y - ky },
    );
    b.close();
    b.build().ok()
}


