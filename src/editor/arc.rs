use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::SegmentId;

// Circular-arc math. An Arc segment passes through start -> ctrl -> end
// (the unique circumcircle); the ctrl point is a REAL document point.

/// Circumcenter and radius of the circle through a, b, c.
pub fn circumcircle(a: Point2, b: Point2, c: Point2) -> Option<(Point2, f64)> {
    let d = 2. * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    // Scale the degeneracy threshold with the triangle coordinates. A fixed
    // epsilon incorrectly treats large, valid arcs as collinear and permits
    // tiny near-collinear arcs to produce enormous unstable circles.
    let scale = [
        (a.x - b.x).abs(), (a.y - b.y).abs(),
        (b.x - c.x).abs(), (b.y - c.y).abs(),
        (c.x - a.x).abs(), (c.y - a.y).abs(),
    ]
    .into_iter()
    .fold(1.0f64, f64::max);
    if d.abs() < 1e-12 * scale * scale {
        return None;
    }
    let a2 = a.x * a.x + a.y * a.y;
    let b2 = b.x * b.x + b.y * b.y;
    let c2 = c.x * c.x + c.y * c.y;
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = Point2::new(ux, uy);
    let r = ((a.x - ux).powi(2) + (a.y - uy).powi(2)).sqrt();
    Some((center, r))
}

/// Sampled points along the arc from a to b PASSING THROUGH c.
pub fn samples_through(a: Point2, b: Point2, c: Point2, n: usize) -> Vec<Point2> {
    let Some((o, _)) = circumcircle(a, b, c) else {
        // Degenerate (collinear): fall back to chord via c.
        return vec![a, c, b];
    };
    let ang = |p: Point2| (p.y - o.y).atan2(p.x - o.x);
    let a0 = ang(a);
    let b0 = ang(b);
    let c0 = ang(c);
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
    // Pick the sweep that actually contains c: the direction where c's
    // midpoint is closest to c.
    let s_pos = norm(b0 - a0);
    let s_neg = s_pos - TAU; // negative equivalent
    let r = ((a.x - o.x).powi(2) + (a.y - o.y).powi(2)).sqrt();
    let mid_dist = |sweep: f64| {
        let mid_ang = a0 + sweep / 2.;
        let mid = Point2::new(o.x + r * mid_ang.cos(), o.y + r * mid_ang.sin());
        let dx = mid.x - c.x;
        let dy = mid.y - c.y;
        dx * dx + dy * dy
    };
    // Two candidates: the short way and the long way — one contains c.
    // Test both orderings of a/b vs c: we need the direction where the
    // midpoint of the arc is nearest to c.
    let d_pos = {
        // a -> b positive sweep
        let d1 = mid_dist(s_pos);
        // Also need to test if c lies exactly on that sweep (not just midpoint proximity).
        // Verify c is within (a0, a0+s_pos): check norm(c0-a0) < s_pos
        let in_pos = norm(c0 - a0) < s_pos + 1e-9 && norm(c0 - a0) > -1e-9;
        if in_pos { d1 } else { f64::MAX }
    };
    let d_neg = {
        let d2 = mid_dist(s_neg);
        let in_neg = norm(a0 - c0) < -s_neg + 1e-9;
        if in_neg { d2 } else { f64::MAX }
    };
    let sweep = if d_pos <= d_neg { s_pos } else { s_neg };
    let a1 = a0 + sweep;
    let steps = n.max(2);
    let mut out = Vec::with_capacity(steps + 1);
    for k in 0..=steps {
        let t = k as f64 / steps as f64;
        let theta = a0 + (a1 - a0) * t;
        out.push(Point2::new(o.x + r * theta.cos(), o.y + r * theta.sin()));
    }
    out
}

