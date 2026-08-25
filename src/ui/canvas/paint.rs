use gpui::{Pixels, Size, rgb, rgba};

use crate::core::constraints::ElementRef;
use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::{Point2, Rect};
use crate::editor::dims::DimRender;
use crate::editor::ruler;
use crate::editor::{Camera, SnapGuide};
use crate::theme::Theme;

// Screen-space draw list built during prepaint (culled to the viewport),
// consumed by the paint callback. Coordinates are plain f32 canvas-local
// pixels; the paint callback converts to gpui types.

pub enum Primitive {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: gpui::Background,
    },
    // Arbitrary filled polygon (fill loops are general polygons once their
    // points move independently).
    Polygon {
        points: Vec<(f32, f32)>,
        color: gpui::Background,
    },
    // Straight stroke of arbitrary angle.
    Line {
        ax: f32,
        ay: f32,
        bx: f32,
        by: f32,
        width: f32,
        color: gpui::Background,
    },
    // 1px outline used for selection indicators.
    Outline {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    // White circle marking an editable/snapped point.
    Circle {
        cx: f32,
        cy: f32,
        radius: f32,
    },
    // Real vector text painted into the canvas (ruler markings etc.) —
    // no DOM overlay containers. Two rows: pixels nearest the dash, inches
    // below; value in ink, unit suffix in empty_text_primary.
    RulerLabel {
        center_x: f32,
        anchor_y: f32,
        px_value: String,
        in_value: String,
    },
}

pub fn build_draw_list(
    doc: &Document,
    camera: &Camera,
    viewport: Size<Pixels>,
    t: Theme,
    pending: Option<Rect>,
    selection: &[ElementRef],
    hover: Option<ElementRef>,
    dim_renders: &[DimRender],
    snap_guides: &[SnapGuide],
    marquee: Option<(Point2, Point2)>,
    pending_ruler: Option<(Point2, Point2)>,
) -> Vec<Primitive> {
    let min = camera.screen_to_unit(Point2::new(0., 0.));
    let max = camera.screen_to_unit(Point2::new(
        f64::from(viewport.width),
        f64::from(viewport.height),
    ));
    let visible = Rect::from_points(min, max);

    // Default fill: neutral gray, fully opaque.
    let color: gpui::Background = rgb(0x808080).into();
    let accent: gpui::Background = rgb(t.accent).into();
    let mut list = Vec::new();

    let scr = |p: Point2| {
        let s = camera.unit_to_screen(p);
        (s.x as f32, s.y as f32)
    };

    // 1) Fills, then rulers (always-visible procedural components). Bare
    // line segments are invisible geometry; they only appear via
    // hover/selection overlays below.
    for layer in &doc.layers {
        for &el in &layer.elements {
            match el {
                ElementRef::Fill(fid) => {
                    let Some(pts) = crate::editor::pick::loop_points(doc, fid) else {
                        continue;
                    };
                    if pts.len() < 3 || !pts.iter().any(|p| visible.contains(*p)) {
                        continue;
                    }
                    list.push(Primitive::Polygon {
                        points: pts.iter().map(|&p| scr(p)).collect(),
                        color,
                    });
                }
                ElementRef::Segment(sid) => {
                    let Some(seg) = doc.segment(sid) else { continue };
                    if seg.kind == SegmentKind::Ruler
                        && let Some((a, b)) = doc.segment_geom(sid)
                        && (visible.contains(a) || visible.contains(b))
                    {
                        push_ruler(&mut list, a, b, camera, t);
                    }
                }
                _ => {}
            }
        }
    }

    // 2) Dimension lines: extension stubs + parallel dashed dim line,
    // any angle. Drawn UNDER points/selection so corner dots always sit
    // on top.
    for d in dim_renders {
        dashed_line(&mut list, d.ax, d.ay, d.lax, d.lay, accent);
        dashed_line(&mut list, d.bx, d.by, d.lbx, d.lby, accent);
        dashed_line(&mut list, d.lax, d.lay, d.lbx, d.lby, accent);
        for e in &d.extra_ext {
            dashed_line(&mut list, e[0], e[1], e[2], e[3], accent);
        }
    }

    // 3) Snap feedback markers.
    for g in snap_guides {
        list.push(Primitive::Circle {
            cx: g.to.x as f32,
            cy: g.to.y as f32,
            radius: 4.,
        });
    }

    // 4) Hover affordance: accent outline of the hovered element.
    if let Some(h) = hover
        && !selection.contains(&h)
    {
        element_outline(doc, h, &scr, accent, &mut list);
    }

    // 5) Selection highlights + point handles drawn after everything —
    // points are the topmost affordance in the entire stack.
    for &sel in selection {
        element_outline(doc, sel, &scr, accent, &mut list);
    }
    for &sel in selection {
        for pid in doc.element_points(sel) {
            if let Some(p) = doc.point(pid) {
                let (x, y) = scr(p);
                list.push(Primitive::Circle { cx: x, cy: y, radius: 4. });
            }
        }
    }

    // 6) Marquee band: low-opacity accent fill + 1px accent border.
    if let Some((a, b)) = marquee {
        let band = Rect::from_points(a, b);
        let (x, y, w, h) = screen_rect(band, camera);
        list.push(Primitive::Rect {
            x,
            y,
            w,
            h,
            color: rgba((t.accent << 8) | 0x1A).into(),
        });
        list.push(Primitive::Outline { x, y, w, h });
    }

    // 7) In-progress rectangle being dragged out + anchor crosshair.
    if let Some(unit) = pending {
        if overlaps(unit, visible) {
            let (x, y, w, h) = screen_rect(unit, camera);
            if w > 0.5 && h > 0.5 {
                list.push(Primitive::Rect { x, y, w, h, color });
            }
        }
        let (sx, sy) = scr(unit.origin);
        const ARM: f32 = 4.;
        list.push(Primitive::Rect {
            x: sx - ARM,
            y: sy,
            w: ARM * 2.,
            h: 1.,
            color: accent,
        });
        list.push(Primitive::Rect {
            x: sx,
            y: sy - ARM,
            w: 1.,
            h: ARM * 2.,
            color: accent,
        });
    }

    // In-progress ruler preview with full tick rendering.
    if let Some((a, b)) = pending_ruler {
        push_ruler(&mut list, a, b, camera, t);
    }
    list
}

