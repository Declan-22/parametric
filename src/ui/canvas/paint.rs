use gpui::{Bounds, Pixels, Point, Size, px, rgb, rgba};

use crate::core::document::{Document, ShapeKind};
use crate::core::geometry::{Point2, Rect};
use crate::editor::{Camera, Handle};
use crate::theme::Theme;

// Screen-space draw list built during prepaint (culled to the viewport),
// consumed by the paint callback.

pub enum Primitive {
    Rect {
        bounds: Bounds<Pixels>,
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
    // Snap target marker: white circle, 1px accent outline.
    Circle {
        center: Point<Pixels>,
        radius: Pixels,
    },
}

const ELLIPSE_KAPPA: f32 = 0.552_284_75;

pub fn build_draw_list(
    doc: &Document,
    camera: &Camera,
    viewport: Size<Pixels>,
    t: Theme,
    pending: Option<(ShapeKind, Rect)>,
    selection: &[crate::core::ids::ShapeId],
    dim_geom: Option<crate::editor::DimGeom>,
    snap_guides: &[crate::editor::SnapGuide],
    hover: Option<(crate::core::ids::ShapeId, Option<crate::editor::Handle>)>,
    selected_handle: &[(crate::core::ids::ShapeId, crate::editor::Handle)],
    edge_dim: Option<(crate::editor::DimGeom, bool)>,
    marquee: Option<(crate::core::geometry::Point2, crate::core::geometry::Point2)>,
) -> Vec<Primitive> {
    let min = camera.screen_to_unit(Point2::new(0., 0.));
    let max = camera.screen_to_unit(Point2::new(
        f64::from(viewport.width),
        f64::from(viewport.height),
    ));
    let visible = Rect::from_points(min, max);

    // Default shape fill: neutral gray, fully opaque. Custom colors come
    // later; until then every new design starts here.
    let color: gpui::Background = rgb(0x808080).into();
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

    // Snap feedback: just the marker circle on edge snaps � no guide lines.
    for g in snap_guides {
        if g.kind == crate::editor::SnapKind::Edge {
            circle(
                &mut list,
                Point {
                    x: px(g.to.x as f32),
                    y: px(g.to.y as f32),
                },
                4.,
            );
        }
    }
    // Hover affordances: outline on interior hover, side bar, corner dot.
    // The independently selected edge/corner stays highlighted permanently.
    let mut handle_highlight = |list: &mut Vec<Primitive>,
                                doc: &Document,
                                camera: &Camera,
                                sid: crate::core::ids::ShapeId,
                                handle: crate::editor::Handle| {
        if let Some(unit) = doc.shape_bounds(sid) {
            let accent: gpui::Background = rgb(t.accent).into();
            let (x, y, w, h) = screen_rect(unit, camera);
            const BAR: f32 = 3.;
            match handle {
                Handle::N => list.push(Primitive::Rect {
                    bounds: Bounds {
                        origin: Point { x: px(x), y: px(y - BAR / 2.) },
                        size: Size { width: px(w), height: px(BAR) },
                    },
                    color: accent,
                }),
                Handle::S => list.push(Primitive::Rect {
                    bounds: Bounds {
                        origin: Point { x: px(x), y: px(y + h - BAR / 2.) },
                        size: Size { width: px(w), height: px(BAR) },
                    },
                    color: accent,
                }),
                Handle::W => list.push(Primitive::Rect {
                    bounds: Bounds {
                        origin: Point { x: px(x - BAR / 2.), y: px(y) },
                        size: Size { width: px(BAR), height: px(h) },
                    },
                    color: accent,
                }),
                Handle::E => list.push(Primitive::Rect {
                    bounds: Bounds {
                        origin: Point { x: px(x + w - BAR / 2.), y: px(y) },
                        size: Size { width: px(BAR), height: px(h) },
                    },
                    color: accent,
                }),
                corner => {
                    let (hx, hy) = match corner {
                        Handle::Nw => (x, y),
                        Handle::Ne => (x + w, y),
                        Handle::Se => (x + w, y + h),
                        _ => (x, y + h),
                    };
                    circle(list, Point { x: px(hx), y: px(hy) }, 4.);
                }
            }
        }
    };

    if let Some((sid, handle)) = hover
        && !selection.contains(&sid)
    {
        match handle {
            Some(hd) => handle_highlight(&mut list, doc, camera, sid, hd),
            None => {
                if let Some(unit) = doc.shape_bounds(sid) {
                    let (x, y, w, h) = screen_rect(unit, camera);
                    list.push(Primitive::Outline {
                        bounds: Bounds {
                            origin: Point { x: px(x), y: px(y) },
                            size: Size { width: px(w), height: px(h) },
                        },
                    });
                }
            }
        }
    }

    // Persistent highlight for the selected edge/corner — only meaningful
    // while its shape is NOT fully selected (a selected shape shows the
    // full handle set instead).
    for (sid, handle) in selected_handle {
        if selection.contains(sid) {
            continue;
        }
        handle_highlight(&mut list, doc, camera, *sid, *handle);
    }

    // Dimension lines render whenever there's an active dimension source
    // (selection, pending shape, or hover-resize) — independent of whether
    // the shape is selected.
    if let Some(geom) = dim_geom {
        let dim_color: gpui::Background = rgb(t.accent).into();
        list.extend(dimension_prims(geom.x, geom.y, geom.w, geom.h, geom.ext, dim_color));
    }
    // Single-axis dimension while edge-resizing.
    if let Some((geom, is_width)) = edge_dim {
        let accent: gpui::Background = rgb(t.accent).into();
        let dim_y = geom.y + geom.h + geom.ext;
        let dim_x = geom.x + geom.w + geom.ext;
        if is_width {
            dashed_v(&mut list, geom.y + geom.h, dim_y, geom.x, accent);
            dashed_v(&mut list, geom.y + geom.h, dim_y, geom.x + geom.w, accent);
            dashed_h(&mut list, geom.x, geom.x + geom.w, dim_y, accent);
        } else {
            dashed_h(&mut list, geom.x + geom.w, dim_x, geom.y, accent);
            dashed_h(&mut list, geom.x + geom.w, dim_x, geom.y + geom.h, accent);
            dashed_v(&mut list, geom.y, geom.y + geom.h, dim_x, accent);
        }
    }

    // Selection overlay drawn AFTER every shape fill so nothing can cover
    // the outline or handles. One outline per fully-selected shape.
    for &sel in selection {
        let Some(unit) = doc.shape_bounds(sel) else {
            continue;
        };
        let (x, y, w, h) = screen_rect(unit, camera);
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

    // Marquee band: low-opacity accent fill + 2px accent border.
    if let Some((a, b)) = marquee {
        let band = Rect::from_points(a, b);
        let (x, y, w, h) = screen_rect(band, camera);
        list.push(Primitive::Rect {
            bounds: Bounds {
                origin: Point { x: px(x), y: px(y) },
                size: Size { width: px(w), height: px(h) },
            },
            color: rgba((t.accent << 8) | 0x1A).into(),
        });
        list.push(Primitive::Outline {
            bounds: Bounds {
                origin: Point { x: px(x), y: px(y) },
                size: Size { width: px(w), height: px(h) },
            },
        });
    }

    // In-progress shape being dragged out (on top of fills), plus an
    // accent crosshair pinned to the anchor corner.
    if let Some((kind, unit)) = pending {
        if overlaps(unit, visible) {
            if let Some(prim) = to_primitive(kind, unit, camera, color) {
                list.push(prim);
            }
        }
        let s = camera.unit_to_screen(unit.origin);
        let (sx, sy) = (s.x as f32, s.y as f32);
        const ARM: f32 = 4.;
        list.push(Primitive::Rect {
            bounds: Bounds {
                origin: Point { x: px(sx - ARM), y: px(sy) },
                size: Size { width: px(ARM * 2.), height: px(1.) },
            },
            color: rgb(t.accent).into(),
        });
        list.push(Primitive::Rect {
            bounds: Bounds {
                origin: Point { x: px(sx), y: px(sy - ARM) },
                size: Size { width: px(1.), height: px(ARM * 2.) },
            },
            color: rgb(t.accent).into(),
        });
    }
    list
}

// White-filled circle with a 1px accent outline (snap target marker).
fn circle(list: &mut Vec<Primitive>, center: Point<Pixels>, r: f32) {
    list.push(Primitive::Circle {
        center,
        radius: px(r),
    });
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
const LINE_W: f32 = 1.;

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
pub fn dimension_prims(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    ext: f32,
    color: gpui::Background,
) -> Vec<Primitive> {
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