/// The COMPLEMENTARY arc — the opposite side of the circle between a and
/// b, bulging AWAY from c. Together with `samples_through(a,b,c)` this
/// forms the full circle. Always runs a -> b via the side NOT containing c.
pub fn complement_samples(a: Point2, b: Point2, c: Point2, n: usize) -> Vec<Point2> {
    let Some((o, _)) = circumcircle(a, b, c) else {
        return vec![a, b];
    };
    let ang = |p: Point2| (p.y - o.y).atan2(p.x - o.x);
    let a0 = ang(a);
    let b0 = ang(b);
    let c0 = ang(c);
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
    let s_neg = s_pos - TAU;
    let in_pos = norm(c0 - a0) < s_pos + 1e-9;
    let sweep = if in_pos { s_pos } else { s_neg };
    // Opposite side: same endpoints, opposite direction.
    let comp_sweep = if sweep >= 0. { sweep - TAU } else { sweep + TAU };
    let steps = n.max(2);
    let r = ((a.x - o.x).powi(2) + (a.y - o.y).powi(2)).sqrt();
    let mut out = Vec::with_capacity(steps + 1);
    for k in 0..=steps {
        let t = k as f64 / steps as f64;
        let theta = a0 + comp_sweep * t;
        out.push(Point2::new(o.x + r * theta.cos(), o.y + r * theta.sin()));
    }
    out
}

/// True when the arc's endpoints are glued by a Coincident constraint,
/// meaning the complementary arc fills the gap and forms a full circle.
pub fn is_complete(doc: &Document, sid: SegmentId) -> bool {
    let Some(seg) = doc.segment(sid) else { return false };
    doc.constraints.iter().any(|c| {
        c.kind == crate::core::constraints::ConstraintKind::Coincident
            && ((c.a == seg.start && c.b == seg.end) || (c.a == seg.end && c.b == seg.start))
    })
}

/// The point at the middle of the arc's SWEEP — ON the curve. (The chord
/// midpoint of an arc floats in empty space off the bend, so snap targets
/// must use this instead.)
pub fn curve_midpoint(a: Point2, b: Point2, c: Point2) -> Option<Point2> {
    const TAU: f64 = std::f64::consts::TAU;
    let (o, r) = circumcircle(a, b, c)?;
    let ang = |p: Point2| (p.y - o.y).atan2(p.x - o.x);
    let norm = |mut t: f64| {
        while t < 0. {
            t += TAU;
        }
        while t >= TAU {
            t -= TAU;
        }
        t
    };
    let a0 = ang(a);
    let s_pos = norm(ang(b) - a0);
    let sweep = if norm(ang(c) - a0) < s_pos { s_pos } else { s_pos - TAU };
    let m0 = a0 + sweep / 2.;
    Some(Point2::new(o.x + r * m0.cos(), o.y + r * m0.sin()))
}

/// SHIFT constraint for the arc bulge: snap the sweep of the arc
/// a -> b through c to the nearest 90 degrees (perfect quarter, half, or
/// three-quarter arc), preserving which side it bends to. The third point
/// is the radial projection of c onto the snapped arc, clamped inside its
/// span — so the bend follows the mouse along the arc instead of jumping
/// to the apex. None when degenerate (collinear).
pub fn snap_sweep(a: Point2, b: Point2, c: Point2) -> Option<Point2> {
    const QUARTER: f64 = std::f64::consts::FRAC_PI_2;
    const TAU: f64 = std::f64::consts::TAU;
    // Current signed sweep (which side c bends to).
    let (o0, _) = circumcircle(a, b, c)?;
    let ang_wrt = |p: Point2, o: Point2| (p.y - o.y).atan2(p.x - o.x);
    let norm = |mut t: f64| {
        while t < 0. {
            t += TAU;
        }
        while t >= TAU {
            t -= TAU;
        }
        t
    };
    let a00 = ang_wrt(a, o0);
    let s_pos = norm(ang_wrt(b, o0) - a00);
    let sweep = if norm(ang_wrt(c, o0) - a00) < s_pos { s_pos } else { s_pos - TAU };
    // Nearest multiple of 90°, preserving the bend direction; never 0 and
    // never a full turn (a 3-point arc can't express either).
    let mut steps = (sweep / QUARTER).round();
    if steps.abs() < 1. {
        steps = sweep.signum();
    }
    steps = steps.clamp(-3., 3.);
    let s = steps * QUARTER;
    // The circle through a and b with EXACTLY that sweep (the chord is
    // fixed, so the snapped sweep fully determines center and radius).
    let chx = b.x - a.x;
    let chy = b.y - a.y;
    let ch = (chx * chx + chy * chy).sqrt();
    if ch < 1e-9 {
        return None;
    }
    let r = ch / (2. * (s.abs() / 2.).sin());
    let d = r * (s / 2.).cos();
    let mid = Point2::new((a.x + b.x) / 2., (a.y + b.y) / 2.);
    // Signed offset along the chord's LEFT normal — flips sides past 180°.
    let o = Point2::new(mid.x - chy / ch * d, mid.y + chx / ch * d);
    let a0 = ang_wrt(a, o);
    // Radial projection of the cursor onto the snapped arc, clamped inside
    // the span (a little shy of the endpoints so the arc stays valid).
    let rel = if sweep >= 0. {
        norm(ang_wrt(c, o) - a0)
    } else {
        norm(ang_wrt(c, o) - a0) - TAU
    };
    const EPS: f64 = 1e-3;
    let t = rel.clamp(EPS, s.abs() - EPS) * s.signum();
    let th = a0 + t;
    Some(Point2::new(o.x + r * th.cos(), o.y + r * th.sin()))
}

