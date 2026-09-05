use crate::core::document::Document;
use crate::core::geometry::Point2;
use crate::core::ids::SegmentId;

// Cubic Bezier math (docs/pen-tool.md section 24, locked for V1).
// B(t) = (1-t)^3 P0 + 3(1-t)^2 t P1 + 3(1-t) t^2 P2 + t^3 P3.

/// A point on the cubic at parameter t.
pub fn point(p0: Point2, p1: Point2, p2: Point2, p3: Point2, t: f64) -> Point2 {
    let s = 1. - t;
    Point2::new(
        s * s * s * p0.x + 3. * s * s * t * p1.x + 3. * s * t * t * p2.x + t * t * t * p3.x,
        s * s * s * p0.y + 3. * s * s * t * p1.y + 3. * s * t * t * p2.y + t * t * t * p3.y,
    )
}

/// Evenly spaced samples along the curve (endpoints included).
pub fn samples(p0: Point2, p1: Point2, p2: Point2, p3: Point2, n: usize) -> Vec<Point2> {
    let steps = n.max(2);
    (0..=steps).map(|k| point(p0, p1, p2, p3, k as f64 / steps as f64)).collect()
}

/// Sample count so the polyline chord error stays small at any zoom.
/// Scales with the control polygon length (screen space), like the arc
/// sampler it mirrors.
pub fn adaptive_samples(p0: Point2, p1: Point2, p2: Point2, p3: Point2, zoom: f64) -> usize {
    let poly = dist(p0, p1) + dist(p1, p2) + dist(p2, p3);
    ((poly * zoom / 8.0).ceil() as usize).clamp(8, 512)
}

/// Sampled polyline of a bezier segment for rendering/hit-testing.
pub fn segment_samples(doc: &Document, sid: SegmentId, n: usize) -> Option<Vec<Point2>> {
    let (p0, p1, p2, p3) = doc.bezier_geom(sid)?;
    Some(samples(p0, p1, p2, p3, n))
}

/// Parameter + point of the closest curve point to `p`, via dense sampling
/// plus refinement. Insertion splits exactly here.
pub fn param_of_closest(
    p: Point2,
    p0: Point2,
    p1: Point2,
    p2: Point2,
    p3: Point2,
) -> (f64, Point2) {
    const N: usize = 64;
    let mut best_t = 0.;
    let mut best_d = f64::MAX;
    for k in 0..=N {
        let t = k as f64 / N as f64;
        let q = point(p0, p1, p2, p3, t);
        let d = dist(p, q);
        if d < best_d {
            best_d = d;
            best_t = t;
        }
    }
    // Refine around the winner.
    let step = 1. / N as f64 / 8.;
    let mut t = best_t;
    for _ in 0..8 {
        let mut improved = false;
        for dt in [-step, step] {
            let nt = (t + dt).clamp(0., 1.);
            if dist(p, point(p0, p1, p2, p3, nt)) < dist(p, point(p0, p1, p2, p3, t)) {
                t = nt;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    (t, point(p0, p1, p2, p3, t))
}

/// Closest point on the curve to `p`. Exact enough for snapping.
pub fn closest_point(
    p: Point2,
    p0: Point2,
    p1: Point2,
    p2: Point2,
    p3: Point2,
) -> Point2 {
    param_of_closest(p, p0, p1, p2, p3).1
}

/// De Casteljau split at t: exact subdivision into two cubics sharing the
/// on-curve point. Returns ((left P0..P3), (right P0..P3)). Insertion
/// (docs/pen-tool.md section 15) uses this, so splitting never changes the
/// drawn shape by even a pixel.
#[allow(clippy::type_complexity)]
pub fn split(
    p0: Point2,
    p1: Point2,
    p2: Point2,
    p3: Point2,
    t: f64,
) -> ((Point2, Point2, Point2, Point2), (Point2, Point2, Point2, Point2)) {
    let lerp = |a: Point2, b: Point2| Point2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    let q0 = lerp(p0, p1);
    let q1 = lerp(p1, p2);
    let q2 = lerp(p2, p3);
    let r0 = lerp(q0, q1);
    let r1 = lerp(q1, q2);
    let s = lerp(r0, r1);
    ((p0, q0, r0, s), (s, r1, q2, p3))
}

fn dist(a: Point2, b: Point2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_exact() {
        let (p0, p1, p2, p3) = (
            Point2::new(0., 0.),
            Point2::new(10., 0.),
            Point2::new(20., 10.),
            Point2::new(30., 10.),
        );
        assert_eq!(point(p0, p1, p2, p3, 0.), p0);
        assert_eq!(point(p0, p1, p2, p3, 1.), p3);
    }

    #[test]
    fn split_preserves_shape() {
        let (p0, p1, p2, p3) = (
            Point2::new(0., 0.),
            Point2::new(10., 30.),
            Point2::new(40., -10.),
            Point2::new(50., 20.),
        );
        let ((l0, l1, l2, l3), (r0, r1, r2, r3)) = split(p0, p1, p2, p3, 0.4);
        // Junctions meet on the original curve.
        assert_eq!(l3, r0);
        assert_eq!(l3, point(p0, p1, p2, p3, 0.4));
        // Both halves re-trace the original.
        for k in 0..=10 {
            let t = k as f64 / 10.;
            let orig = point(p0, p1, p2, p3, t);
            let half = if t <= 0.4 {
                point(l0, l1, l2, l3, t / 0.4)
            } else {
                point(r0, r1, r2, r3, (t - 0.4) / 0.6)
            };
            assert!((orig.x - half.x).abs() < 1e-9 && (orig.y - half.y).abs() < 1e-9);
        }
    }

    #[test]
    fn straight_degenerates() {
        // Collapsed handles = straight chord.
        let (p0, p3) = (Point2::new(0., 0.), Point2::new(100., 0.));
        let m = point(p0, p0, p3, p3, 0.5);
        assert!((m.x - 50.).abs() < 1e-9 && m.y.abs() < 1e-9);
    }
}
