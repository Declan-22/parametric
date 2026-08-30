use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::PointId;

use super::pick::distance;

// Snapping: candidate locations exposed by geometry, best-match search,
// and visual guide descriptors. Pure functions over document + camera.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Edge,
    /// A drawn grid intersection (both axes lock together).
    Grid,
}

// A visual snap connection: what locked onto what. Edge snaps carry the
// target's full span so rendering can trace it. `from` is the FEATURE
// (what locked), `to` the manipulated point — the line between them is the
// connection drawn on screen. `solid` marks FULL feature locks (both axes
// onto one feature, or a grid crossing): only those earn the accent badge.
// `linked` marks features ALREADY connected to the point by existing
// geometry — the shape itself shows that connection, so the stub line is
// suppressed (dot + badge still render).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapGuide {
    pub vertical: bool,
    pub from: Point2,
    pub to: Point2,
    pub kind: SnapKind,
    pub solid: bool,
    pub linked: bool,
    pub span_is_x: bool,
    pub span_lo: f64,
    pub span_hi: f64,
}

// One candidate location other geometry exposes for snapping. Points offer
// both axes; edges snap only their normal axis within their span.
#[derive(Clone, Copy, Debug)]
pub struct SnapTarget {
    pub x: f64,
    pub y: f64,
    pub kind: SnapKind,
    pub snap_x: bool,
    pub snap_y: bool,
    pub span_lo: f64,
    pub span_hi: f64,
    pub span_is_x: bool,
}

