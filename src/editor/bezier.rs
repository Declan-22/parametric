use crate::core::document::Document;
use crate::core::geometry::Point2;
use crate::core::ids::SegmentId;

// Cubic bezier math. Handles are real document points (free — the solver
// only constrains endpoints); sampling uses the shared render-cache path
// so hundreds of spans stay at 120fps (zoom-bucketed, viewport-culled).

pub fn eval(p0: Point2, c1: Point2, c2: Point2, p1: Point2, t: f64) -> Point2 {
    let u = 1. - t;
    Point2::new(
        u * u * u * p0.x + 3. * u * u * t * c1.x + 3. * u * t * t * c2.x + t * t * t * p1.x,
        u * u * u * p0.y + 3. * u * u * t * c1.y + 3. * u * t * t * c2.y + t * t * t * p1.y,
    )
}

/// End tangent direction (unit). `at_start`: p0->c1, else p1->c2.
/// Falls back to the chord when handles coincide with endpoints.
pub fn end_tangent(p0: Point2, c1: Point2, c2: Point2, p1: Point2, at_start: bool) -> (f64, f64) {
    let (mut dx, mut dy) = if at_start { (c1.x - p0.x, c1.y - p0.y) } else { (p1.x - c2.x, p1.y - c2.y) };
    if dx == 0. && dy == 0. {
        dx = p1.x - p0.x;
        dy = p1.y - p0.y;
    }
    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
    (dx / l, dy / l)
}

/// Adaptive sample count from control-polygon length in doc units + zoom.
pub fn adaptive_samples(p0: Point2, c1: Point2, c2: Point2, p1: Point2, zoom: f64) -> usize {
    let poly = dist(p0, c1) + dist(c1, c2) + dist(c2, p1);
    let px = poly * zoom;
    // ~1 sample per 4px, clamped for perf.
    (px / 4.).ceil().clamp(8., 128.) as usize
}

pub fn samples(p0: Point2, c1: Point2, c2: Point2, p1: Point2, n: usize) -> Vec<Point2> {
    (0..=n).map(|k| eval(p0, c1, c2, p1, k as f64 / n as f64)).collect()
}

pub fn segment_samples(doc: &Document, sid: SegmentId, n: usize) -> Option<Vec<Point2>> {
    let s = doc.segment(sid)?;
    if s.kind != crate::core::document::SegmentKind::Bezier {
        return None;
    }
    let (h1, h2) = s.bezier_handles();
    Some(samples(
        doc.point(s.start)?,
        doc.point(h1?)?,
        doc.point(h2?)?,
        doc.point(s.end)?,
        n,
    ))
}

/// Polyline arc-length of the sampled curve.
pub fn arc_length(p0: Point2, c1: Point2, c2: Point2, p1: Point2) -> f64 {
    let pts = samples(p0, c1, c2, p1, 64);
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

pub fn segment_length(doc: &Document, sid: SegmentId) -> Option<f64> {
    let s = doc.segment(sid)?;
    let (h1, h2) = s.bezier_handles();
    Some(arc_length(
        doc.point(s.start)?,
        doc.point(h1?)?,
        doc.point(h2?)?,
        doc.point(s.end)?,
    ))
}

/// Nearest sample to `at` + its tangent. Used for tangent snapping and
/// curve hit-testing without per-frame allocation pressure (caller caps n).
pub fn nearest_on_curve(
    p0: Point2,
    c1: Point2,
    c2: Point2,
    p1: Point2,
    at: Point2,
    n: usize,
) -> (Point2, f64, (f64, f64)) {
    let pts = samples(p0, c1, c2, p1, n);
    let mut best = (pts[0], f64::MAX, 0usize);
    for (i, p) in pts.iter().enumerate() {
        let d = dist(*p, at);
        if d < best.1 {
            best = (*p, d, i);
        }
    }
    // Tangent from neighbors.
    let a = pts[best.2.saturating_sub(1)];
    let b = pts[(best.2 + 1).min(pts.len() - 1)];
    let mut dx = b.x - a.x;
    let mut dy = b.y - a.y;
    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
    dx /= l;
    dy /= l;
    (best.0, best.1, (dx, dy))
}

fn dist(a: Point2, b: Point2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}
