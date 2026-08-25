use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::PointId;

// Snapping: candidate locations exposed by geometry, best-match search,
// and visual guide descriptors. Pure functions over document + camera.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Edge,
}

// A visual snap connection: what locked onto what. Edge snaps carry the
// target's full span so rendering can trace it.
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
/// drags fluid — midpoints and edge spans apply only to precise placement
/// (shape tools), never while dragging geometry.
pub fn targets(doc: &Document, exclude: &[PointId], endpoints_only: bool) -> Vec<SnapTarget> {
    let mut out = Vec::new();
    for (pid, p) in doc.all_points() {
        if exclude.contains(&pid) {
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
    for (sid, seg) in doc.all_segments() {
        if seg.kind == SegmentKind::Ruler {
            // Rulers are measurement aids, not snap scaffolding.
            continue;
        }
        let Some((a, b)) = doc.segment_geom(sid) else { continue };
        let m = mid(a, b);
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

/// Best single correction for a point against all targets. Returns
/// (adjustment delta, guides). `coincident_only` demands BOTH axes hit —
/// used while dragging so a passing row/column alignment doesn't yank a
/// single axis (the "slight 90-degree snap").
pub fn best(
    doc: &Document,
    tol: f64,
    p: Point2,
    exclude: &[PointId],
    endpoints_only: bool,
    coincident_only: bool,
) -> (Point2, Vec<SnapGuide>) {
    let mut best: Option<(f64, f64, f64, bool, bool, SnapTarget)> = None;
    for tgt in targets(doc, exclude, endpoints_only) {
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
    let Some((_, dx, dy, hit_x, hit_y, tgt)) = best else {
        return (Point2::new(0., 0.), Vec::new());
    };
    let mut adj = Point2::new(0., 0.);
    let mut guides = Vec::new();
    if hit_x {
        adj.x = dx;
        guides.push(SnapGuide {
            vertical: true,
            from: p,
            to: Point2::new(tgt.x, p.y),
            kind: tgt.kind,
            span_is_x: tgt.span_is_x,
            span_lo: tgt.span_lo,
            span_hi: tgt.span_hi,
        });
    }
    if hit_y {
        adj.y = dy;
        guides.push(SnapGuide {
            vertical: false,
            from: p,
            to: Point2::new(p.x, tgt.y),
            kind: tgt.kind,
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
