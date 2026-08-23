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
    // White square with an accent border marking a corner resize handle.
    CornerHandle {
        center: Point<Pixels>,
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
    dim_geom: Option<crate::editor::DimGeom>,
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
        }
    }

    // Selection overlay drawn AFTER every shape fill so nothing can cover
    // the outline, dimension lines, or handles.
    if let Some(sel) = selection
        && let Some(unit) = doc.shape_bounds(sel)
    {
        let (x, y, w, h) = screen_rect(unit, camera);
        // Dimensions at the bottom of the overlay stack.
        if let Some(geom) = dim_geom {
            list.extend(dimension_prims(geom.x, geom.y, geom.w, geom.h, geom.ext));
        }
        // 2px selection outline.
        list.push(Primitive::Outline {
            bounds: Bounds {
                origin: Point { x: px(x - 1.), y: px(y - 1.) },
                size: Size { width: px(w + 2.), height: px(h + 2.) },
            },
        });
        // Corner resize handles on top of everything.
        for (hx, hy) in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)] {
            list.push(Primitive::CornerHandle {
                center: Point { x: px(hx), y: px(hy) },
            });
        }
    }

    // In-progress shape being dragged out (on top of fills).
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

// -- dimensions --

pub const EXTENSION_SCREEN_PX: f32 = 32.;

// Shared by the drawn lines AND the label overlay so they can never
// disagree. Scales with zoom (feels attached to the shape) but clamps so
// it stays readable at extremes.
pub fn extension_offset(zoom: f64) -> f32 {
    ((EXTENSION_SCREEN_PX as f64 * zoom) as f32).clamp(10., EXTENSION_SCREEN_PX)
}
const DASH: f32 = 6.;
const GAP: f32 = 4.;
const LINE_W: f32 = 2.;

fn dashed_h(list: &mut Vec<Primitive>, x0: f32, x1: f32, y: f32, color: gpui::Background) {
    let (a, b) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let mut t = a;
    while t < b {
        let end = (t + DASH).min(b);
        list.push(Primitive::Rect {
            bounds: Bounds {
                origin: Point { x: px(t), y: px(y - LINE_W / 2.) },
                size: Size { width: px(end - t), height: px(LINE_W) },
            },
            color,
        });
        t += DASH + GAP;
    }
}

fn dashed_v(list: &mut Vec<Primitive>, y0: f32, y1: f32, x: f32, color: gpui::Background) {
    let (a, b) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let mut t = a;
    while t < b {
        let end = (t + DASH).min(b);
        list.push(Primitive::Rect {
            bounds: Bounds {
                origin: Point { x: px(x - LINE_W / 2.), y: px(t) },
                size: Size { width: px(LINE_W), height: px(end - t) },
            },
            color,
        });
        t += DASH + GAP;
    }
}

// Witness stubs (perpendicular, offset by `ext` SCREEN pixels from the edge)
// plus the parallel dashed dimension line, for width (bottom) and height
// (right). The label itself is a DOM overlay added by the canvas view.
pub fn dimension_prims(x: f32, y: f32, w: f32, h: f32, ext: f32) -> Vec<Primitive> {
    let color: gpui::Background = rgb(crate::theme::ACCENT).into();
    let mut list = Vec::new();
    const OVERSHOOT: f32 = 3.;

    // Width: below the bottom edge. The parallel line spans EXACTLY
    // corner to corner — it never overshoots the witness stubs.
    let dim_y = y + h + ext;
    dashed_v(&mut list, y + h, dim_y, x, color);
    dashed_v(&mut list, y + h, dim_y, x + w, color);
    dashed_h(&mut list, x, x + w, dim_y, color);

    // Height: right of the right edge.
    let dim_x = x + w + ext;
    dashed_h(&mut list, x + w, dim_x, y, color);
    dashed_h(&mut list, x + w, dim_x, y + h, color);
    dashed_v(&mut list, y, y + h, dim_x, color);

    list
}

// Center point (in canvas-local screen coords) of each dimension label.
pub fn dimension_label_centers(x: f32, y: f32, w: f32, h: f32, ext: f32) -> [(f32, f32); 2] {
    [
        (x + w / 2., y + h + ext), // width
        (x + w + ext, y + h / 2.), // height
    ]
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







