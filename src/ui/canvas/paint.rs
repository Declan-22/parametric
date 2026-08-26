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
    // Constraint chip: bordered square with a tiny vector icon. Painted in
    // the canvas pass so it shares EXACT coordinates with geometry (DOM
    // overlays drift relative to painted content).
    Chip {
        x: f32,
        y: f32,
        size: f32,
        bg: Option<gpui::Hsla>,
        border: gpui::Hsla,
        icon: gpui::Hsla,
        // 0 = vertical, 1 = horizontal, 2 = coincident.
        kind: u8,
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
    pending_line: Option<(Point2, Point2)>,
    constraint_markers: &[crate::editor::dims::ConstraintMarker],
    pending_circle: Option<crate::editor::PendingCircle>,
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
                    if pts.len() < 3 {
                        continue;
                    }
                    // Cull on BOUNDS intersection, not corner containment:
                    // zoomed in, every corner can sit outside the viewport
                    // while the fill still covers the whole screen.
                    let mut bb: Option<Rect> = None;
                    for &p in &pts {
                        let r = Rect::from_points(p, p);
                        bb = Some(match bb {
                            Some(a) => a.union(&r),
                            None => r,
                        });
                    }
                    let Some(bb) = bb else { continue };
                    let intersects = visible.origin.x < bb.origin.x + bb.size.w
                        && bb.origin.x < visible.origin.x + visible.size.w
                        && visible.origin.y < bb.origin.y + bb.size.h
                        && bb.origin.y < visible.origin.y + visible.size.h;
                    if !intersects {
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
                    // Standalone stroked lines (line tool output).
                    if seg.kind == SegmentKind::Line
                        && seg.stroke_width > 0.
                        && let Some((a, b)) = doc.segment_geom(sid)
                        && (visible.contains(a) || visible.contains(b))
                    {
                        let (ax, ay) = scr(a);
                        let (bx, by) = scr(b);
                        list.push(Primitive::Line {
                            ax,
                            ay,
                            bx,
                            by,
                            width: seg.stroke_width as f32,
                            color,
                        });
                    }
                    // Arc segments: sampled polyline of the arc through
                    // start -> ctrl -> end. Incomplete arcs also show their
                    // dashed complementary portion.
                    if seg.kind == SegmentKind::Arc {
                        let Some(samples) = crate::editor::arc::segment_samples(doc, sid, 64)
                        else {
                            continue;
                        };
                        if samples.iter().any(|p| visible.contains(*p)) {
                            push_polyline(&mut list, &samples.iter().map(|p| scr(*p)).collect::<Vec<_>>(), 1.5, color);
                        }
                        // Dashed complement while incomplete — show whenever the
                        // arc itself is selected, or any of its defining
                        // points (including the center) are selected / being
                        // dragged (resizing).
                        let complete = crate::editor::arc::is_complete(doc, sid);
                        let arc_selected = selection.contains(&el)
                            || seg.ctrl.is_some_and(|c| {
                                selection.contains(&ElementRef::Point(c))
                                    || selection.contains(&ElementRef::Point(seg.start))
                                    || selection.contains(&ElementRef::Point(seg.end))
                            })
                            || seg.center.is_some_and(|c| selection.contains(&ElementRef::Point(c)));
                        if !complete
                            && arc_selected
                            && let Some(ctrl_id) = seg.ctrl
                            && let (Some(sa), Some(sb)) =
                                (doc.point(seg.start), doc.point(seg.end))
                            && let Some(cpos) = doc.point(ctrl_id)
                        {
                            let comp = crate::editor::arc::complement_samples(sa, sb, cpos, 48);
                            let pts: Vec<(f32, f32)> = comp.iter().map(|p| scr(*p)).collect();
                            dashed_polyline(&mut list, &pts, accent);
                        }
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

    // 2b) Constraint guide lines (distant H/V pairs), under everything.
    let guide_color: gpui::Background =
        rgba((t.accent << 8) | 0x66).into();
    for m in constraint_markers {
        if let Some(g) = m.guide {
            dashed_line(&mut list, g[0], g[1], g[2], g[3], guide_color);
        }
    }

    // 3) Snap feedback markers. During CREATION they mark the exact lock
    // location (cursor snapping) so it's obvious what you're joined to.
    let creating = pending.is_some() || pending_line.is_some();
    for g in snap_guides {
        list.push(Primitive::Circle {
            cx: g.to.x as f32,
            cy: g.to.y as f32,
            radius: 4.,
        });
    }
    let _ = creating;

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
    // Arc center handles — show whenever the arc is selected in any way
    // (segment itself, or any of its defining points including the center).
    {
        let selected_pids: std::collections::HashSet<_> = selection
            .iter()
            .flat_map(|el| doc.element_points(*el))
            .collect();
        for (sid, seg) in doc.all_segments() {
            if seg.kind != SegmentKind::Arc {
                continue;
            }
            let is_touched = selection.contains(&ElementRef::Segment(sid))
                || seg.ctrl.is_some_and(|c| selected_pids.contains(&c))
                || selected_pids.contains(&seg.start)
                || selected_pids.contains(&seg.end)
                || seg.center.is_some_and(|c| selected_pids.contains(&c));
            if !is_touched {
                continue;
            }
            // Prefer the stored center point (real document point) if present.
            let center_pos = seg
                .center
                .and_then(|id| doc.point(id))
                .or_else(|| {
                    let (Some(a), Some(b), Some(c)) = (
                        doc.point(seg.start),
                        doc.point(seg.end),
                        seg.ctrl.and_then(|id| doc.point(id)),
                    ) else {
                        return None;
                    };
                    crate::editor::arc::circumcircle(a, b, c).map(|(o, _)| o)
                });
            if let Some(center) = center_pos {
                let (cx0, cy0) = scr(center);
                list.push(Primitive::Circle { cx: cx0, cy: cy0, radius: 3. });
            }
        }
    }

    // 5b) Constraint chips — topmost interactive affordances.
    for m in constraint_markers {
        if !m.visible {
            continue;
        }
        const S: f32 = crate::ui::canvas::CHIP_SIZE;
        let faded_alpha: u32 = if m.hovered { 0xFF } else { 0x73 };
        let border: gpui::Hsla = if m.clicked {
            rgb(t.accent_border).into()
        } else {
            rgb(t.accent).into()
        };
        let icon: gpui::Hsla = if m.emphasized {
            rgb(0xFFFFFF).into()
        } else {
            rgba((t.accent << 8) | faded_alpha).into()
        };
        let bg = m
            .emphasized
            .then_some(rgb(t.accent))
            .map(gpui::Hsla::from);
        let kind = if m.coincident {
            2
        } else if m.vertical {
            0
        } else {
            1
        };
        list.push(Primitive::Chip {
            x: m.cx_out - S / 2.,
            y: m.cy_out - S / 2.,
            size: S,
            bg,
            border,
            icon,
            kind,
        });
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

    // In-progress line preview: accent stroke at the final width.
    if let Some((a, b)) = pending_line {
        let (ax, ay) = scr(a);
        let (bx, by) = scr(b);
        list.push(Primitive::Line { ax, ay, bx, by, width: 1., color: accent });
    }

    // In-progress circle preview, per stage:
    //  2 (a set, b not yet): chord A -> cursor like the Line tool;
    //  3 (a+b set): arc through a->cursor->b + dashed complement on the
    //  far side + dashed radius from the chord midpoint to the cursor.
    if let Some(pc) = pending_circle {
        match pc.stage() {
            2 => {
                if let Some(a) = pc.a {
                    // Chord preview to the ghost cursor (second point not yet placed).
                    push_chord_preview(&mut list, &scr, a, pc.cursor, accent);
                }
            }
            _ => {
                if let (Some(a), Some(b)) = (pc.a, pc.b) {
                    let arc = crate::editor::arc::samples_through(a, b, pc.cursor, 64);
                    let pts: Vec<(f32, f32)> = arc.iter().map(|p| scr(*p)).collect();
                    push_polyline(&mut list, &pts, 1.5, color);
                    let comp = crate::editor::arc::complement_samples(a, b, pc.cursor, 48);
                    dashed_polyline(&mut list, &comp.iter().map(|p| scr(*p)).collect::<Vec<_>>(), accent);
                    // Radius guide: true center -> cursor (visible handle).
                    if let Some((center, _)) = crate::editor::arc::circumcircle(a, b, pc.cursor) {
                        let (mx, my) = scr(center);
                        let (cx0, cy0) = scr(pc.cursor);
                        dashed_polyline(&mut list, &[(mx, my), (cx0, cy0)], accent);
                        list.push(Primitive::Circle { cx: mx, cy: my, radius: 3. });
                    }
                }
            }
        }
    }
    list
}

// Thin accent chord line with endpoint handles (circle tool stages 1-2).
fn push_chord_preview(
    list: &mut Vec<Primitive>,
    scr: &impl Fn(Point2) -> (f32, f32),
    a: Point2,
    b: Point2,
    accent: gpui::Background,
) {
    let (ax, ay) = scr(a);
    let (bx, by) = scr(b);
    list.push(Primitive::Line { ax, ay, bx, by, width: 1., color: accent });
    list.push(Primitive::Circle { cx: ax, cy: ay, radius: 3. });
    list.push(Primitive::Circle { cx: bx, cy: by, radius: 3. });
}

const LINE_W: f32 = 1.5;

// Solid polyline (arc rendering).
fn push_polyline(list: &mut Vec<Primitive>, pts: &[(f32, f32)], width: f32, color: gpui::Background) {
    for w in pts.windows(2) {
        list.push(Primitive::Line {
            ax: w[0].0,
            ay: w[0].1,
            bx: w[1].0,
            by: w[1].1,
            width,
            color,
        });
    }
}

// Dashed polyline (missing arc portion) — continuous dash pattern
// along the whole polyline (fewer dashes for shorter arcs, not smaller dashes).
fn dashed_polyline(list: &mut Vec<Primitive>, pts: &[(f32, f32)], color: gpui::Background) {
    const DASH: f32 = 6.;
    const GAP: f32 = 4.;
    const PERIOD: f32 = DASH + GAP;
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let dx = bx - ax;
        let dy = by - ay;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 {
            acc += len;
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let mut t = 0.0;
        while t < len {
            let phase = (acc + t) % PERIOD;
            let is_dash = phase < DASH;
            let remaining = if is_dash { DASH - phase } else { PERIOD - phase };
            let seg = remaining.min(len - t);
            if is_dash {
                list.push(Primitive::Line {
                    ax: ax + ux * t,
                    ay: ay + uy * t,
                    bx: ax + ux * (t + seg),
                    by: ay + uy * (t + seg),
                    width: 1.,
                    color,
                });
            }
            t += seg;
        }
        acc += len;
    }
}

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
            if let Some(seg) = doc.segment(sid)
                && seg.kind == SegmentKind::Arc
                && let Some(samples) = crate::editor::arc::segment_samples(doc, sid, 48)
            {
                let pts: Vec<(f32, f32)> = samples.iter().map(|p| scr(*p)).collect();
                push_polyline(list, &pts, 2.5, accent);
            } else if let Some((a, b)) = doc.segment_geom(sid) {
                let (ax, ay) = scr(a);
                let (bx, by) = scr(b);
                list.push(Primitive::Line { ax, ay, bx, by, width: 2.5, color: accent });
            }
        }        ElementRef::Fill(fid) => {
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