/// All snap locations exposed by the geometry. `endpoints_only` keeps
/// single-point resize drags fluid. `exclude_pts` silences endpoint AND
/// midpoint targets (the dragged object's component must never receive
/// snaps); `exclude_segs` silences edge-span targets (only the actually
/// dragged segments).
pub fn targets(
    doc: &Document,
    exclude_pts: &[PointId],
    exclude_segs: &[crate::core::ids::SegmentId],
    endpoints_only: bool,
    visible: Rect,
) -> Vec<SnapTarget> {
    let _ = exclude_segs;
    let mut out = Vec::new();
    for (pid, p) in doc.all_points() {
        if exclude_pts.contains(&pid) {
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
    // Ruler graduations: every half-inch and inch mark along a ruler is a
    // snap target (both axes), so objects align to the measuring system.
    for (sid, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Ruler {
            continue;
        }
        let Some((a, b)) = doc.segment_geom(sid) else { continue };
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let step = crate::editor::ruler::HALF_INCH;
        let steps = (len / step).floor() as usize;
        for k in 1..steps {
            let d = step * k as f64;
            out.push(SnapTarget {
                x: a.x + ux * d,
                y: a.y + uy * d,
                kind: SnapKind::Endpoint,
                snap_x: true,
                snap_y: true,
                span_lo: 0.,
                span_hi: 0.,
                span_is_x: false,
            });
        }
    }
    // Arc centers (circumcenters) — snappable midpoints for circles.
    for (sid, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Arc {
            continue;
        }
        let Some(sc) = seg.ctrl else { continue };
        let (Some(a), Some(b), Some(c)) =
            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
        else {
            continue;
        };
        if exclude_pts.contains(&seg.start)
            || exclude_pts.contains(&seg.end)
            || exclude_pts.contains(&sc)
            || seg.center.is_some_and(|id| exclude_pts.contains(&id))
        {
            continue;
        }
        if let Some((center, _)) = crate::editor::arc::circumcircle(a, b, c) {
            if !visible.contains(center) {
                continue;
            }
            out.push(SnapTarget {
                x: center.x,
                y: center.y,
                kind: SnapKind::Midpoint,
                snap_x: true,
                snap_y: true,
                span_lo: 0.,
                span_hi: 0.,
                span_is_x: false,
            });
        }
    }
    for (sid, seg) in doc.all_segments() {
        if seg.kind == SegmentKind::Ruler {
            // Rulers are measurement aids, not snap scaffolding.
            continue;
        }
        let Some((a, b)) = doc.segment_geom(sid) else { continue };
        let m = segment_mid_target(doc, seg, a, b);
        // Midpoints are positional targets: own-component edges never
        // offer them.
        if exclude_pts.contains(&seg.start) || exclude_pts.contains(&seg.end) {
            continue;
        }
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
        // Edge spans: horizontal edge snaps Y within X range, vertical edge
        // snaps X within Y range.
        let horizontal = (a.y - b.y).abs() < 1e-9;
        let vertical = (a.x - b.x).abs() < 1e-9;
        if horizontal {
            out.push(SnapTarget {
                x: m.x,
                y: a.y,
                kind: SnapKind::Edge,
                snap_x: false,
                snap_y: true,
                span_lo: a.x.min(b.x),
                span_hi: a.x.max(b.x),
                span_is_x: true,
            });
        } else if vertical {
            out.push(SnapTarget {
                x: a.x,
                y: m.y,
                kind: SnapKind::Edge,
                snap_x: true,
                snap_y: false,
                span_lo: a.y.min(b.y),
                span_hi: a.y.max(b.y),
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

/// Grid tolerance: the ordinary snap tolerance, capped to a fraction of the
/// drawn cell so there is always free space between intersections — the
/// grid is a magnet at crossings, never a rail along the lines.
fn grid_tol(tol: f64, step: f64, zoom: f64) -> f64 {
    if zoom < 1e-9 {
        return tol;
    }
    tol.min(0.4 * step * zoom) / zoom
}

/// Nearest drawn-grid intersection to `p`, if both axes are within `gtol`.
fn nearest_intersection(p: Point2, step: f64, gtol: f64) -> Option<(Point2, f64, f64)> {
    let gx = (p.x / step).round() * step;
    let gy = (p.y / step).round() * step;
    let dx = gx - p.x;
    let dy = gy - p.y;
    if dx.abs() <= gtol && dy.abs() <= gtol {
        Some((Point2::new(gx, gy), dx, dy))
    } else {
        None
    }
}

fn grid_guide(p: Point2, to: Point2) -> SnapGuide {
    SnapGuide {
        vertical: false,
        from: p,
        to,
        kind: SnapKind::Grid,
        solid: true,
        linked: false,
        span_is_x: false,
        span_lo: 0.,
        span_hi: 0.,
    }
}

/// Best single correction for a point against all targets. Returns
/// (adjustment delta, guides). `coincident_only` demands BOTH axes hit —
/// used while dragging so a passing row/column alignment doesn't yank a
/// single axis (the "slight 90-degree snap"). `visible` culls targets to
/// the viewport (plus margin already applied by the caller) — snapping is
/// a proximity affair, not a document-wide search.
///
/// `grid_step` (Some when Snap to Grid is on) adds the DRAWN grid's
/// intersections as targets. Objects keep priority: the grid only fills
/// axes no object locked, so both snap modes work together instead of
/// excluding each other. Single-axis grid fills are skipped for
/// `coincident_only` / `endpoints_only` drags (resizes stay fluid).
pub fn best(
    doc: &Document,
    tol: f64,
    p: Point2,
    exclude_pts: &[PointId],
    exclude_segs: &[crate::core::ids::SegmentId],
    endpoints_only: bool,
    coincident_only: bool,
    visible: Rect,
    grid_step: Option<f64>,
    zoom: f64,
) -> (Point2, Vec<SnapGuide>) {
    let mut best: Option<(f64, f64, f64, bool, bool, SnapTarget)> = None;
    for tgt in targets(doc, exclude_pts, exclude_segs, endpoints_only, visible) {
        if !visible.contains(Point2::new(tgt.x, tgt.y)) {
            continue;
        }
        let dx = tgt.x - p.x;
        let dy = tgt.y - p.y;
        let hit_x = tgt.snap_x && dx.abs() <= tol && span_ok(&tgt, p);
        let hit_y = tgt.snap_y && dy.abs() <= tol && span_ok(&tgt, p);
        if !hit_x && !hit_y {
            continue;
        }
        if coincident_only && !(hit_x && hit_y) {
            continue;
        }
        let score = dx.abs() + dy.abs();
        if best.as_ref().map_or(true, |(s, _, _, _, _, _)| score < *s) {
            best = Some((score, dx, dy, hit_x, hit_y, tgt));
        }
    }
    // Arc bodies — closest point on the arc curve (larger tolerance).
    let arc_tol = tol * 1.8;
    for (sid, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Arc {
            continue;
        }
        if exclude_segs.contains(&sid) {
            continue;
        }
        let Some(sc) = seg.ctrl else { continue };
        if exclude_pts.contains(&seg.start)
            || exclude_pts.contains(&seg.end)
            || exclude_pts.contains(&sc)
            || seg.center.is_some_and(|id| exclude_pts.contains(&id))
        {
            continue;
        }
        let (Some(a), Some(b), Some(c)) =
            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
        else {
            continue;
        };
        let Some(proj) = closest_point_on_arc(p, a, b, c) else {
            continue;
        };
        if !visible.contains(proj) {
            continue;
        }
        let dx = proj.x - p.x;
        let dy = proj.y - p.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d > arc_tol {
            continue;
        }
        let score = d;
        if best.as_ref().map_or(true, |(s, _, _, _, _, _)| score < *s) {
            let tgt = SnapTarget {
                x: proj.x,
                y: proj.y,
                kind: SnapKind::Edge,
                snap_x: true,
                snap_y: true,
                span_lo: 0.,
                span_hi: 0.,
                span_is_x: false,
            };
            best = Some((score, dx, dy, true, true, tgt));
        }
    }

    let Some((_, dx, dy, hit_x, hit_y, tgt)) = best else {
        // No object target: fall back to the grid, intersections only.
        // Feature and snapped point COINCIDE at the crossing: no connection
        // line, and the badge anchors at the locked position — anchoring it
        // at the raw cursor would make it trail the cursor while dragging.
        if let Some(step) = grid_step {
            let gtol = grid_tol(tol, step, zoom);
            if let Some((crossing, _, _)) = nearest_intersection(p, step, gtol) {
                return (
                    Point2::new(crossing.x - p.x, crossing.y - p.y),
                    vec![grid_guide(crossing, crossing)],
                );
            }
        }
        return (Point2::new(0., 0.), Vec::new());
    };
    let mut adj = Point2::new(0., 0.);
    if hit_x {
        adj.x = dx;
    }
    if hit_y {
        adj.y = dy;
    }
    // One axis free: fill it from the nearest grid LINE so drags ride
    // object edges while landing exactly on drawn crossings. Grid fills
    // emit no connection line — the locked crossing IS the final point.
    if let Some(step) = grid_step {
        let fillable = !coincident_only && !endpoints_only;
        if fillable && (hit_x != hit_y) {
            let gtol = grid_tol(tol, step, zoom);
            if !hit_x {
                let gx = (p.x / step).round() * step;
                let gdx = gx - p.x;
                if gdx.abs() <= gtol {
                    adj.x = gdx;
                }
            } else {
                let gy = (p.y / step).round() * step;
                let gdy = gy - p.y;
                if gdy.abs() <= gtol {
                    adj.y = gdy;
                }
            }
        }
    }
    // Connection guide: a line between the two snapping pieces — the
    // feature (from) and the snapped point (to). An axis lock spans the
    // FULL distance between them (Fusion-style alignment line); edge spans
    // anchor at their nearest end.
    let snapped = Point2::new(p.x + adj.x, p.y + adj.y);
    let mut guides = Vec::new();
    if hit_x {
        let anchor_y = match tgt.kind {
            SnapKind::Edge => snapped
                .y
                .clamp(tgt.span_lo.min(tgt.span_hi), tgt.span_lo.max(tgt.span_hi)),
            _ => tgt.y,
        };
        guides.push(SnapGuide {
            vertical: true,
            from: Point2::new(tgt.x, anchor_y),
            to: snapped,
            kind: tgt.kind,
            solid: hit_x && hit_y,
            linked: false,
            span_is_x: tgt.span_is_x,
            span_lo: tgt.span_lo,
            span_hi: tgt.span_hi,
        });
    }
    if hit_y {
        let anchor_x = match tgt.kind {
            SnapKind::Edge => snapped
                .x
                .clamp(tgt.span_lo.min(tgt.span_hi), tgt.span_lo.max(tgt.span_hi)),
            _ => tgt.x,
        };
        guides.push(SnapGuide {
            vertical: false,
            from: Point2::new(anchor_x, tgt.y),
            to: snapped,
            kind: tgt.kind,
            solid: hit_x && hit_y,
            linked: false,
            span_is_x: tgt.span_is_x,
            span_lo: tgt.span_lo,
            span_hi: tgt.span_hi,
        });
    }
    (adj, guides)
}

pub fn mid(a: Point2, b: Point2) -> Point2 {
    Point2::new((a.x + b.x) / 2., (a.y + b.y) / 2.)
}

/// Segment midpoint as a snap target: lines use the chord midpoint; arcs
/// project to the middle of their sweep — ON the curve (the chord midpoint
/// of an arc floats in empty space off the bend).
fn segment_mid_target(doc: &Document, seg: crate::core::document::Segment, a: Point2, b: Point2) -> Point2 {
    if seg.kind == SegmentKind::Arc
        && let Some(sc) = seg.ctrl
        && let Some(c) = doc.point(sc)
        && let Some(m) = crate::editor::arc::curve_midpoint(a, b, c)
    {
        return m;
    }
    mid(a, b)
}

/// One axis' lock candidate for combined snapping.
struct AxisLock {
    to: f64,
    kind: SnapKind,
    // The feature's full position — anchors the connection line.
    feature: Point2,
    // Edge spans anchor the connection at their nearest end to the point.
    span_lo: f64,
    span_hi: f64,
}

/// Per-axis object locks: a point's coordinate or an axis-aligned edge span
/// can lock a SINGLE axis when the cursor is aligned with it (within tol on
/// that axis) but not close enough for a full point lock. Nearest wins.
fn per_axis_object_locks(
    doc: &Document,
    tol: f64,
    p: Point2,
    visible: Rect,
) -> (Option<AxisLock>, Option<AxisLock>) {
    let mut best_x: Option<(f64, AxisLock)> = None;
    let mut best_y: Option<(f64, AxisLock)> = None;

    // Endpoints.
    for (_, q) in doc.all_points() {
        if !visible.contains(q) {
            continue;
        }
        let dx = q.x - p.x;
        if dx.abs() <= tol && best_x.as_ref().map_or(true, |(s, _)| dx.abs() < *s) {
            best_x = Some((
                dx.abs(),
                AxisLock { to: q.x, kind: SnapKind::Endpoint, feature: q, span_lo: 0., span_hi: 0. },
            ));
        }
        let dy = q.y - p.y;
        if dy.abs() <= tol && best_y.as_ref().map_or(true, |(s, _)| dy.abs() < *s) {
            best_y = Some((
                dy.abs(),
                AxisLock { to: q.y, kind: SnapKind::Endpoint, feature: q, span_lo: 0., span_hi: 0. },
            ));
        }
    }
    // Arc centers (circumcenters) — midpoint-grade targets.
    for (_, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Arc {
            continue;
        }
        let Some(sc) = seg.ctrl else { continue };
        let (Some(a), Some(b), Some(c)) =
            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
        else {
            continue;
        };
        let Some((center, _)) = crate::editor::arc::circumcircle(a, b, c) else {
            continue;
        };
        if !visible.contains(center) {
            continue;
        }
        let dx = center.x - p.x;
        if dx.abs() <= tol && best_x.as_ref().map_or(true, |(s, _)| dx.abs() < *s) {
            best_x = Some((
                dx.abs(),
                AxisLock {
                    to: center.x,
                    kind: SnapKind::Midpoint,
                    feature: center,
                    span_lo: 0.,
                    span_hi: 0.,
                },
            ));
        }
        let dy = center.y - p.y;
        if dy.abs() <= tol && best_y.as_ref().map_or(true, |(s, _)| dy.abs() < *s) {
            best_y = Some((
                dy.abs(),
                AxisLock {
                    to: center.y,
                    kind: SnapKind::Midpoint,
                    feature: center,
                    span_lo: 0.,
                    span_hi: 0.,
                },
            ));
        }
    }
    // Segment midpoints + axis-aligned edge spans.
    for (sid, seg) in doc.all_segments() {
        if seg.kind == SegmentKind::Ruler {
            continue;
        }
        let Some((a, b)) = doc.segment_geom(sid) else {
            continue;
        };
        let m = segment_mid_target(doc, seg, a, b);
        if visible.contains(m) {
            let dx = m.x - p.x;
            if dx.abs() <= tol && best_x.as_ref().map_or(true, |(s, _)| dx.abs() < *s) {
                best_x = Some((
                    dx.abs(),
                    AxisLock {
                        to: m.x,
                        kind: SnapKind::Midpoint,
                        feature: m,
                        span_lo: 0.,
                        span_hi: 0.,
                    },
                ));
            }
            let dy = m.y - p.y;
            if dy.abs() <= tol && best_y.as_ref().map_or(true, |(s, _)| dy.abs() < *s) {
                best_y = Some((
                    dy.abs(),
                    AxisLock {
                        to: m.y,
                        kind: SnapKind::Midpoint,
                        feature: m,
                        span_lo: 0.,
                        span_hi: 0.,
                    },
                ));
            }
        }
        let horizontal = (a.y - b.y).abs() < 1e-9;
        let vertical = (a.x - b.x).abs() < 1e-9;
        if horizontal && p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) {
            let dy = a.y - p.y;
            if dy.abs() <= tol && best_y.as_ref().map_or(true, |(s, _)| dy.abs() < *s) {
                best_y = Some((
                    dy.abs(),
                    AxisLock {
                        to: a.y,
                        kind: SnapKind::Edge,
                        feature: a,
                        span_lo: a.x.min(b.x),
                        span_hi: a.x.max(b.x),
                    },
                ));
            }
        } else if vertical && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y) {
            let dx = a.x - p.x;
            if dx.abs() <= tol && best_x.as_ref().map_or(true, |(s, _)| dx.abs() < *s) {
                best_x = Some((
                    dx.abs(),
                    AxisLock {
                        to: a.x,
                        kind: SnapKind::Edge,
                        feature: a,
                        span_lo: a.y.min(b.y),
                        span_hi: a.y.max(b.y),
                    },
                ));
            }
        }
    }
    (best_x.map(|(_, l)| l), best_y.map(|(_, l)| l))
}

/// Creation-tool combined snapping (Fusion-style):
///   1. OBJECTS FIRST, all-or-nothing: nearest endpoint > arc center >
///      midpoint > edge body > arc body within tolerance locks both axes;
///   2. otherwise per-axis: a point's coordinate or an axis-aligned edge
///      span locks the axis it aligns with;
///   3. axes still free go to the grid, intersections only: both axes free
///      snaps to the nearest DRAWN crossing (both within the grid
///      tolerance); one axis object-locked snaps the other to the nearest
///      grid line, so you ride object edges landing exactly on crossings;
///   4. everything else stays free — between intersections the cursor is
///      never yanked along a grid line.
pub fn cursor_snap_combined(
    doc: &Document,
    tol: f64,
    p: Point2,
    visible: Rect,
    grid_step: Option<f64>,
    snap_objects: bool,
    zoom: f64,
) -> (Point2, Vec<SnapGuide>) {
    // 1) Full object locks.
    if snap_objects {
        let (pos, guide) = cursor_snap(doc, tol, p, visible);
        if let Some(g) = guide {
            return (pos, vec![g]);
        }
    }

    // 2) Per-axis object locks.
    let (mut x_lock, mut y_lock) = if snap_objects {
        per_axis_object_locks(doc, tol, p, visible)
    } else {
        (None, None)
    };

    // 3) Grid fills only what the objects left free.
    if let Some(step) = grid_step {
        let gtol = grid_tol(tol, step, zoom);
        if x_lock.is_none() && y_lock.is_none() {
            // Pure grid mode for this cursor: intersections only.
            if let Some((to, _, _)) = nearest_intersection(p, step, gtol) {
                return (to, vec![grid_guide(p, to)]);
            }
        } else if x_lock.is_none() || y_lock.is_none() {
            // One axis is object-locked: the free axis may snap to the
            // nearest grid line, putting the point on a drawn crossing.
            if x_lock.is_none() {
                let gx = (p.x / step).round() * step;
                if (gx - p.x).abs() <= gtol {
                    x_lock = Some(AxisLock {
                        to: gx,
                        kind: SnapKind::Grid,
                        feature: Point2::new(gx, p.y),
                        span_lo: 0.,
                        span_hi: 0.,
                    });
                }
            } else {
                let gy = (p.y / step).round() * step;
                if (gy - p.y).abs() <= gtol {
                    y_lock = Some(AxisLock {
                        to: gy,
                        kind: SnapKind::Grid,
                        feature: Point2::new(p.x, gy),
                        span_lo: 0.,
                        span_hi: 0.,
                    });
                }
            }
        }
    }

    let x = x_lock.as_ref().map_or(p.x, |l| l.to);
    let y = y_lock.as_ref().map_or(p.y, |l| l.to);
    let snapped = Point2::new(x, y);
    // Connection guides between the two snapping pieces: the feature
    // (from) and the snapped point (to), spanning their FULL distance —
    // Fusion-style alignment lines. Grid fills are skipped: the locked
    // crossing IS the final point (nothing to connect).
    let mut guides = Vec::new();
    if let Some(l) = &x_lock {
        if l.kind != SnapKind::Grid {
            let anchor_y = if l.kind == SnapKind::Edge {
                snapped
                    .y
                    .clamp(l.span_lo.min(l.span_hi), l.span_lo.max(l.span_hi))
            } else {
                l.feature.y
            };
            guides.push(SnapGuide {
                vertical: true,
                from: Point2::new(l.to, anchor_y),
                to: snapped,
                kind: l.kind,
                solid: false,
                linked: false,
                span_is_x: false,
                span_lo: l.span_lo,
                span_hi: l.span_hi,
            });
        }
    }
    if let Some(l) = &y_lock {
        if l.kind != SnapKind::Grid {
            let anchor_x = if l.kind == SnapKind::Edge {
                snapped
                    .x
                    .clamp(l.span_lo.min(l.span_hi), l.span_lo.max(l.span_hi))
            } else {
                l.feature.x
            };
            guides.push(SnapGuide {
                vertical: false,
                from: Point2::new(anchor_x, l.to),
                to: snapped,
                kind: l.kind,
                solid: false,
                linked: false,
                span_is_x: false,
                span_lo: l.span_lo,
                span_hi: l.span_hi,
            });
        }
    }
    (snapped, guides)
}

/// Creation-tool cursor snapping. Priority:
///  1. nearest ENDPOINT within tol -> cursor locks exactly onto it;
///  2. nearest MIDPOINT within tol;
///  3. nearest point ON an edge body within tol (perpendicular lock);
///  4. otherwise free.
/// Returns the snapped position plus an optional visual guide.
pub fn cursor_snap(
    doc: &Document,
    tol: f64,
    p: Point2,
    visible: Rect,
) -> (Point2, Option<SnapGuide>) {
    // Guide convention: `from` is the FEATURE (what locked), `to` is the
    // manipulated point — so the connection line always runs between the
    // two snapping pieces.
    let guide = |feature: Point2, kind: SnapKind| {
        Some(SnapGuide {
            vertical: false,
            from: feature,
            to: p,
            kind,
            solid: true,
            linked: false,
            span_is_x: false,
            span_lo: 0.,
            span_hi: 0.,
        })
    };

    // 1) Endpoints.
    let mut best_pt: Option<(f64, Point2)> = None;
    for (_, q) in doc.all_points() {
        if !visible.contains(q) {
            continue;
        }
        let d = distance(p, q);
        if d <= tol && best_pt.map_or(true, |(bd, _)| d < bd) {
            best_pt = Some((d, q));
        }
    }
    if let Some((_, q)) = best_pt {
        return (q, guide(q, SnapKind::Endpoint));
    }

    // 1b) Arc centers (circumcenters) — treated as midpoints.
    let mut best_center: Option<(f64, Point2)> = None;
    for (_, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Arc {
            continue;
        }
        let Some(sc) = seg.ctrl else { continue };
        let (Some(a), Some(b), Some(c)) =
            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
        else {
            continue;
        };
        let Some((center, _)) = crate::editor::arc::circumcircle(a, b, c) else {
            continue;
        };
        if !visible.contains(center) {
            continue;
        }
        let d = distance(p, center);
        if d <= tol && best_center.map_or(true, |(bd, _)| d < bd) {
            best_center = Some((d, center));
        }
    }
    if let Some((_, center)) = best_center {
        return (center, guide(center, SnapKind::Midpoint));
    }

    // 2) Midpoints.
    let mut best_mid: Option<(f64, Point2)> = None;
    for (sid, seg) in doc.all_segments() {
        if seg.kind == SegmentKind::Ruler {
            continue;
        }
        let Some((a, b)) = doc.segment_geom(sid) else {
            continue;
        };
        let m = segment_mid_target(doc, seg, a, b);
        if !visible.contains(m) {
            continue;
        }
        let d = distance(p, m);
        if d <= tol && best_mid.map_or(true, |(bd, _)| d < bd) {
            best_mid = Some((d, m));
        }
    }
    if let Some((_, m)) = best_mid {
        return (m, guide(m, SnapKind::Midpoint));
    }

    // 3) Edge bodies (interior only; endpoints handled above) — lines.
    let mut best_edge: Option<(f64, Point2)> = None;
    for (sid, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Line {
            continue;
        }
        let Some((a, b)) = doc.segment_geom(sid) else {
            continue;
        };
        if !visible.contains(a) && !visible.contains(b) {
            continue;
        }
        let proj = closest_point_on_segment(p, a, b);
        if distance(proj, a) < 1e-9 || distance(proj, b) < 1e-9 {
            continue;
        }
        let d = distance(p, proj);
        if d <= tol && best_edge.map_or(true, |(bd, _)| d < bd) {
            best_edge = Some((d, proj));
        }
    }
    if let Some((_, proj)) = best_edge {
        return (proj, guide(proj, SnapKind::Edge));
    }

    // 4) Arc bodies — closest point on the arc curve (with larger tolerance).
    let arc_tol = tol * 1.8;
    let mut best_arc: Option<(f64, Point2)> = None;
    for (_, seg) in doc.all_segments() {
        if seg.kind != SegmentKind::Arc {
            continue;
        }
        let Some(sc) = seg.ctrl else { continue };
        let (Some(a), Some(b), Some(c)) =
            (doc.point(seg.start), doc.point(seg.end), doc.point(sc))
        else {
            continue;
        };
        let Some(proj) = closest_point_on_arc(p, a, b, c) else {
            continue;
        };
        if !visible.contains(proj) && !visible.contains(a) && !visible.contains(b) {
            continue;
        }
        let d = distance(p, proj);
        if d <= arc_tol && best_arc.map_or(true, |(bd, _)| d < bd) {
            best_arc = Some((d, proj));
        }
    }
    if let Some((_, proj)) = best_arc {
        return (proj, guide(proj, SnapKind::Edge));
    }

    (p, None)
}

fn closest_point_on_segment(p: Point2, a: Point2, b: Point2) -> Point2 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    if len_sq == 0. {
        return a;
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len_sq).clamp(0., 1.);
    Point2::new(a.x + t * abx, a.y + t * aby)
}

fn closest_point_on_arc(p: Point2, a: Point2, b: Point2, c: Point2) -> Option<Point2> {
    use crate::editor::arc::circumcircle;
    let Some((o, r)) = circumcircle(a, b, c) else {
        return None;
    };
    let dx = p.x - o.x;
    let dy = p.y - o.y;
    let d = (dx * dx + dy * dy).sqrt();
    if d < 1e-9 {
        return None;
    }
    let proj = Point2::new(o.x + dx / d * r, o.y + dy / d * r);
    // Check if projection lies within the arc's angular interval (a -> b via c).
    let ang = |q: Point2| (q.y - o.y).atan2(q.x - o.x);
    let a0 = ang(a);
    let b0 = ang(b);
    let c0 = ang(c);
    let p0 = ang(proj);
    const TAU: f64 = std::f64::consts::TAU;
    let norm = |mut t: f64| {
        while t < 0. {
            t += TAU;
        }
        while t >= TAU {
            t -= TAU;
        }
        t
    };
    let s_pos = norm(b0 - a0);
    let in_pos = norm(c0 - a0) < s_pos + 1e-9;
    let sweep = if in_pos { s_pos } else { s_pos - TAU };
    let in_arc = if sweep >= 0. {
        norm(p0 - a0) <= sweep + 1e-7 && norm(p0 - a0) >= -1e-7
    } else {
        norm(a0 - p0) <= -sweep + 1e-7 && norm(a0 - p0) >= -1e-7
    };
    if in_arc {
        Some(proj)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_snap_locks_endpoint() {
        let mut doc = Document::new();
        let pid = doc.add_point(Point2::new(100., 100.));
        let _ = pid;
        let vis = Rect::from_points(Point2::new(0., 0.), Point2::new(500., 500.));
        let (p, g) = cursor_snap(&doc, 10., Point2::new(106., 97.), vis);
        assert_eq!(p, Point2::new(100., 100.));
        assert!(g.is_some());
    }

    #[test]
    fn cursor_snap_edge_body() {
        let mut doc = Document::new();
        let a = doc.add_point(Point2::new(0., 0.));
        let b = doc.add_point(Point2::new(200., 0.));
        doc.add_segment(a, b);
        let vis = Rect::from_points(Point2::new(-50., -50.), Point2::new(500., 500.));
        // 5 units above the edge body.
        let (p, g) = cursor_snap(&doc, 10., Point2::new(80., -5.), vis);
        assert_eq!(p, Point2::new(80., 0.));
        assert!(g.is_some());
    }
}
