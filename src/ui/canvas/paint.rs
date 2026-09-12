use gpui::{Pixels, Size, rgb, rgba};
use std::collections::HashMap;

use crate::core::constraints::ElementRef;
use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::{Point2, Rect};
use crate::editor::dims::DimRender;
use crate::editor::ruler;
use crate::editor::{Camera, SnapGuide};
use crate::theme::Theme;

/// Cached bezier flattening: tessellation plus its bounds for O(1)
/// viewport culling (previously every curve scanned all ~160 samples per
/// frame just to ask "are you visible?").
pub struct BezEntry {
    pub fp: u64,
    pub pts: Vec<Point2>,
    /// (min_x, min_y, max_x, max_y) over `pts`.
    pub bb: [f64; 4],
}

#[derive(Default)]
pub struct RenderCache {
    arcs: HashMap<crate::core::ids::SegmentId, (u64, Vec<Point2>)>,
    beziers: HashMap<crate::core::ids::SegmentId, BezEntry>,
}

/// Quantized zoom for cache fingerprints. Retessellating on every
/// fractional wheel tick is invisible (<1 sample of difference) but costs
/// a full flatten per curve; 1/64 steps keep tessellation visually
/// identical while the cache actually hits during zoom gestures.
fn zoom_key(zoom: f64) -> u64 {
    (zoom * 64.0).round().max(1.0).to_bits()
}

fn bbox_of(pts: &[Point2]) -> [f64; 4] {
    let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in pts {
        if p.x < bb[0] {
            bb[0] = p.x;
        }
        if p.y < bb[1] {
            bb[1] = p.y;
        }
        if p.x > bb[2] {
            bb[2] = p.x;
        }
        if p.y > bb[3] {
            bb[3] = p.y;
        }
    }
    if bb[0].is_infinite() {
        bb = [0., 0., 0., 0.];
    }
    bb
}

impl RenderCache {
    fn clear_if_oversized(&mut self) {
        // Arcs only: beziers have their own incremental eviction below.
        // The old combined clear wiped both maps at once, forcing every
        // curve to retessellate on the same frame.
        if self.arcs.len() >= 4096 {
            self.arcs.clear();
        }
    }

    /// Incremental eviction for the (larger) bezier entries: drop an
    /// arbitrary half instead of everything. The old all-clear made every
    /// curve retessellate on the same frame (jank spike + transient 2x
    /// memory from old + new Vecs).
    fn evict_beziers_if_oversized(&mut self) {
        const CAP: usize = 2048;
        if self.beziers.len() >= CAP {
            let kill: Vec<crate::core::ids::SegmentId> =
                self.beziers.keys().take(CAP / 2).copied().collect();
            for k in kill {
                self.beziers.remove(&k);
            }
        }
    }

    pub fn bezier_samples(
        &mut self,
        doc: &Document,
        sid: crate::core::ids::SegmentId,
        zoom: f64,
    ) -> Option<&BezEntry> {
        let seg = doc.segment(sid)?;
        if seg.kind != SegmentKind::Bezier {
            return None;
        }
        let (h1, h2) = seg.bezier_handles();
        let (a, b, c, d) = (
            doc.point(seg.start)?,
            doc.point(h1?)?,
            doc.point(h2?)?,
            doc.point(seg.end)?,
        );
        let zkey = zoom_key(zoom);
        let fingerprint = [
            zkey,
            a.x.to_bits(),
            a.y.to_bits(),
            b.x.to_bits(),
            b.y.to_bits(),
            c.x.to_bits(),
            c.y.to_bits(),
            d.x.to_bits(),
            d.y.to_bits(),
        ]
        .iter()
        .fold(0xcbf29ce484222325, |hash, bits| {
            (hash ^ bits).wrapping_mul(0x100000001b3)
        });
        let needs_refresh = self
            .beziers
            .get(&sid)
            .is_none_or(|e| e.fp != fingerprint);
        if needs_refresh {
            let n = crate::editor::bezier::adaptive_samples(a, b, c, d, zoom);
            self.evict_beziers_if_oversized();
            // Reuse the retained allocation when the entry exists: during
            // drags the fingerprint changes every frame, and a fresh Vec
            // per curve per frame was pure allocator churn.
            let entry = self.beziers.entry(sid).or_insert_with(|| BezEntry {
                fp: 0,
                pts: Vec::new(),
                bb: [0., 0., 0., 0.],
            });
            crate::editor::bezier::samples_into(a, b, c, d, n, &mut entry.pts);
            entry.fp = fingerprint;
            entry.bb = bbox_of(&entry.pts);
        }
        self.beziers.get(&sid)
    }