/// Sample count so the polyline approximation's chord error stays under
/// ~0.5 screen px regardless of zoom. `zoom` = camera zoom.
pub fn adaptive_samples(a: Point2, b: Point2, c: Point2, zoom: f64) -> usize {
    let Some((o, r)) = circumcircle(a, b, c) else {
        return 8;
    };
    let ang = |p: Point2| (p.y - o.y).atan2(p.x - o.x);
    let a0 = ang(a);
    let b0 = ang(b);
    let c0 = ang(c);
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
    let s_neg = s_pos - TAU;
    let sweep = if norm(c0 - a0) <= s_pos + 1e-9 { s_pos } else { s_neg };
    let rs = r * zoom; // screen-space radius
    // Sagitta per segment ≈ R·(θ/2)²/2; keeping it < 0.5px gives
    // N > |sweep|·sqrt(R)/2.
    let n = ((sweep.abs() * rs.sqrt()) / 2.0).ceil() as usize;
    // Keep tessellation bounded during extreme zoom or near-collinear arcs.
    // Beyond this point the curve is visually sub-pixel in practice, while
    // the allocation and paint cost continues to grow linearly.
    n.clamp(16, 1024)
}

/// Sampled polyline of an arc segment for rendering/hit-testing.
pub fn segment_samples(doc: &Document, sid: SegmentId, n: usize) -> Option<Vec<Point2>> {
    let seg = doc.segment(sid)?;
    if seg.kind != SegmentKind::Arc {
        return None;
    }
    let a = doc.point(seg.start)?;
    let b = doc.point(seg.end)?;
    let c = doc.point(seg.ctrl?)?;
    Some(samples_through(a, b, c, n))
}

#[cfg(test)]
mod tests {
    use super::{circumcircle, samples_through};
    use crate::core::geometry::Point2;

    #[test]
    fn circumcircle_fits_right_triangle() {
        let circle = circumcircle(
            Point2::new(0., 0.),
            Point2::new(2., 0.),
            Point2::new(0., 2.),
        )
        .expect("non-collinear points have a circumcircle");
        assert!((circle.0.x - 1.).abs() < 1e-9);
        assert!((circle.0.y - 1.).abs() < 1e-9);
        assert!((circle.1 - 2f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn collinear_arc_has_no_circle() {
        assert!(circumcircle(
            Point2::new(0., 0.),
            Point2::new(1., 0.),
            Point2::new(2., 0.),
        )
        .is_none());
    }

    #[test]
    fn degenerate_arc_falls_back_to_chord() {
        let samples = samples_through(
            Point2::new(0., 0.),
            Point2::new(2., 0.),
            Point2::new(1., 0.),
            8,
        );
        assert_eq!(samples, vec![Point2::new(0., 0.), Point2::new(1., 0.), Point2::new(2., 0.)]);
    }
}
