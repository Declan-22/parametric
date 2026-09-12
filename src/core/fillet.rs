//! Parametric fillets. Source segments remain untouched; this module derives
//! tangent points and the replacement arc from their current geometry.
//!
//! Convention: every direction below is an *away-ray* — a unit vector at the
//! shared corner pointing ALONG its segment AWAY from the corner. Tangent
//! points, the bisector center, and the control point all derive from the
//! two away-rays, so the fillet always lands on the wedge between the
//! segments instead of mirroring outside the shape.
use crate::core::document::{Document, Segment, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::{PointId, SegmentId};

/// Which wedge of the corner the fillet rounds. `Inner` is the wedge
/// between the away-rays (the default: inside rounds on convex corners).
/// `Outer` mirrors across the corner — tangent points sit on the line
/// extensions past the corner and the arc bulges into the reflex wedge,
/// which is the useful side for notches (and an outside round on convex
/// corners). Persisted as 0/1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilletSide {
    Inner,
    Outer,
}

impl FilletSide {
    pub fn as_str(self) -> &'static str {
        match self {
            FilletSide::Inner => "Inner",
            FilletSide::Outer => "Outer",
        }
    }

    pub fn flip(self) -> Self {
        match self {
            FilletSide::Inner => FilletSide::Outer,
            FilletSide::Outer => FilletSide::Inner,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fillet {
    pub first: SegmentId,
    pub second: SegmentId,
    pub corner: PointId,
    pub radius: f64,
    pub side: FilletSide,
    pub arc: Option<SegmentId>,
    pub first_tangent: Option<PointId>,
    pub second_tangent: Option<PointId>,
    pub center: Option<PointId>,
    pub control: Option<PointId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvaluatedFillet {
    pub corner: Point2,
    pub first_tangent: Point2,
    pub second_tangent: Point2,
    pub center: Point2,
    pub control: Point2,
    pub radius: f64,
}

const MIN_RADIUS: f64 = 0.01;
/// Tangent points stay this far inside the segment far-ends so the derived
/// arc never touches an endpoint (which would collapse its constraints).
const END_MARGIN: f64 = 1.0;

fn norm(x: f64, y: f64) -> Option<(f64, f64)> {
    let l = (x * x + y * y).sqrt();
    (l > 1e-9).then_some((x / l, y / l))
}

/// Live corner position for line-line pairs (exact intersection).
/// Used by gestures that need the corner axis without a stored point.
pub(crate) fn line_corner(doc: &Document, first: SegmentId, second: SegmentId) -> Option<Point2> {
    let (a, b) = (doc.segment(first)?, doc.segment(second)?);
    if !matches!(a.kind, SegmentKind::Line) || !matches!(b.kind, SegmentKind::Line) {
        return None;
    }
    intersect_lines(
        doc.point(a.start)?,
        doc.point(a.end)?,
        doc.point(b.start)?,
        doc.point(b.end)?,
    )
}

/// Intersection of two infinite lines (each through p→q). None when
/// parallel or degenerate. Used to recover subdivided corners (whose
/// point was consumed) live from current geometry.
fn intersect_lines(a1: Point2, a2: Point2, b1: Point2, b2: Point2) -> Option<Point2> {
    let (ux, uy) = (a2.x - a1.x, a2.y - a1.y);
    let (vx, vy) = (b2.x - b1.x, b2.y - b1.y);
    let (lu, lv) = (
        (ux * ux + uy * uy).sqrt(),
        (vx * vx + vy * vy).sqrt(),
    );
    if lu < 1e-9 || lv < 1e-9 {
        return None;
    }
    let denom = ux * vy - uy * vx;
    // Scale-free parallelism test (|sin θ| between the directions).
    if (denom / (lu * lv)).abs() < 1e-6 {
        return None;
    }
    let (wx, wy) = (b1.x - a1.x, b1.y - a1.y);
    let t = (wx * vy - wy * vx) / denom;
    Some(Point2::new(a1.x + ux * t, a1.y + uy * t))
}

/// Unit direction along `seg` AWAY from `corner` (toward the far end).
/// The corner-side end is whichever endpoint sits nearer the corner
/// position (exactly the corner for legacy topology). Lines use the far
/// endpoint; arcs resolve the circle-travel winding explicitly; beziers
/// use the end-handle ray. None on degenerate geometry (zero length, bad
/// circle, missing handles).
fn away_ray(doc: &Document, seg: Segment, corner: Point2) -> Option<(f64, f64)> {
    let (sp, ep) = (doc.point(seg.start)?, doc.point(seg.end)?);
    let ds = (sp.x - corner.x).powi(2) + (sp.y - corner.y).powi(2);
    let de = (ep.x - corner.x).powi(2) + (ep.y - corner.y).powi(2);
    let at_start = ds <= de;
    let anchor = if at_start { sp } else { ep };
    match seg.kind {
        SegmentKind::Line => {
            let far = if at_start { ep } else { sp };
            norm(far.x - anchor.x, far.y - anchor.y)
        }
        SegmentKind::Arc => {
            let ctrl = doc.point(seg.ctrl?)?;
            let (o, _) = crate::editor::arc::circumcircle(sp, ep, ctrl)?;
            // Winding of start -> ctrl travel: sign of the signed angle
            // from (start - O) to (ctrl - O). The travel tangent is the
            // radial rotated +90° times that sign — no orientation guess,
            // so arcs fillet correctly whichever way they wind.
            let (rsx, rsy) = (sp.x - o.x, sp.y - o.y);
            let (rcx, rcy) = (ctrl.x - o.x, ctrl.y - o.y);
            let phi = (rsx * rcy - rsy * rcx).atan2(rsx * rcx + rsy * rcy);
            let s = phi.signum();
            let (rx, ry) = (anchor.x - o.x, anchor.y - o.y);
            let rl = (rx * rx + ry * ry).sqrt();
            if rl < 1e-9 {
                return None;
            }
            let (tx, ty) = (-ry / rl, rx / rl);
            let travel = if s == 0.0 {
                // Ctrl diametrically opposed to start: fall back to the
                // chord (tangent-chord theorem keeps the sense right for
                // sub-180° spans).
                let (cx, cy) = if at_start {
                    (ctrl.x - anchor.x, ctrl.y - anchor.y)
                } else {
                    (anchor.x - ctrl.x, anchor.y - ctrl.y)
                };
                if tx * cx + ty * cy >= 0.0 {
                    (tx, ty)
                } else {
                    (-tx, -ty)
                }
            } else {
                (tx * s, ty * s)
            };
            // Travel leaves the corner at the start but arrives into it at
            // the end — flip for the end case.
            Some(if at_start {
                travel
            } else {
                (-travel.0, -travel.1)
            })
        }
        SegmentKind::Bezier => {
            let (h1, h2) = seg.bezier_handles();
            // end_tangent already points away from the queried endpoint.
            Some(crate::editor::bezier::end_tangent(
                doc.point(seg.start)?,
                doc.point(h1?)?,
                doc.point(h2?)?,
                doc.point(seg.end)?,
                at_start,
            ))
        }
        SegmentKind::Ruler => None,
    }
}

/// Shared basis: away-rays, included angle, shortest corner→far-end chord,
/// and the corner position itself. The corner is the stored point while it
/// still terminates both sources (legacy topology); once subdivision
/// rewires the sources to tangent points, it is recovered live as the
/// line-line intersection (exact, never stale) — curve-involved pairs
/// never subdivide, so the intersection path only runs on lines.
fn basis(
    doc: &Document,
    first: SegmentId,
    second: SegmentId,
    corner: PointId,
) -> Option<((f64, f64), (f64, f64), f64, f64, Point2)> {
    let a = doc.segment(first)?;
    let b = doc.segment(second)?;
    if !matches!(
        a.kind,
        SegmentKind::Line | SegmentKind::Arc | SegmentKind::Bezier
    ) || !matches!(
        b.kind,
        SegmentKind::Line | SegmentKind::Arc | SegmentKind::Bezier
    ) {
        return None;
    }
    let legacy = doc.point(corner).filter(|_| {
        [a.start, a.end].contains(&corner) && [b.start, b.end].contains(&corner)
    });
    let corner_pos = match legacy {
        Some(p) => p,
        None => {
            if !matches!(a.kind, SegmentKind::Line) || !matches!(b.kind, SegmentKind::Line) {
                return None;
            }
            let (a1, a2) = (doc.point(a.start)?, doc.point(a.end)?);
            let (b1, b2) = (doc.point(b.start)?, doc.point(b.end)?);
            intersect_lines(a1, a2, b1, b2)?
        }
    };
    let u = away_ray(doc, a, corner_pos)?;
    let v = away_ray(doc, b, corner_pos)?;
    let theta = (u.0 * v.0 + u.1 * v.1).clamp(-0.999999, 0.999999).acos();
    if theta < 1e-4 || theta > std::f64::consts::PI - 1e-4 {
        return None;
    }
    let chord = |s: Segment| {
        // Far end = whichever endpoint sits farther from the corner
        // position (exactly the non-corner end for legacy topology).
        let (sp, ep) = (doc.point(s.start)?, doc.point(s.end)?);
        let ds = (sp.x - corner_pos.x).powi(2) + (sp.y - corner_pos.y).powi(2);
        let de = (ep.x - corner_pos.x).powi(2) + (ep.y - corner_pos.y).powi(2);
        let p = if ds >= de { sp } else { ep };
        Some(
            ((p.x - corner_pos.x).powi(2) + (p.y - corner_pos.y).powi(2)).sqrt(),
        )
    };
    Some((u, v, theta, chord(a)?.min(chord(b)?), corner_pos))
}

impl Fillet {
    /// Largest radius that stays proportionate to the corner: the tangent
    /// distance fits inside the shortest far-end chord (on the segments
    /// for Inner, on the extensions for Outer). None when the pair can't
    /// take a fillet at any radius — wrong kinds, no shared corner, or a
    /// degenerate (hairline/straight) angle.
    pub fn max_radius(&self, doc: &Document) -> Option<f64> {
        let (_, _, theta, min_chord, _) = basis(doc, self.first, self.second, self.corner)?;
        let max = (min_chord - END_MARGIN) * (theta / 2.).tan();
        (max >= MIN_RADIUS).then_some(max)
    }

    pub fn evaluate(&self, doc: &Document) -> Option<EvaluatedFillet> {
        if self.radius < MIN_RADIUS {
            return None;
        }
        let ((ux, uy), (vx, vy), theta, min_chord, corner) =
            basis(doc, self.first, self.second, self.corner)?;
        let d = self.radius / (theta / 2.).tan();
        // Oversize radii would throw tangent points past the far ends
        // (outside the shape for Inner, disproportionate for Outer) —
        // reject instead of emitting garbage.
        if d > min_chord - END_MARGIN {
            return None;
        }
        // Outer mirrors every offset across the corner: tangent points
        // land on the line extensions past it and the arc bulges into the
        // reflex wedge. Tangency and radius hold by the same algebra (the
        // circle through the mirrored tangent points is unique), and the
        // solver's point-on-line equations are infinite-line, so the
        // derived constraints keep working off-segment.
        let s = match self.side {
            FilletSide::Inner => 1.,
            FilletSide::Outer => -1.,
        };
        let first_tangent = Point2::new(corner.x + s * ux * d, corner.y + s * uy * d);
        let second_tangent = Point2::new(corner.x + s * vx * d, corner.y + s * vy * d);
        let (bx, by) = (ux + vx, uy + vy);
        let bl = (bx * bx + by * by).sqrt();
        let center = Point2::new(
            corner.x + s * bx / bl * self.radius / (theta / 2.).sin(),
            corner.y + s * by / bl * self.radius / (theta / 2.).sin(),
        );
        let (rx, ry) = (
            first_tangent.x + second_tangent.x - 2. * center.x,
            first_tangent.y + second_tangent.y - 2. * center.y,
        );
        let rl = (rx * rx + ry * ry).sqrt().max(1e-9);
        // The control rides the corner side of the center for Inner and
        // the far side for Outer (the arc bulges away from the corner).
        let control = Point2::new(
            center.x + s * rx / rl * self.radius,
            center.y + s * ry / rl * self.radius,
        );
        Some(EvaluatedFillet {
            corner,
            first_tangent,
            second_tangent,
            center,
            control,
            radius: self.radius,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::Document;
    use crate::core::geometry::Point2;

    fn line_doc() -> (Document, PointId, PointId, PointId) {
        let mut d = Document::new();
        let c = d.add_point(Point2::new(0., 0.));
        let a = d.add_point(Point2::new(-100., 0.));
        let b = d.add_point(Point2::new(0., 100.));
        (d, a, c, b)
    }

    fn probe(first: SegmentId, second: SegmentId, corner: PointId) -> Fillet {
        Fillet {
            first,
            second,
            corner,
            radius: 10.,
            side: FilletSide::Inner,
            arc: None,
            first_tangent: None,
            second_tangent: None,
            center: None,
            control: None,
        }
    }

    #[test]
    fn evaluates_a_right_angle() {
        let (mut d, a, c, b) = line_doc();
        let s1 = d.add_segment(a, c);
        let s2 = d.add_segment(c, b);
        let g = probe(s1, s2, c).evaluate(&d).unwrap();
        assert!((g.first_tangent.x + 10.).abs() < 1e-6 && g.first_tangent.y.abs() < 1e-6);
        assert!(g.second_tangent.x.abs() < 1e-6 && (g.second_tangent.y - 10.).abs() < 1e-6);
        assert!((g.radius - 10.).abs() < 1e-6);
    }

    /// The corner-at-end orientation used to negate one away-ray, throwing
    /// the fillet outside the shape. All four start/end combinations must
    /// agree on the same wedge.
    #[test]
    fn right_angle_all_segment_orientations() {
        for (flip_a, flip_b) in [(false, false), (true, false), (false, true), (true, true)] {
            let (mut d, a, c, b) = line_doc();
            let s1 = d.add_segment(if flip_a { c } else { a }, if flip_a { a } else { c });
            let s2 = d.add_segment(if flip_b { b } else { c }, if flip_b { c } else { b });
            let g = probe(s1, s2, c).evaluate(&d).unwrap();
            assert!(
                (g.first_tangent.x + 10.).abs() < 1e-6 && g.first_tangent.y.abs() < 1e-6,
                "flip_a={flip_a} flip_b={flip_b}: first tangent {g:?}"
            );
            assert!(
                g.second_tangent.x.abs() < 1e-6 && (g.second_tangent.y - 10.).abs() < 1e-6,
                "flip_a={flip_a} flip_b={flip_b}: second tangent {g:?}"
            );
            assert!(
                (g.center.x + 10.).abs() < 1e-6 && (g.center.y - 10.).abs() < 1e-6,
                "flip_a={flip_a} flip_b={flip_b}: center {g:?}"
            );
        }
    }

    /// Outer mirrors across the corner: tangent points on the extensions,
    /// arc bulging into the reflex wedge.
    #[test]
    fn outer_side_mirrors_across_corner() {
        let (mut d, a, c, b) = line_doc();
        let s1 = d.add_segment(a, c);
        let s2 = d.add_segment(c, b);
        let mut f = probe(s1, s2, c);
        f.side = FilletSide::Outer;
        let g = f.evaluate(&d).unwrap();
        assert!((g.first_tangent.x - 10.).abs() < 1e-6 && g.first_tangent.y.abs() < 1e-6);
        assert!(g.second_tangent.x.abs() < 1e-6 && (g.second_tangent.y + 10.).abs() < 1e-6);
        assert!((g.center.x - 10.).abs() < 1e-6 && (g.center.y + 10.).abs() < 1e-6);
        // Radius and tangency hold exactly on the mirrored geometry.
        let r1 = ((g.first_tangent.x - g.center.x).powi(2)
            + (g.first_tangent.y - g.center.y).powi(2))
        .sqrt();
        assert!((r1 - 10.).abs() < 1e-6);
    }

    /// Subdivided topology: sources rewired to tangent points, corner id
    /// dead — evaluation recovers the corner live from the line
    /// intersection and reproduces the fillet exactly.
    #[test]
    fn subdivided_topology_evaluates_from_intersection() {
        let mut d = Document::new();
        // Trimmed rectangle edges: A→T1 and T2→B, ex-corner (100, 0).
        let a = d.add_point(Point2::new(0., 0.));
        let t1 = d.add_point(Point2::new(90., 0.));
        let t2 = d.add_point(Point2::new(100., 10.));
        let b = d.add_point(Point2::new(100., 100.));
        let s1 = d.add_segment(a, t1);
        let s2 = d.add_segment(t2, b);
        let dead = PointId { idx: 999, generation: 0 };
        let mut f = probe(s1, s2, dead);
        f.radius = 10.;
        let g = f.evaluate(&d).unwrap();
        assert!((g.first_tangent.x - 90.).abs() < 1e-6 && g.first_tangent.y.abs() < 1e-6);
        assert!((g.second_tangent.x - 100.).abs() < 1e-6 && (g.second_tangent.y - 10.).abs() < 1e-6);
        assert!((g.center.x - 90.).abs() < 1e-6 && (g.center.y - 10.).abs() < 1e-6);
        // max_radius measures from the recovered corner (chords 100/90).
        let max = f.max_radius(&d).unwrap();
        assert!((max - 89.).abs() < 1e-6);
    }

    #[test]
    fn acute_angle_tangent_distance() {
        let mut d = Document::new();
        let c = d.add_point(Point2::new(0., 0.));
        let a = d.add_point(Point2::new(100., 0.));
        // 60° ray: (cos60, sin60) * 100.
        let b = d.add_point(Point2::new(50., 86.60254037844386));
        let s1 = d.add_segment(c, a);
        let s2 = d.add_segment(c, b);
        let g = probe(s1, s2, c).evaluate(&d).unwrap();
        // d = r / tan(30°) ≈ 17.32 along each ray.
        let dist = |p: Point2| (p.x.powi(2) + p.y.powi(2)).sqrt();
        assert!((dist(g.first_tangent) - 17.32050807568877).abs() < 1e-6);
        assert!((dist(g.second_tangent) - 17.32050807568877).abs() < 1e-6);
        // Center rides the bisector at r / sin(30°) = 20.
        assert!((dist(g.center) - 20.).abs() < 1e-6);
    }

    #[test]
    fn obtuse_angle_tangent_distance() {
        let mut d = Document::new();
        let c = d.add_point(Point2::new(0., 0.));
        let a = d.add_point(Point2::new(100., 0.));
        // 120° ray: (cos120, sin120) * 100.
        let b = d.add_point(Point2::new(-50., 86.60254037844386));
        let s1 = d.add_segment(c, a);
        let s2 = d.add_segment(c, b);
        let g = probe(s1, s2, c).evaluate(&d).unwrap();
        // d = r / tan(60°) ≈ 5.77; center at r / sin(60°) ≈ 11.55.
        let dist = |p: Point2| (p.x.powi(2) + p.y.powi(2)).sqrt();
        assert!((dist(g.first_tangent) - 5.773502691896258).abs() < 1e-6);
        assert!((dist(g.second_tangent) - 5.773502691896258).abs() < 1e-6);
        assert!((dist(g.center) - 11.547005383792516).abs() < 1e-6);
    }

    #[test]
    fn rejects_degenerate_angles() {
        let mut d = Document::new();
        let c = d.add_point(Point2::new(0., 0.));
        let a = d.add_point(Point2::new(-100., 0.));
        let e = d.add_point(Point2::new(100., 0.));
        let s1 = d.add_segment(a, c);
        let s2 = d.add_segment(c, e);
        // Straight (180°) continuation: no wedge to fillet.
        assert!(probe(s1, s2, c).evaluate(&d).is_none());
        // Disjoint pair: no shared corner.
        let q = d.add_point(Point2::new(0., 100.));
        let s3 = d.add_segment(c, q);
        assert!(probe(s1, s3, e).evaluate(&d).is_none());
    }

    #[test]
    fn oversize_radius_rejected_and_max_reported() {
        let (mut d, a, c, b) = line_doc();
        let s1 = d.add_segment(a, c);
        let s2 = d.add_segment(c, b);
        // Chords are 100; margin eats 1 → max = 99 * tan(45°) = 99.
        let max = probe(s1, s2, c).max_radius(&d).unwrap();
        assert!((max - 99.).abs() < 1e-6);
        let mut big = probe(s1, s2, c);
        big.radius = 99.5;
        assert!(big.evaluate(&d).is_none());
        big.radius = 99.0;
        assert!(big.evaluate(&d).is_some());
    }

    #[test]
    fn line_arc_corner_uses_arc_travel_direction() {
        let mut d = Document::new();
        // Quarter circle, center origin r=50: (50,0) -> (0,50).
        let o = d.add_point(Point2::new(0., 0.));
        let c = d.add_point(Point2::new(50., 0.));
        let m = d.add_point(Point2::new(35.35533905932738, 35.35533905932738));
        let e = d.add_point(Point2::new(0., 50.));
        let arc = d.add_arc_segment(c, m, e, o);
        // Line leaving the shared corner east.
        let tip = d.add_point(Point2::new(100., 0.));
        let line = d.add_segment(c, tip);
        let g = probe(line, arc, c).evaluate(&d).unwrap();
        assert!((g.first_tangent.x - 60.).abs() < 1e-6 && g.first_tangent.y.abs() < 1e-6);
        assert!((g.second_tangent.x - 50.).abs() < 1e-6 && (g.second_tangent.y - 10.).abs() < 1e-6);
        assert!((g.center.x - 60.).abs() < 1e-6 && (g.center.y - 10.).abs() < 1e-6);
    }

    #[test]
    fn bezier_corner_at_end_points_away() {
        let mut d = Document::new();
        let c = d.add_point(Point2::new(0., 0.));
        let tip = d.add_point(Point2::new(100., 0.));
        let line = d.add_segment(c, tip);
        // Bezier ending at the corner, last handle straight below it, so
        // the away-ray at the end must point north.
        let p0 = d.add_point(Point2::new(-100., 0.));
        let h1 = d.add_point(Point2::new(-50., 0.));
        let h2 = d.add_point(Point2::new(0., -50.));
        let bez = d.add_bezier_segment(p0, h1, h2, c);
        let g = probe(line, bez, c).evaluate(&d).unwrap();
        assert!((g.first_tangent.x - 10.).abs() < 1e-6 && g.first_tangent.y.abs() < 1e-6);
        assert!(g.second_tangent.x.abs() < 1e-6 && (g.second_tangent.y - 10.).abs() < 1e-6);
    }
}