    fn arc_samples(
        &mut self,
        doc: &Document,
        sid: crate::core::ids::SegmentId,
        zoom: f64,
    ) -> Option<&Vec<Point2>> {
        let seg = doc.segment(sid)?;
        let ctrl = seg.ctrl?;
        let (a, b, c) = (doc.point(seg.start)?, doc.point(seg.end)?, doc.point(ctrl)?);
        let fingerprint = [
            zoom_key(zoom),
            a.x.to_bits(), a.y.to_bits(),
            b.x.to_bits(), b.y.to_bits(),
            c.x.to_bits(), c.y.to_bits(),
        ]
        .iter()
        .fold(0xcbf29ce484222325, |hash, bits| {
            (hash ^ bits).wrapping_mul(0x100000001b3)
        });
        let needs_refresh = self
            .arcs
            .get(&sid)
            .is_none_or(|(old_fingerprint, _)| *old_fingerprint != fingerprint);
        if needs_refresh {
            let n = crate::editor::arc::adaptive_samples(a, b, c, zoom);
            let samples = crate::editor::arc::segment_samples(doc, sid, n)?;
            self.clear_if_oversized();
            self.arcs.insert(sid, (fingerprint, samples));
        }
        self.arcs.get(&sid).map(|(_, samples)| samples)
    }
}

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
    // Solid color disk: round joins/caps for stroked polylines. Strokes
    // paint as butt-jointed quads, which crack open at every tessellation
    // joint once the weight gets thick — a disk per vertex fuses them.
    Disk {
        cx: f32,
        cy: f32,
        radius: f32,
        color: gpui::Background,
    },
    // White circle marking an editable/snapped point.
    Circle {
        cx: f32,
        cy: f32,
        radius: f32,
    },
    // Solid accent diamond marking a BEZIER handle point — deliberately
    // distinct from point dots so handles never read as geometry.
    Diamond {
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
    curve_dim_renders: &[crate::editor::dims::CurveDimRender],
    snap_guides: &[SnapGuide],
    marquee: Option<(Point2, Point2)>,
    pending_ruler: Option<(Point2, Point2)>,
    pending_line: Option<(Point2, Point2)>,
    constraint_markers: &[crate::editor::dims::ConstraintMarker],
    pending_circle: Option<crate::editor::PendingCircle>,
    pending_pen: Option<crate::editor::PendingPen>,
    pending_bezier: Option<crate::editor::PendingBezier>,
    show_grid: bool,
    tool: crate::editor::Tool,
    cursor_doc: Option<Point2>,
    cache: Option<&mut RenderCache>,
    fillet_preview: Option<crate::editor::fillet::FilletPreview>,
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
    let element_count: usize = doc.layers.iter().map(|layer| layer.elements.len()).sum();
    // Bezier handle ids, built ONCE per frame (was O(sel·segs): a full
    // segment scan per selected point via is_bezier_handle).
    let bezier_handles: std::collections::HashSet<crate::core::ids::PointId> = doc
        .all_segments()
        .filter(|(_, s)| s.kind == SegmentKind::Bezier)
        .flat_map(|(_, s)| [s.ctrl, s.center].into_iter().flatten())
        .collect();
    let mut list = Vec::with_capacity(element_count.saturating_mul(2).saturating_add(64));
    // Reused screen-space workspaces for curve flattening: every curve in
    // the frame shares these instead of allocating a Vec per curve per
    // frame (the allocator churn showed up as heap growth while dragging).
    let mut scr_buf: Vec<(f32, f32)> = Vec::new();
    let mut sim_buf: Vec<(f32, f32)> = Vec::new();
    let mut owned_cache = RenderCache::default();
    let cache: &mut RenderCache = match cache {
        Some(cache) => cache,
        None => &mut owned_cache,
    };

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
                    let Some(fill) = doc.fill(fid) else { continue };
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
                        color: rgb(fill.fill_color).into(),
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
                        && let Some((a, b)) = fillet_trimmed_line(doc, sid)
                            .or_else(|| doc.segment_geom(sid))
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
                            color: rgba((seg.stroke_color << 8) | ((seg.opacity.clamp(0., 1.) * 255.) as u32)).into(),
                        });
                    }
                    // Arc segments: sampled polyline of the arc through
                    // start -> ctrl -> end (adaptive so the curve stays
                    // smooth at any zoom). Incomplete arcs also show their
                    // dashed complementary portion.
                    if seg.kind == SegmentKind::Arc {
                        // Fillet arcs resolve their stroke live from the
                        // source edges (or paint nothing when unstroked)
                        // instead of the unconditional arc stroke below.
                        let fillet_style = doc
                            .modifiers
                            .iter()
                            .find(|m| m.arc == Some(sid))
                            .map(|m| source_stroke(doc, m.first, m.second));
                        if matches!(fillet_style, Some(None)) {
                            continue;
                        }
                        let Some(sc) = seg.ctrl else { continue };
                        let (Some(sa), Some(sb), Some(scp)) =
                            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
                        else {
                            continue;
                        };
                        // The circumcircle bounds are a conservative cheap
                        // rejection test. It may keep some offscreen arcs,
                        // but never removes a visible one, and avoids
                        // tessellating distant geometry.
                        if let Some((center, radius)) =
                            crate::editor::arc::circumcircle(sa, sb, scp)
                            && (center.x + radius < visible.origin.x
                                || center.x - radius > visible.origin.x + visible.size.w
                                || center.y + radius < visible.origin.y
                                || center.y - radius > visible.origin.y + visible.size.h)
                        {
                            continue;
                        }
                        // Note: no outer adaptive_samples here — the cache
                        // computes it internally (the old code computed it
                        // twice: 3 atan2s discarded per arc per frame).
                        let Some(samples) = cache.arc_samples(doc, sid, camera.zoom) else {
                            continue;
                        };
                        if samples.iter().any(|p| visible.contains(*p)) {
                            let live = trimmed_samples(doc, sid, samples);
                            let (w, col) = fillet_style.flatten().unwrap_or((
                                1.5,
                                rgba((seg.stroke_color << 8) | ((seg.opacity.clamp(0., 1.) * 255.) as u32)).into(),
                            ));
                            push_simplified_polyline(
                                &mut list,
                                &mut scr_buf,
                                &mut sim_buf,
                                live,
                                &scr,
                                w,
                                col,
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
                            let cn = crate::editor::arc::adaptive_samples(sa, sb, scp, camera.zoom).max(32);
                            let comp =
                                crate::editor::arc::complement_samples(sa, sb, scp, cn);
                            let pts: Vec<(f32, f32)> = comp.iter().map(|p| scr(*p)).collect();
                            dashed_polyline(&mut list, &pts, accent);
                        }
                    }
                    // Bezier spans: cached flatten, viewport-culled.
                    if seg.kind == SegmentKind::Bezier {
                        let (h1, h2) = seg.bezier_handles();
                        let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                            doc.point(seg.start),
                            h1.and_then(|id| doc.point(id)),
                            h2.and_then(|id| doc.point(id)),
                            doc.point(seg.end),
                        ) else {
                            continue;
                        };
                        // Note: adaptive count lives inside the cache —
                        // the old outer computation was discarded.
                        let Some(entry) = cache.bezier_samples(doc, sid, camera.zoom) else {
                            continue;
                        };
                        // O(1) bbox cull on the cached bounds (was a full
                        // N-point scan per curve per frame).
                        let bb = entry.bb;
                        let vx0 = visible.origin.x;
                        let vy0 = visible.origin.y;
                        if bb[2] < vx0
                            || bb[0] > vx0 + visible.size.w
                            || bb[3] < vy0
                            || bb[1] > vy0 + visible.size.h
                        {
                            // Still fall through to handles below.
                        } else {
                            let live = trimmed_samples(doc, sid, &entry.pts);
                            push_simplified_polyline(
                                &mut list,
                                &mut scr_buf,
                                &mut sim_buf,
                                live,
                                &scr,
                                seg.stroke_width.max(1.) as f32,
                                color,
                            );
                        }
                        // Handles whenever the span OR any of its four
                        // points (endpoints included) is selected or
                        // hovered: dashed arms + diamond dots. Selecting
                        // just the point still reveals its handles.
                        let hot = |pid: crate::core::ids::PointId| {
                            selection.contains(&ElementRef::Point(pid))
                                || hover.is_some_and(|h| h == ElementRef::Point(pid))
                        };
                        let touched = selection.contains(&el)
                            || hot(seg.start)
                            || hot(seg.end)
                            || h1.is_some_and(hot)
                            || h2.is_some_and(hot);
                        if touched {
                            // Handle arms are solid 1px accent lines (never
                            // dashed) with diamond dots.
                            let (x0, y0) = scr(p0);
                            let (x1, y1) = scr(c1);
                            let (x2, y2) = scr(c2);
                            let (x3, y3) = scr(p1);
                            for (ax, ay, bx, by) in
                                [(x0, y0, x1, y1), (x3, y3, x2, y2)]
                            {
                                list.push(Primitive::Line {
                                    ax,
                                    ay,
                                    bx,
                                    by,
                                    width: 1.,
                                    color: accent,
                                });
                            }
                            list.push(Primitive::Diamond { cx: x1, cy: y1, radius: 5. });
                            list.push(Primitive::Diamond { cx: x2, cy: y2, radius: 5. });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Fillets are derived geometry: linked arcs paint through the arc
    // pass above (source-resolved style); this fallback only covers
    // unlinkable modifiers, styled the same — never a default stroke.
    for modifier in &doc.modifiers {
        if modifier.arc.is_some() {
            continue;
        }
        if let Some(g) = modifier.evaluate(doc)
            && let Some((w, col)) = source_stroke(doc, modifier.first, modifier.second)
        {
            let n = crate::editor::arc::adaptive_samples(
                g.first_tangent,
                g.second_tangent,
                g.control,
                camera.zoom,
            );
            let pts = crate::editor::arc::samples_through(g.first_tangent, g.second_tangent, g.control, n);
            push_simplified_polyline(&mut list, &mut scr_buf, &mut sim_buf, &pts, &scr, w, col);
        }
    }
    // Fillet drag affordances: committed fillet centers always show while
    // the Fillet tool is active, plus for any selected fillet arc (the
    // radius-drag grab targets). Otherwise they stay hidden, as before.
    let show_centers = tool == crate::editor::Tool::Fillet;
    for m in &doc.modifiers {
        let selected = m.arc.is_some_and(|a| selection.contains(&ElementRef::Segment(a)));
        if !(show_centers || selected) {
            continue;
        }
        if let Some(c) = m.center.and_then(|id| doc.point(id)) {
            let (x, y) = scr(c);
            list.push(Primitive::Circle { cx: x, cy: y, radius: 4. });
        }
    }
    if let Some(preview) = fillet_preview {
        let g = preview.geometry;
        let n = crate::editor::arc::adaptive_samples(
            g.first_tangent,
            g.second_tangent,
            g.control,
            camera.zoom,
        );
        let pts = crate::editor::arc::samples_through(g.first_tangent, g.second_tangent, g.control, n);
        push_simplified_polyline(&mut list, &mut scr_buf, &mut sim_buf, &pts, &scr, 2.2, accent);
        let (x,y)=scr(g.center);
        list.push(Primitive::Circle { cx:x, cy:y, radius:4. });
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

    // 2a2) Curve-length dimensions: the dim line IS the exact curve shape —
    // a dashed offset replica of the bezier path (never the chord), with
    // the value container riding on it (label painted by the DOM layer).
    for c in curve_dim_renders {
        let ink = if c.constraint {
            if c.hovered {
                rgb(t.text_secondary).into()
            } else {
                rgb(t.empty_text_secondary).into()
            }
        } else {
            accent
        };
        let pts: Vec<(f32, f32)> = c.pts.iter().map(|p| (p[0], p[1])).collect();
        dashed_polyline(&mut list, &pts, ink);
        // Offset clamping stubs: replica ends to the real endpoints.
        for s in &c.stubs {
            dashed_line(&mut list, s[0], s[1], s[2], s[3], 1., ink);
        }
        // Arrowheads along the replica's end tangents.
        if pts.len() >= 2 {
            let (a, b) = (pts[0], pts[1]);
            dim_arrowhead(&mut list, b.0, b.1, a.0, a.1, ink);
            let (a, b) = (pts[pts.len() - 2], pts[pts.len() - 1]);
            dim_arrowhead(&mut list, a.0, a.1, b.0, b.1, ink);
        }
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
            | crate::editor::Tool::Pen
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
        element_outline(doc, h, &scr, accent, &mut list, camera.zoom, cache, &bezier_handles, &mut scr_buf, &mut sim_buf);
    }

    // 5) Selection highlights + point handles drawn after everything —
    // points are the topmost affordance in the entire stack.
    for &sel in selection {
        element_outline(doc, sel, &scr, accent, &mut list, camera.zoom, cache, &bezier_handles, &mut scr_buf, &mut sim_buf);
    }
    for &sel in selection {
        for pid in doc.element_points(sel) {
            if let Some(p) = doc.point(pid) {
                let (x, y) = scr(p);
                // Bezier handle points render as diamonds, never dots.
                if bezier_handles.contains(&pid) {
                    list.push(Primitive::Diamond {
                        cx: x,
                        cy: y,
                        radius: 5.,
                    });
                    continue;
                }
                list.push(Primitive::Circle {
                    cx: x,
                    cy: y,
                    radius: 4.,
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
                    push_simplified_polyline(&mut list, &mut scr_buf, &mut sim_buf, &arc, &scr, 1.5, color);
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

    // Unified pen preview.
    if let Some(pen) = pending_pen {
        match pen.mode {
            crate::editor::PenMode::Line => {
                if let Some(l) = pen.line {
                    let (ax, ay) = scr(l.start);
                    let (bx, by) = scr(l.cursor);
                    list.push(Primitive::Line {
                        ax,
                        ay,
                        bx,
                        by,
                        width: 1.,
                        color: accent,
                    });
                }
            }
            crate::editor::PenMode::Arc => {
                if let Some(pc) = pen.circle {
                    match pc.stage() {
                        2 => {
                            if let Some(a) = pc.a {
                                push_chord_preview(&mut list, &scr, a, pc.cursor, accent);
                            }
                        }
                        _ => {
                            if let (Some(a), Some(b)) = (pc.a, pc.b) {
                                let n = crate::editor::arc::adaptive_samples(
                                    a,
                                    b,
                                    pc.cursor,
                                    camera.zoom,
                                );
                                let arc = crate::editor::arc::samples_through(a, b, pc.cursor, n);
                                push_simplified_polyline(&mut list, &mut scr_buf, &mut sim_buf, &arc, &scr, 1.5, color);
                            }
                        }
                    }
                }
            }
            crate::editor::PenMode::Bezier => {
                if let Some(pb) = pen.bezier {
                    // On-curve UX: the live end is the fixed endpoint when
                    // set, else the cursor. Both handle arms preview.
                    let end = pb.p1.unwrap_or(pb.cursor);
                    let (c1, c2) = pb.effective(end);
                    let pts = crate::editor::bezier::samples(pb.p0, c1, c2, end, 32);
                    push_simplified_polyline(&mut list, &mut scr_buf, &mut sim_buf, &pts, &scr, 1.5, accent);
                    // Anchor + live end dots.
                    for p in [pb.p0, end] {
                        let (x, y) = scr(p);
                        list.push(Primitive::Circle { cx: x, cy: y, radius: 3. });
                    }
                    // Dragged handle arms (solid 1px accent): h1 rides the
                    // p0 side; the live end shows the full symmetric pair
                    // — the forward drag arm plus the working (mirrored)
                    // arm the curve follows.
                    let arm = |list: &mut Vec<Primitive>,
                               ax: f32,
                               ay: f32,
                               hx: f32,
                               hy: f32| {
                        list.push(Primitive::Line {
                            ax,
                            ay,
                            bx: hx,
                            by: hy,
                            width: 1.,
                            color: accent,
                        });
                        list.push(Primitive::Diamond { cx: hx, cy: hy, radius: 5. });
                    };
                    if let Some(h) = pb.h1 {
                        let (ax, ay) = scr(pb.p0);
                        let (hx, hy) = scr(h);
                        arm(&mut list, ax, ay, hx, hy);
                    }
                    if let Some(h) = pb.h2 {
                        let (ex, ey) = scr(end);
                        let (hx, hy) = scr(h);
                        arm(&mut list, ex, ey, hx, hy);
                        let (cx2, cy2) = scr(c2);
                        arm(&mut list, ex, ey, cx2, cy2);
                    }
                }
            }
        }
    }
    if let Some(pb) = pending_bezier {
        let end = pb.p1.unwrap_or(pb.cursor);
        let (c1, c2) = pb.effective(end);
        let pts = crate::editor::bezier::samples(pb.p0, c1, c2, end, 32);
        push_simplified_polyline(&mut list, &mut scr_buf, &mut sim_buf, &pts, &scr, 1.5, accent);
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

// Solid polyline (arc rendering) with round joins AND round caps: a
// disk per vertex fuses the butt-jointed quads below, so thick strokes
// never crack at tessellation joints and curves read round instead of
// faceted. Zero-length runs collapse to a single disk, never NaNs.
// Disks are gated to width > 2.0: at thin UI widths (1–1.5px, the common
// case) joint cracks are subpixel and the N extra Disk primitives double
// the draw list for no visible change.
fn push_polyline(
    list: &mut Vec<Primitive>,
    pts: &[(f32, f32)],
    width: f32,
    color: gpui::Background,
) {
    if pts.is_empty() {
        return;
    }
    list.reserve(pts.len());
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
    if width <= 2.0 {
        return;
    }
    let r = (width / 2.).max(0.5);
    let mut prev: Option<(f32, f32)> = None;
    for &p in pts {
        // Skip degenerate repeats (a zero-length run still needs exactly
        // one disk, emitted on its first occurrence).
        if prev.is_some_and(|q| (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) < 1e-12) {
            continue;
        }
        prev = Some(p);
        list.push(Primitive::Disk { cx: p.0, cy: p.1, radius: r, color });
    }
}

// Streaming polyline simplification with a hard subpixel error bound
// (greedy RDP: emit the farthest deviator, restart from it). Smooth
// curves sampled at ~3px collapse 100+ tessellation points to ~10 with
// <0.25px deviation — invisible under AA — so each curve emits ~10 GPU
// primitives instead of ~100. Cusps survive: deviation spikes keep them.
// O(n·run) worst case, trivial for n ≤ 1024.
fn simplify_screen(out: &mut Vec<(f32, f32)>, pts: &[(f32, f32)]) {
    const EPS: f32 = 0.25; // px
    out.clear();
    if pts.len() <= 2 {
        out.extend_from_slice(pts);
        return;
    }
    out.reserve(pts.len());
    let mut anchor = 0usize;
    out.push(pts[0]);
    let mut i = 1usize;
    while i < pts.len() {
        let (ax, ay) = pts[anchor];
        let (bx, by) = pts[i];
        let dx = bx - ax;
        let dy = by - ay;
        let len = (dx * dx + dy * dy).sqrt();
        let mut max_d = 0f32;
        let mut max_j = anchor + 1;
        if len > 1e-6 {
            let inv = 1.0 / len;
            for j in anchor + 1..i {
                let d = ((pts[j].0 - ax) * dy - (pts[j].1 - ay) * dx).abs() * inv;
                if d > max_d {
                    max_d = d;
                    max_j = j;
                }
            }
        }
        if max_d > EPS {
            out.push(pts[max_j]);
            anchor = max_j;
        } else {
            i += 1;
        }
    }
    let last = pts[pts.len() - 1];
    if out.last() != Some(&last) {
        out.push(last);
    }
}

/// Transform doc points to screen, simplify, and emit as one stroked
/// polyline — with zero per-curve allocation once the caller's scratch
/// buffers are warm. Replaces the `collect::<Vec<_>>()` + `push_polyline`
/// pattern at every curve paint site.
fn push_simplified_polyline(
    list: &mut Vec<Primitive>,
    scr_buf: &mut Vec<(f32, f32)>,
    sim_buf: &mut Vec<(f32, f32)>,
    pts: &[Point2],
    scr: &impl Fn(Point2) -> (f32, f32),
    width: f32,
    color: gpui::Background,
) {
    scr_buf.clear();
    scr_buf.reserve(pts.len());
    for p in pts {
        scr_buf.push(scr(*p));
    }
    simplify_screen(sim_buf, scr_buf);
    push_polyline(list, sim_buf, width, color);
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

/// Stroke style for a fillet, resolved live from its source edges: the
/// first source with a real stroke wins (weight + color). None when no
/// source is stroked — the fillet then paints nothing at all.
fn source_stroke(
    doc: &Document,
    first: crate::core::ids::SegmentId,
    second: crate::core::ids::SegmentId,
) -> Option<(f32, gpui::Background)> {
    for src in [first, second] {
        if let Some(s) = doc.segment(src)
            && s.stroke_width > 0.
        {
            return Some((
                s.stroke_width as f32,
                rgba((s.stroke_color << 8) | ((s.opacity.clamp(0., 1.) * 255.) as u32)).into(),
            ));
        }
    }
    None
}

/// Render span for a line feeding a fillet: far end to tangent point, so
/// the sharp corner stub never sticks out past the arc. None when the
/// segment isn't a filleted line or its ids don't resolve (draw full).
fn fillet_trimmed_line(
    doc: &Document,
    sid: crate::core::ids::SegmentId,
) -> Option<(Point2, Point2)> {
    let seg = doc.segment(sid)?;
    if seg.kind != SegmentKind::Line {
        return None;
    }
    let m = doc
        .modifiers
        .iter()
        .find(|m| m.first == sid || m.second == sid)?;
    let tangent = doc.point(if m.first == sid {
        m.first_tangent?
    } else {
        m.second_tangent?
    })?;
    let far_id = if seg.start == m.corner {
        seg.end
    } else if seg.end == m.corner {
        seg.start
    } else {
        return None;
    };
    Some((doc.point(far_id)?, tangent))
}

/// Corner + tangent positions for a fillet source curve (arc/bezier),
/// used to truncate the corner stub out of cached samples. Never matches
/// the fillet arcs themselves.
fn fillet_source_cut(
    doc: &Document,
    sid: crate::core::ids::SegmentId,
) -> Option<(Point2, Point2)> {
    let seg = doc.segment(sid)?;
    if !matches!(seg.kind, SegmentKind::Arc | SegmentKind::Bezier) {
        return None;
    }
    if doc.modifiers.iter().any(|m| m.arc == Some(sid)) {
        return None;
    }
    let m = doc
        .modifiers
        .iter()
        .find(|m| m.first == sid || m.second == sid)?;
    let tangent = doc.point(if m.first == sid {
        m.first_tangent?
    } else {
        m.second_tangent?
    })?;
    Some((doc.point(m.corner)?, tangent))
}

/// Sample range keeping a source curve's far-end side, dropping the
/// corner stub past the tangent point. Falls back to full-span on any
/// doubt (range < 2 kept, tangent coinciding with the corner end).
fn trim_sample_range(pts: &[Point2], corner: Point2, tangent: Point2) -> Option<(usize, usize)> {
    if pts.len() < 3 {
        return None;
    }
    let near = |p: Point2| {
        pts.iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = (a.x - p.x).powi(2) + (a.y - p.y).powi(2);
                let db = (b.x - p.x).powi(2) + (b.y - p.y).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (i_c, i_t) = (near(corner), near(tangent));
    if i_c == i_t {
        return None;
    }
    let (lo, hi) = if i_c < i_t {
        (i_t, pts.len())
    } else {
        (0, i_t + 1)
    };
    if hi - lo < 2 {
        return None;
    }
    Some((lo, hi))
}

/// Cached samples truncated to a fillet source's live span (far end to
/// tangent), or the full sample set when the segment isn't a filleted
/// source curve.
fn trimmed_samples<'a>(
    doc: &Document,
    sid: crate::core::ids::SegmentId,
    samples: &'a [Point2],
) -> &'a [Point2] {
    match fillet_source_cut(doc, sid).and_then(|(c, t)| trim_sample_range(samples, c, t)) {
        Some((lo, hi)) => &samples[lo..hi.min(samples.len())],
        None => samples,
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
    cache: &mut RenderCache,
    bezier_handles: &std::collections::HashSet<crate::core::ids::PointId>,
    scr_buf: &mut Vec<(f32, f32)>,
    sim_buf: &mut Vec<(f32, f32)>,
) {
    match el {
        // Points use the SAME styling everywhere: one clean small dot —
        // except bezier handles, which are always diamonds.
        ElementRef::Point(pid) => {
            if let Some(p) = doc.point(pid) {
                let (x, y) = scr(p);
                if bezier_handles.contains(&pid) {
                    list.push(Primitive::Diamond {
                        cx: x,
                        cy: y,
                        radius: 5.,
                    });
                } else {
                    list.push(Primitive::Circle {
                        cx: x,
                        cy: y,
                        radius: 4.,
                    });
                }
            }
        }
        ElementRef::Segment(sid) => {
            if let Some(seg) = doc.segment(sid)
                && seg.kind == SegmentKind::Arc
                && let Some(samples) = cache.arc_samples(doc, sid, zoom)
            {
                let live = trimmed_samples(doc, sid, samples);
                push_simplified_polyline(list, scr_buf, sim_buf, live, scr, 2.5, accent);
            } else if let Some(seg) = doc.segment(sid)
                && seg.kind == SegmentKind::Bezier
                && let Some(entry) = cache.bezier_samples(doc, sid, zoom)
            {
                let live = trimmed_samples(doc, sid, &entry.pts);
                push_simplified_polyline(list, scr_buf, sim_buf, live, scr, 2.5, accent);
            } else if let Some((a, b)) = fillet_trimmed_line(doc, sid).or_else(|| doc.segment_geom(sid)) {
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
