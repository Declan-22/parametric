use gpui::{Pixels, Size, rgb, rgba};
use std::collections::HashMap;

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
    angle_dim_renders: &[crate::editor::dims::AngleDimRender],
    snap_guides: &[SnapGuide],
    marquee: Option<(Point2, Point2)>,
    pending_ruler: Option<(Point2, Point2)>,
    pending_line: Option<(Point2, Point2)>,
    constraint_markers: &[crate::editor::dims::ConstraintMarker],
    pending_circle: Option<crate::editor::PendingCircle>,
    show_grid: bool,
    tool: crate::editor::Tool,
    cursor_doc: Option<Point2>,
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
    // Arc tessellation is reused by the base pass and selection overlays.
    // Keep it frame-local for now; the next step can promote this to a
    // revision-keyed cache owned by the editor.
    let mut arc_cache: HashMap<crate::core::ids::SegmentId, Vec<Point2>> = HashMap::new();

    // 0) Infinite grid — viewport-culled, LOD-clamped, pan-aware. This is the
    // "genius" part: cost is O(viewport) not O(world). We never allocate
    // world-sized geometry; we recompute the handful of lines intersecting the
    // visible doc rect each frame, and we double the step when zoomed out so
    // the primitive count stays bounded (~viewport/min_spacing). Pan just shifts
    // which lines are emitted — the grid is document-anchored, not screen-locked.
    push_grid(&mut list, camera, viewport, visible, t, show_grid);

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
                    let Some(seg) = doc.segment(sid) else {
                        continue;
                    };
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
                    // start -> ctrl -> end (adaptive so the curve stays
                    // smooth at any zoom). Incomplete arcs also show their
                    // dashed complementary portion.
                    if seg.kind == SegmentKind::Arc {
                        let Some(sc) = seg.ctrl else { continue };
                        let (Some(sa), Some(sb), Some(scp)) =
                            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
                        else {
                            continue;
                        };
                        let n = crate::editor::arc::adaptive_samples(sa, sb, scp, camera.zoom);
                        let Some(samples) = cached_arc_samples(
                            doc, sid, camera.zoom, &mut arc_cache,
                        ) else {
                            continue;
                        };
                        if samples.iter().any(|p| visible.contains(*p)) {
                            push_polyline(
                                &mut list,
                                &samples.iter().map(|p| scr(*p)).collect::<Vec<_>>(),
                                1.5,
                                color,
                            );
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
                            || seg
                                .center
                                .is_some_and(|c| selection.contains(&ElementRef::Point(c)));
                        if !complete
                            && arc_selected
                            && seg.ctrl.is_some_and(|c| {
                                selection.contains(&ElementRef::Point(c))
                                    || selection.contains(&ElementRef::Point(seg.start))
                                    || selection.contains(&ElementRef::Point(seg.end))
                            })
                        {
                            let comp =
                                crate::editor::arc::complement_samples(sa, sb, scp, n.max(32));
                            let pts: Vec<(f32, f32)> = comp.iter().map(|p| scr(*p)).collect();
                            dashed_polyline(&mut list, &pts, accent);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // 1b) Arc CENTER reveal: when the cursor sits inside an arc's full
    // circle (the disk, not just near the curve), draw its center — the
    // middle of the whole sweep's circle — so it is discoverable without
    // pixel-hunting hover. (Not the on-curve third point; the centerpoint.)
    if let Some(cur) = cursor_doc {
        for (_sid, seg) in doc.all_segments() {
            if seg.kind != SegmentKind::Arc {
                continue;
            }
            let Some(sc) = seg.ctrl else { continue };
            let (Some(sa), Some(sb), Some(scp)) =
                (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
            else {
                continue;
            };
            let Some((center, r)) = crate::editor::arc::circumcircle(sa, sb, scp) else {
                continue;
            };
            if r < 1e-9 {
                continue;
            }
            let dx = cur.x - center.x;
            let dy = cur.y - center.y;
            if (dx * dx + dy * dy).sqrt() > r {
                continue;
            }
            // Prefer the stored center point; fall back to the computed one.
            let c = seg.center.and_then(|id| doc.point(id)).unwrap_or(center);
            if !visible.contains(c) {
                continue;
            }
            let (mx, my) = scr(c);
            list.push(Primitive::Circle {
                cx: mx,
                cy: my,
                radius: 4.,
            });
        }
    }

    // 2) Dimension lines: extension stubs + parallel dashed dim line,
    // any angle. Drawn UNDER points/selection so corner dots always sit
    // on top. Tool-created dimension constraints render in the muted
    // constraint ink; transient measurement previews stay accent.
    for d in dim_renders {
        // Hover lifts the whole dimension (lines included) to
        // text_secondary; constraint dims idle in the muted ink.
        let ink = if d.constraint {
            if d.hovered {
                rgb(t.text_secondary).into()
            } else {
                rgb(t.empty_text_secondary).into()
            }
        } else {
            accent
        };
        dashed_line(&mut list, d.ax, d.ay, d.lax, d.lay, 1., ink);
        dashed_line(&mut list, d.bx, d.by, d.lbx, d.lby, 1., ink);
        dashed_line(&mut list, d.lax, d.lay, d.lbx, d.lby, 1., ink);
        // Arrowheads at both ends of the dim container line.
        dim_arrowhead(&mut list, d.lbx, d.lby, d.lax, d.lay, ink);
        dim_arrowhead(&mut list, d.lax, d.lay, d.lbx, d.lby, ink);
        for e in &d.extra_ext {
            dashed_line(&mut list, e[0], e[1], e[2], e[3], 1., ink);
        }
    }

    // 2a) Angle dimensions: a dashed arc between the two lines, with the
    // value container riding on it (label painted by the DOM layer).
    for a in angle_dim_renders {
        let ink = if a.constraint {
            rgb(t.empty_text_secondary).into()
        } else {
            accent
        };
        const N: usize = 48;
        let mut pts = Vec::with_capacity(N + 1);
        for k in 0..=N {
            let th = a.a0 + a.sweep * (k as f32 / N as f32);
            pts.push((a.cx + a.r * th.cos(), a.cy + a.r * th.sin()));
        }
        dashed_polyline(&mut list, &pts, ink);
    }

    // 2b) Constraint guide lines (distant pairs), under everything. A guide
    // belongs to its chip: hidden chips must not leave unexplained dashes.
    let guide_color: gpui::Background = rgb(t.empty_text_secondary).into();
    for m in constraint_markers {
        if m.visible && let Some(g) = m.guide {
            dashed_line(&mut list, g[0], g[1], g[2], g[3], 1., guide_color);
        }
    }

    // 3) Snap feedback: a 2px dashed accent CONNECTION LINE between the two
    // snapping pieces — the feature (guide.from) and the snapped point
    // (guide.to) — spanning their full distance, for EVERY snap, creation
    // or drag. Fusion-style alignment lines. During creation the feature
    // marker itself is the DOM snap-cursor (crosshair + accent square);
    // drags keep the classic dot markers on the feature.
    let is_creation = matches!(
        tool,
        crate::editor::Tool::Rectangle
            | crate::editor::Tool::Line
            | crate::editor::Tool::Ruler
            | crate::editor::Tool::Circle
    );
    for g in snap_guides {
        let from = camera.unit_to_screen(g.from);
        let to = camera.unit_to_screen(g.to);
        // Linked features are already joined by the shape's own geometry —
        // the dashed stub would double-draw an existing edge. Edge-body hits
        // (no real point anchor) never earn a stub either. Sub-4px stubs are
        // cursor jitter, not alignment information — skip them so guides
        // stop popping in and out for no apparent reason.
        if !g.linked {
            let dx = to.x - from.x;
            let dy = to.y - from.y;
            if (dx * dx + dy * dy).sqrt() >= 4.0 {
                dashed_line(
                    &mut list,
                    from.x as f32,
                    from.y as f32,
                    to.x as f32,
                    to.y as f32,
                    2.,
                    accent,
                );
            }
        }
        if !is_creation {
            list.push(Primitive::Circle {
                cx: from.x as f32,
                cy: from.y as f32,
                radius: 4.,
            });
            // Snap badge: accent square outline around the snapped point,
            // ONLY for solid feature locks (both axes onto one feature, or
            // a grid crossing). One-axis alignments draw their connection
            // line but must NOT claim "100% snapped".
            if g.solid {
                const SQUARE: f32 = 12.0;
                list.push(Primitive::Outline {
                    x: to.x as f32 - SQUARE / 2.0,
                    y: to.y as f32 - SQUARE / 2.0,
                    w: SQUARE,
                    h: SQUARE,
                });
            }
        }
    }

    // 4) Hover affordance: accent outline of the hovered element.
    if let Some(h) = hover
        && !selection.contains(&h)
    {
        element_outline(doc, h, &scr, accent, &mut list, camera.zoom, &mut arc_cache);
    }

    // 5) Selection highlights + point handles drawn after everything —
    // points are the topmost affordance in the entire stack.
    for &sel in selection {
        element_outline(doc, sel, &scr, accent, &mut list, camera.zoom, &mut arc_cache);
    }
    for &sel in selection {
        for pid in doc.element_points(sel) {
            if let Some(p) = doc.point(pid) {
                let (x, y) = scr(p);
                list.push(Primitive::Circle {
                    cx: x,
                    cy: y,
                    radius: 4.,
                });
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
            let center_pos = seg.center.and_then(|id| doc.point(id)).or_else(|| {
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
                list.push(Primitive::Circle {
                    cx: cx0,
                    cy: cy0,
                    radius: 3.,
                });
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

    // In-progress line preview: accent stroke at the final width.
    if let Some((a, b)) = pending_line {
        let (ax, ay) = scr(a);
        let (bx, by) = scr(b);
        list.push(Primitive::Line {
            ax,
            ay,
            bx,
            by,
            width: 1.,
            color: accent,
        });
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
                    let n = crate::editor::arc::adaptive_samples(a, b, pc.cursor, camera.zoom);
                    let arc = crate::editor::arc::samples_through(a, b, pc.cursor, n);
                    let pts: Vec<(f32, f32)> = arc.iter().map(|p| scr(*p)).collect();
                    push_polyline(&mut list, &pts, 1.5, color);
                    let comp = crate::editor::arc::complement_samples(a, b, pc.cursor, n.max(32));
                    dashed_polyline(
                        &mut list,
                        &comp.iter().map(|p| scr(*p)).collect::<Vec<_>>(),
                        accent,
                    );
                    // Radius guide: true center -> cursor (visible handle).
                    if let Some((center, _)) = crate::editor::arc::circumcircle(a, b, pc.cursor) {
                        let (mx, my) = scr(center);
                        let (cx0, cy0) = scr(pc.cursor);
                        dashed_polyline(&mut list, &[(mx, my), (cx0, cy0)], accent);
                        list.push(Primitive::Circle {
                            cx: mx,
                            cy: my,
                            radius: 3.,
                        });
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
    list.push(Primitive::Line {
        ax,
        ay,
        bx,
        by,
        width: 1.,
        color: accent,
    });
    list.push(Primitive::Circle {
        cx: ax,
        cy: ay,
        radius: 3.,
    });
    list.push(Primitive::Circle {
        cx: bx,
        cy: by,
        radius: 3.,
    });
}

const LINE_W: f32 = 1.5;

// Solid polyline (arc rendering).
fn push_polyline(
    list: &mut Vec<Primitive>,
    pts: &[(f32, f32)],
    width: f32,
    color: gpui::Background,
) {
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
            let remaining = if is_dash {
                DASH - phase
            } else {
                PERIOD - phase
            };
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
    zoom: f64,
    arc_cache: &mut HashMap<crate::core::ids::SegmentId, Vec<Point2>>,
) {
    match el {
        // Points use the SAME styling everywhere: one clean small dot.
        ElementRef::Point(pid) => {
            if let Some(p) = doc.point(pid) {
                let (x, y) = scr(p);
                list.push(Primitive::Circle {
                    cx: x,
                    cy: y,
                    radius: 4.,
                });
            }
        }
        ElementRef::Segment(sid) => {
            if let Some(seg) = doc.segment(sid)
                && seg.kind == SegmentKind::Arc
                && let Some(samples) = cached_arc_samples(doc, sid, zoom, arc_cache)
            {
                let pts: Vec<(f32, f32)> = samples.iter().map(|p| scr(*p)).collect();
                push_polyline(list, &pts, 2.5, accent);
            } else if let Some((a, b)) = doc.segment_geom(sid) {
                let (ax, ay) = scr(a);
                let (bx, by) = scr(b);
                list.push(Primitive::Line {
                    ax,
                    ay,
                    bx,
                    by,
                    width: 2.5,
                    color: accent,
                });
            }
        }
        ElementRef::Fill(fid) => {
            if let Some(pts) = crate::editor::pick::loop_points(doc, fid) {
                for i in 0..pts.len() {
                    let (ax, ay) = scr(pts[i]);
                    let (bx, by) = scr(pts[(i + 1) % pts.len()]);
                    list.push(Primitive::Line {
                        ax,
                        ay,
                        bx,
                        by,
                        width: 2.,
                        color: accent,
                    });
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

// Procedural ruler along segment a->b, driven entirely by the shared
// editor::ruler module: baseline + perpendicular ticks + real vector text
// labels at each inch mark. No DOM overlays involved.
fn push_ruler(list: &mut Vec<Primitive>, a: Point2, b: Point2, cam: &Camera, t: Theme) {
    let ink: gpui::Background = rgb(t.text_secondary).into();

    // Baseline sits exactly on the stored segment.
    let (ax, ay) = ruler::to_screen(cam, a);
    let (bx, by) = ruler::to_screen(cam, b);
    list.push(Primitive::Line {
        ax,
        ay,
        bx,
        by,
        width: 1.,
        color: ink,
    });

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
} // Infinite document-anchored grid. Optimized to feel free:
//  - Screen-space tiling: offset = -(pan*zoom) % step_screen, so pan slides
//    the lattice (different parts become visible) instead of a static overlay.
//  - Viewport-culled: only emits lines whose screen coord lands in [0, viewport].
//  - LOD clamp: the 5x-step level selection lives in editor::grid so the
//    drawn lattice and the snap lattice (grid::snap_step) can never drift.
//  - O(viewport) primitive count: at most ~viewport/MIN_PX per axis.
//  - Pixel-snapped 1px quads (cheapest GPU primitive), no allocation beyond vec.
fn push_grid(
    list: &mut Vec<Primitive>,
    camera: &Camera,
    viewport: Size<Pixels>,
    _visible: Rect,
    t: Theme,
    show_grid: bool,
) {
    if !show_grid {
        return;
    }
    if camera.zoom < 1e-9 {
        return;
    }
    let vw = f64::from(viewport.width);
    let vh = f64::from(viewport.height);
    if vw < 1. || vh < 1. {
        return;
    }

    // Hierarchical 5x5 grid like Fusion360: big squares contain 5x5
    // smaller squares, and as you zoom in each 5x5 cell subdivides into
    // another 5x5. Grid is document-anchored (multiples of grid::GRID_BASE)
    // so an object placed on an intersection stays on that intersection
    // when you zoom — no popping to the middle of a cell. The level
    // selection here is THE SAME computation snapping uses.
    let lv = crate::editor::grid::levels(camera.zoom);
    let minor_step = lv.minor;
    let minor_screen = minor_step * camera.zoom;
    let major_step = minor_step * 5.0;
    let major_screen = minor_screen * 5.0;
    let finer_step = minor_step / 5.0;
    let finer_screen = minor_screen / 5.0;

    let vw_f = vw as f32;
    let vh_f = vh as f32;

    // Helper to draw a grid level with given step and color
    let mut draw_level = |step: f64, screen: f64, color: gpui::Background| {
        if step < 1e-6 || screen < 4.0 {
            return;
        }
        // Don't draw if it would be too dense (thousands of lines) or too sparse
        if screen < 8.0 || screen > 400.0 {
            return;
        }
        let off_x = (-camera.pan.x * camera.zoom).rem_euclid(screen);
        let off_y = (-camera.pan.y * camera.zoom).rem_euclid(screen);
        let mut x = off_x;
        let mut count = 0usize;
        while x <= vw + 1e-6 && count < 4096 {
            let sx = x.floor() as f32;
            if sx >= 0.0 && sx < vw_f {
                list.push(Primitive::Rect {
                    x: sx,
                    y: 0.0,
                    w: 1.0,
                    h: vh_f,
                    color,
                });
            }
            x += screen;
            count += 1;
        }
        let mut y = off_y;
        count = 0;
        while y <= vh + 1e-6 && count < 4096 {
            let sy = y.floor() as f32;
            if sy >= 0.0 && sy < vh_f {
                list.push(Primitive::Rect {
                    x: 0.0,
                    y: sy,
                    w: vw_f,
                    h: 1.0,
                    color,
                });
            }
            y += screen;
            count += 1;
        }
    };

    // Draw from coarsest to finest so finer (more transparent) is on top
    // Major (outer shell) full opacity
    let major_color: gpui::Background = rgb(t.component_border_color).into();
    // Minor 50% lower opacity than major
    let minor_color: gpui::Background = rgba((t.component_border_color << 8) | 0x80).into(); // 50%
    // Finer 50% lower than minor (25% of original)
    let finer_color: gpui::Background = rgba((t.component_border_color << 8) | 0x40).into(); // 25%

    // Only draw finer if it will be readable (not too dense)
    let finer_visible = lv.finer_visible;
    let minor_visible = lv.minor_visible;
    let major_visible = lv.major_visible;

    if major_visible {
        draw_level(major_step, major_screen, major_color);
    }
    if minor_visible {
        // If major was drawn, minor is the 5x subdivision inside it at 50% opacity.
        // If major wasn't drawn (extreme zoom), minor becomes the outer shell at full opacity.
        let c = if major_visible {
            minor_color
        } else {
            major_color
        };
        draw_level(minor_step, minor_screen, c);
    }
    if finer_visible {
        // Finer subdivision inside minor
        let c = if minor_visible && major_visible {
            finer_color
        } else if minor_visible {
            minor_color
        } else {
            major_color
        };
        draw_level(finer_step, finer_screen, c);
    }
    // Fallback: if nothing was drawn (extreme), draw base grid at full opacity
    if !major_visible && !minor_visible && !finer_visible {
        draw_level(
            crate::editor::grid::GRID_BASE,
            crate::editor::grid::GRID_BASE * camera.zoom,
            major_color,
        );
    }
}

fn cached_arc_samples<'a>(
    doc: &Document,
    sid: crate::core::ids::SegmentId,
    zoom: f64,
    cache: &'a mut HashMap<crate::core::ids::SegmentId, Vec<Point2>>,
) -> Option<&'a Vec<Point2>> {
    if !cache.contains_key(&sid) {
        let seg = doc.segment(sid)?;
        let ctrl = seg.ctrl?;
        let (a, b, c) = (doc.point(seg.start)?, doc.point(seg.end)?, doc.point(ctrl)?);
        let n = crate::editor::arc::adaptive_samples(a, b, c, zoom);
        cache.insert(sid, crate::editor::arc::segment_samples(doc, sid, n)?);
    }
    cache.get(&sid)
}

// Dashed straight line between two screen points, any angle.
fn dashed_line(
    list: &mut Vec<Primitive>,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    width: f32,
    color: gpui::Background,
) {
    const DASH: f32 = 6.;
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
            width,
            color,
        });
        t += DASH + GAP;
    }
}

/// Two short diagonal lines forming a V-shaped arrowhead at the end of
/// a dim line, pointing outward.
fn dim_arrowhead(
    list: &mut Vec<Primitive>,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    color: gpui::Background,
) {
    const LEN: f32 = 6.;
    const SPREAD: f32 = 4.;
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return;
    }
    let ux = dx / len;
    let uy = dy / len;
    let nx = -uy;
    let ny = ux;
    // Tip at end, two arms angled back and outward.
    let lx = bx - ux * LEN + nx * SPREAD;
    let ly = by - uy * LEN + ny * SPREAD;
    let rx = bx - ux * LEN - nx * SPREAD;
    let ry = by - uy * LEN - ny * SPREAD;
    list.push(Primitive::Line {
        ax: bx,
        ay: by,
        bx: lx,
        by: ly,
        width: 1.,
        color,
    });
    list.push(Primitive::Line {
        ax: bx,
        ay: by,
        bx: rx,
        by: ry,
        width: 1.,
        color,
    });
}