const LINE_W: f32 = 1.5;

// Accent outline overlay for one element.
fn element_outline(
    doc: &Document,
    el: ElementRef,
    scr: &impl Fn(Point2) -> (f32, f32),
    accent: gpui::Background,
    list: &mut Vec<Primitive>,
) {
    match el {
        // Points use the SAME styling everywhere: one clean small dot.
        ElementRef::Point(pid) => {
            if let Some(p) = doc.point(pid) {
                let (x, y) = scr(p);
                list.push(Primitive::Circle { cx: x, cy: y, radius: 4. });
            }
        }
        ElementRef::Segment(sid) => {
            if let Some((a, b)) = doc.segment_geom(sid) {
                let (ax, ay) = scr(a);
                let (bx, by) = scr(b);
                list.push(Primitive::Line { ax, ay, bx, by, width: 2.5, color: accent });
            }
        }
        ElementRef::Fill(fid) => {
            if let Some(pts) = crate::editor::pick::loop_points(doc, fid) {
                for i in 0..pts.len() {
                    let (ax, ay) = scr(pts[i]);
                    let (bx, by) = scr(pts[(i + 1) % pts.len()]);
                    list.push(Primitive::Line { ax, ay, bx, by, width: 2., color: accent });
                }
            }
        }
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.origin.x <= b.origin.x + b.size.w
        && b.origin.x <= a.origin.x + a.size.w
        && a.origin.y <= b.origin.y + b.size.h
        && b.origin.y <= a.origin.y + a.size.h
}

fn screen_rect(unit: Rect, cam: &Camera) -> (f32, f32, f32, f32) {
    let tl = cam.unit_to_screen(unit.origin);
    let br =
        cam.unit_to_screen(Point2::new(unit.origin.x + unit.size.w, unit.origin.y + unit.size.h));
    (
        tl.x.min(br.x) as f32,
        tl.y.min(br.y) as f32,
        (tl.x - br.x).abs() as f32,
        (tl.y - br.y).abs() as f32,
    )
}

// Procedural ruler along segment a->b, driven entirely by the shared
// editor::ruler module: baseline + perpendicular ticks + real vector text
// labels at each inch mark. No DOM overlays involved.
fn push_ruler(list: &mut Vec<Primitive>, a: Point2, b: Point2, cam: &Camera, t: Theme) {
    let ink: gpui::Background = rgb(t.text_secondary).into();

    // Baseline sits exactly on the stored segment.
    let (ax, ay) = ruler::to_screen(cam, a);
    let (bx, by) = ruler::to_screen(cam, b);
    list.push(Primitive::Line { ax, ay, bx, by, width: 1., color: ink });

    for tick in ruler::ticks(a, b) {
        let (x0, y0) = ruler::to_screen(cam, tick.base);
        let (x1, y1) = ruler::to_screen(cam, tick.tip);
        list.push(Primitive::Line {
            ax: x0,
            ay: y0,
            bx: x1,
            by: y1,
            width: if tick.inch_mark { 1.5 } else { 1. },
            color: ink,
        });
    }

    for (pos, px_n, in_n) in ruler::labels(a, b) {
        let (x, y) = ruler::to_screen(cam, pos);
        list.push(Primitive::RulerLabel {
            center_x: x,
            anchor_y: y,
            px_value: format!("{px_n}"),
            in_value: format!("{in_n}"),
        });
    }
}// Dashed straight line between two screen points, any angle.
fn dashed_line(list: &mut Vec<Primitive>, ax: f32, ay: f32, bx: f32, by: f32, color: gpui::Background) {    const DASH: f32 = 6.;
    const GAP: f32 = 4.;
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return;
    }
    let ux = dx / len;
    let uy = dy / len;
    let mut t = 0.;
    while t < len {
        let end = (t + DASH).min(len);
        list.push(Primitive::Line {
            ax: ax + ux * t,
            ay: ay + uy * t,
            bx: ax + ux * end,
            by: ay + uy * end,
            width: 1.,
            color,
        });
        t += DASH + GAP;
    }
}
