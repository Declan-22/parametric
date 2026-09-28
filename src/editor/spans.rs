use std::collections::HashMap;

use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::{PointId, SegmentId};

use super::joints::{Grade, JointData};

// Clay derivation (Phase 1a): pure joint -> bezier-handle math. No GPUI, no
// solver, no editor mutation — the editor pass writes results into handle
// points (which are solver-free by construction: "handles are free, only
// endpoints constrain").
//
// Rules:
// - G1 direction = neighbor chord (prev -> next), always collinear.
// - G0 direction = own span chord (aim at the neighbor on that side).
// - Arm length = span length * fullness / 3 (fullness 1 = thirds, matching
//   the pen's auto handles). 0 collapses: needle cusp.
// - Angle offsets rotate the auto direction (exit wins; unset side mirrors
//   by inheriting the set one). G0 ignores offsets (aim via neighbors).
// - G2 solves the exit-arm scale for signed-curvature match against the
//   incoming span's CURRENT stored handles (local, one pass — documented
//   approximation; the global relax op in Phase 3 solves jointly).
// - Neighbor context = Line + Bezier spans only (arc tangents are not
//   chords). Branching (>1 neighbor span) falls back to own chord.
// - Every exit is finite by construction (degenerate guards); a non-finite
//   G2 result keeps the G1 fallback.

const EPS: f64 = 1e-9;

/// Derived handle positions (start-side, end-side) for one bezier segment.
/// None = not derivable (wrong kind, missing points, neither end managed).
pub fn derive_segment(
    doc: &Document,
    joints: &HashMap<PointId, JointData>,
    sid: SegmentId,
) -> Option<(Point2, Point2)> {
    let seg = doc.segment(sid)?;
    if seg.kind != SegmentKind::Bezier {
        return None;
    }
    if !joints.contains_key(&seg.start) && !joints.contains_key(&seg.end) {
        return None;
    }
    let (a, b) = (doc.point(seg.start)?, doc.point(seg.end)?);
    let ja = joints.get(&seg.start).copied().unwrap_or_default();
    let jb = joints.get(&seg.end).copied().unwrap_or_default();
    let c1 = exit_arm(doc, joints, sid, seg.start, seg.end, a, b, ja, true);
    let c2 = exit_arm(doc, joints, sid, seg.end, seg.start, b, a, jb, false);
    Some((c1, c2))
}

/// Bezier segments with at least one managed endpoint: the editor derive
/// pass set.
pub fn managed_segments(doc: &Document, joints: &HashMap<PointId, JointData>) -> Vec<SegmentId> {
    doc.all_segments()
        .filter(|(_, s)| {
            s.kind == SegmentKind::Bezier
                && (joints.contains_key(&s.start) || joints.contains_key(&s.end))
        })
        .map(|(id, _)| id)
        .collect()
}

/// Exit arm from joint J along the span toward O (both span ends are exits
/// from their own joint's perspective). `is_out` selects full/ang sides.
fn exit_arm(
    doc: &Document,
    joints: &HashMap<PointId, JointData>,
    sid: SegmentId,
    j: PointId,
    o: PointId,
    jp: Point2,
    op: Point2,
    jd: JointData,
    is_out: bool,
) -> Point2 {
    let chord = (op.x - jp.x, op.y - jp.y);
    let len = chord.0.hypot(chord.1);
    if len < EPS {
        return jp; // degenerate span: collapse
    }
    let own = normalize(chord);
    let dir = match jd.grade {
        Grade::G0 => own,
        Grade::G1 | Grade::G2 => neighbor_direction(doc, sid, j, o, op).unwrap_or(own),
    };
    // Mirror rule: the set side wins, the unset side inherits the same
    // rotation (keeps sides collinear). G0 aims via neighbors only.
    let ang = if jd.grade == Grade::G0 {
        0.0
    } else if is_out {
        if jd.ang_out != 0.0 {
            jd.ang_out as f64
        } else {
            jd.ang_in as f64
        }
    } else if jd.ang_in != 0.0 {
        jd.ang_in as f64
    } else {
        jd.ang_out as f64
    };
    let dir = rotate(dir, ang);
    let full = (if is_out { jd.full_out } else { jd.full_in } as f64).max(0.0);
    let mut arm_len = len * full / 3.0;
    if jd.grade == Grade::G2 {
        arm_len = g2_arm(doc, joints, sid, j, o, jp, op, dir, arm_len);
    }
    let p = Point2::new(jp.x + dir.0 * arm_len, jp.y + dir.1 * arm_len);
    if p.x.is_finite() && p.y.is_finite() {
        p
    } else {
        Point2::new(jp.x + own.0 * len / 3.0, jp.y + own.1 * len / 3.0)
    }
}

/// Direction of travel (prev -> next) through J, from the single neighbor
/// span. None = end joint (no neighbor) or branching (ambiguous).
fn neighbor_direction(
    doc: &Document,
    sid: SegmentId,
    j: PointId,
    o: PointId,
    op: Point2,
) -> Option<(f64, f64)> {
    let (_, prev) = neighbor_span(doc, sid, j, o)?;
    let pp = doc.point(prev)?;
    let d = (op.x - pp.x, op.y - pp.y);
    let l = d.0.hypot(d.1);
    if l < EPS {
        return None;
    }
    Some((d.0 / l, d.1 / l))
}

/// The single Line/Bezier span touching J besides `sid`, + its far endpoint.
/// None if zero (end joint) or more than one (branching).
fn neighbor_span(
    doc: &Document,
    sid: SegmentId,
    j: PointId,
    o: PointId,
) -> Option<(SegmentId, PointId)> {
    let mut found = None;
    for (id, s) in doc.all_segments() {
        if id == sid || !matches!(s.kind, SegmentKind::Line | SegmentKind::Bezier) {
            continue;
        }
        let other = if s.start == j && s.end != o {
            Some(s.end)
        } else if s.end == j && s.start != o {
            Some(s.start)
        } else {
            None
        };
        if let Some(p) = other {
            if found.is_some() {
                return None; // branching: ambiguous
            }
            found = Some((id, p));
        }
    }
    found
}

/// Local G2: scale the exit arm so signed curvature at J matches the
/// incoming span's curvature at J (from its CURRENT stored handles).
/// Bisection on log-arm; falls back to the G1 length on any failure.
fn g2_arm(
    doc: &Document,
    _joints: &HashMap<PointId, JointData>,
    sid: SegmentId,
    j: PointId,
    o: PointId,
    jp: Point2,
    op: Point2,
    dir: (f64, f64),
    fallback: f64,
) -> f64 {
    if fallback < EPS {
        return fallback;
    }
    let (in_sid, _) = match neighbor_span(doc, sid, j, o) {
        Some(v) => v,
        None => return fallback,
    };
    let seg = match doc.segment(in_sid) {
        Some(s) if s.kind == SegmentKind::Bezier => s,
        _ => return fallback,
    };
    let (h1, h2) = seg.bezier_handles();
    let (p0, p1, p2, p3) = match (doc.point(seg.start), h1.and_then(|h| doc.point(h)), h2.and_then(|h| doc.point(h)), doc.point(seg.end)) {
        (Some(a), Some(c1), Some(c2), Some(b)) => (a, c1, c2, b),
        _ => return fallback,
    };
    let at_start = seg.start == j;
    // Incoming traversal is into J: if J is the span's start, the flow
    // arrives reversed — evaluate the reversed curve's end instead (same
    // turning, correct orientation).
    let k_target = if at_start {
        end_curvature(p3, p2, p1, p0, false)
    } else {
        end_curvature(p0, p1, p2, p3, false)
    };
    if !k_target.is_finite() {
        return fallback;
    }
    // Other-side arm: current stored handle of THIS span, else thirds.
    let c2 = match doc.segment(sid).and_then(|s| s.bezier_handles().1).and_then(|h| doc.point(h)) {
        Some(p) => p,
        None => {
            let own = normalize((op.x - jp.x, op.y - jp.y));
            let l = (op.x - jp.x).hypot(op.y - jp.y);
            Point2::new(op.x - own.0 * l / 3.0, op.y - own.1 * l / 3.0)
        }
    };
    let k_out = |m: f64| {
        let c1 = Point2::new(jp.x + dir.0 * m, jp.y + dir.1 * m);
        end_curvature(jp, c1, c2, op, true)
    };
    let (mut lo, mut hi) = (fallback / 8.0, fallback * 8.0);
    let (mut flo, mut fhi) = (k_out(lo) - k_target, k_out(hi) - k_target);
    if !flo.is_finite() || !fhi.is_finite() || flo * fhi > 0.0 {
        return fallback;
    }
    for _ in 0..25 {
        let mid = (lo * hi).sqrt();
        let fm = k_out(mid) - k_target;
        if !fm.is_finite() {
            hi = mid;
            fhi = f64::INFINITY;
            continue;
        }
        if flo * fm <= 0.0 {
            hi = mid;
            fhi = fm;
        } else {
            lo = mid;
            flo = fm;
        }
    }
    ((lo * hi).sqrt()).clamp(fallback / 8.0, fallback * 8.0)
}

/// Signed curvature of a cubic at an endpoint. `at_start` = t=0 (leaving),
/// else t=1 (arriving, forward orientation).
fn end_curvature(a: Point2, c1: Point2, c2: Point2, b: Point2, at_start: bool) -> f64 {
    let (d1, d2) = if at_start {
        (
            (3.0 * (c1.x - a.x), 3.0 * (c1.y - a.y)),
            (
                6.0 * (a.x - 2.0 * c1.x + c2.x),
                6.0 * (a.y - 2.0 * c1.y + c2.y),
            ),
        )
    } else {
        (
            (3.0 * (b.x - c2.x), 3.0 * (b.y - c2.y)),
            (
                6.0 * (c1.x - 2.0 * c2.x + b.x),
                6.0 * (c1.y - 2.0 * c2.y + b.y),
            ),
        )
    };
    let denom = d1.0.hypot(d1.1).powi(3);
    if denom < 1e-18 {
        return f64::INFINITY;
    }
    (d1.0 * d2.1 - d1.1 * d2.0) / denom
}

fn normalize(d: (f64, f64)) -> (f64, f64) {    let l = d.0.hypot(d.1);
    if l < EPS {
        (1.0, 0.0)
    } else {
        (d.0 / l, d.1 / l)
    }
}

fn rotate(d: (f64, f64), ang: f64) -> (f64, f64) {
    if ang == 0.0 {
        return d;
    }
    let (c, s) = (ang.cos(), ang.sin());
    (d.0 * c - d.1 * s, d.0 * s + d.1 * c)
}

// -- curvature combs ------------------------------------------------------
// Closed-form cubic frame at t: base point, inside normal (toward the
// center of curvature), signed curvature. None when degenerate.

/// (base, inside_unit_normal, signed_curvature) at t. Inside = direction of
/// the normal component of B''(t); falls back to the left normal when the
/// curve is locally straight.
pub fn comb_frame(
    a: Point2,
    c1: Point2,
    c2: Point2,
    b: Point2,
    t: f64,
) -> Option<(Point2, (f64, f64), f64)> {
    let mt = 1.0 - t;
    // B(t), B'(t), B''(t) Bernstein form.
    let p = Point2::new(
        mt * mt * mt * a.x + 3.0 * mt * mt * t * c1.x + 3.0 * mt * t * t * c2.x + t * t * t * b.x,
        mt * mt * mt * a.y + 3.0 * mt * mt * t * c1.y + 3.0 * mt * t * t * c2.y + t * t * t * b.y,
    );
    let d1 = (
        3.0 * mt * mt * (c1.x - a.x) + 6.0 * mt * t * (c2.x - c1.x) + 3.0 * t * t * (b.x - c2.x),
        3.0 * mt * mt * (c1.y - a.y) + 6.0 * mt * t * (c2.y - c1.y) + 3.0 * t * t * (b.y - c2.y),
    );
    let d2 = (
        6.0 * mt * (c2.x - 2.0 * c1.x + a.x) + 6.0 * t * (b.x - 2.0 * c2.x + c1.x),
        6.0 * mt * (c2.y - 2.0 * c1.y + a.y) + 6.0 * t * (b.y - 2.0 * c2.y + c1.y),
    );
    let speed = d1.0.hypot(d1.1);
    if speed < EPS {
        return None;
    }
    let tangent = (d1.0 / speed, d1.1 / speed);
    let cross = d1.0 * d2.1 - d1.1 * d2.0;
    let k = cross / speed.powi(3);
    // Normal component of acceleration points inside.
    let along = d2.0 * tangent.0 + d2.1 * tangent.1;
    let mut n = (
        d2.0 - along * tangent.0,
        d2.1 - along * tangent.1,
    );
    let nl = n.0.hypot(n.1);
    if nl < 1e-12 {
        n = (-tangent.1, tangent.0); // straight: left normal
    } else {
        n = (n.0 / nl, n.1 / nl);
    }
    Some((p, n, k))
}

/// Comb teeth for one span: (base, tip) in DOC units, `n` teeth at
/// t=(i+0.5)/n. Length = |k| * zoom * scale_px clamped to [2, max_px].
/// Paint maps both ends through the camera (handles y-flip).
pub fn comb_teeth(
    a: Point2,
    c1: Point2,
    c2: Point2,
    b: Point2,
    n: usize,
    zoom: f64,
    scale_px: f32,
    max_px: f32,
) -> Vec<(Point2, Point2)> {
    comb_teeth_k(a, c1, c2, b, n, zoom, scale_px, max_px)
        .into_iter()
        .map(|(base, tip, _)| (base, tip))
        .collect()
}

/// Same, plus signed curvature per tooth (drives heat coloring).
pub fn comb_teeth_k(
    a: Point2,
    c1: Point2,
    c2: Point2,
    b: Point2,
    n: usize,
    zoom: f64,
    scale_px: f32,
    max_px: f32,
) -> Vec<(Point2, Point2, f64)> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f64 + 0.5) / n as f64;
        let Some((base, nrm, k)) = comb_frame(a, c1, c2, b, t) else {
            continue;
        };
        let len_px = ((k.abs() * zoom * scale_px as f64) as f32).clamp(2.0, max_px);
        let len_doc = len_px as f64 / zoom.max(1e-9);
        out.push((
            base,
            Point2::new(base.x + nrm.0 * len_doc, base.y + nrm.1 * len_doc),
            k,
        ));
    }
    out
}

/// Anti-crossing clamp (two passes): no tooth may exceed its neighbor's
/// length by more than their base spacing, so adjacent teeth never cross
/// and the tip envelope stays a clean polyline. Direction preserved; only
/// spikes are shortened. Color (true unfairness) is computed separately,
/// so the clamp costs no information — length reads clean, color reads true.
pub fn clamp_comb_lengths(teeth: &mut [(Point2, Point2, f64)]) {
    if teeth.len() < 2 {
        return;
    }
    for i in 1..teeth.len() {
        let max = tooth_len(&teeth[i - 1]) + tooth_gap(&teeth[i - 1], &teeth[i]);
        shorten_tooth(teeth, i, max);
    }
    for i in (0..teeth.len() - 1).rev() {
        let max = tooth_len(&teeth[i + 1]) + tooth_gap(&teeth[i], &teeth[i + 1]);
        shorten_tooth(teeth, i, max);
    }
}

fn tooth_len(t: &(Point2, Point2, f64)) -> f64 {
    (t.1.x - t.0.x).hypot(t.1.y - t.0.y)
}

fn tooth_gap(a: &(Point2, Point2, f64), b: &(Point2, Point2, f64)) -> f64 {
    (b.0.x - a.0.x).hypot(b.0.y - a.0.y)
}

fn shorten_tooth(teeth: &mut [(Point2, Point2, f64)], i: usize, max: f64) {
    let dx = teeth[i].1.x - teeth[i].0.x;
    let dy = teeth[i].1.y - teeth[i].0.y;
    let l = dx.hypot(dy);
    if l > max && l > 1e-12 {
        let s = max / l;
        teeth[i].1 = Point2::new(teeth[i].0.x + dx * s, teeth[i].0.y + dy * s);
    }
}

/// Heat color for unfairness s in [0,1]: theme blue (fair) -> snap orange
/// -> destructive red (worst). Pure (r,g,b) for testability; paint wraps.
pub fn heat_rgb(s: f32) -> (u8, u8, u8) {
    const BLUE: (f32, f32, f32) = (0x4C as f32, 0x8D as f32, 0xFF as f32);
    const ORANGE: (f32, f32, f32) = (0xFF as f32, 0x95 as f32, 0x00 as f32);
    const RED: (f32, f32, f32) = (0xE5 as f32, 0x3E as f32, 0x3E as f32);
    let s = s.clamp(0.0, 1.0);
    let (a, b, t) = if s < 0.5 {
        (BLUE, ORANGE, s * 2.0)
    } else {
        (ORANGE, RED, (s - 0.5) * 2.0)
    };
    (
        (a.0 + (b.0 - a.0) * t).round() as u8,
        (a.1 + (b.1 - a.1) * t).round() as u8,
        (a.2 + (b.2 - a.2) * t).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::geometry::Point2;

    fn bez_chain() -> (Document, SegmentId, SegmentId, PointId) {
        // P(0,0) -> J(100,0) -> N(200,0), two chained beziers.
        let mut doc = Document::new();
        let p = doc.add_point(Point2::new(0., 0.));
        let j = doc.add_point(Point2::new(100., 0.));
        let n = doc.add_point(Point2::new(200., 0.));
        let h1 = doc.add_point(Point2::new(30., 0.));
        let h2 = doc.add_point(Point2::new(70., 0.));
        let h3 = doc.add_point(Point2::new(130., 0.));
        let h4 = doc.add_point(Point2::new(170., 0.));
        let s1 = doc.add_bezier_segment(p, h1, h2, j);
        let s2 = doc.add_bezier_segment(j, h3, h4, n);
        (doc, s1, s2, j)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn clamp_stops_crossing() {
        // Spike middle tooth: lengths must satisfy |ΔL| <= spacing after.
        let mut teeth = vec![
            (Point2::new(0., 0.), Point2::new(0., 10.), 0.1),
            (Point2::new(10., 0.), Point2::new(10., 60.), 0.6),
            (Point2::new(20., 0.), Point2::new(20., 12.), 0.12),
        ];
        clamp_comb_lengths(&mut teeth);
        let lens: Vec<f64> = teeth
            .iter()
            .map(|t| (t.1.x - t.0.x).hypot(t.1.y - t.0.y))
            .collect();
        assert!((lens[1] - lens[0]).abs() <= 10.0 + 1e-9);
        assert!((lens[2] - lens[1]).abs() <= 10.0 + 1e-9);
        // Spike shortened, direction (straight up) preserved.
        assert!(lens[1] < 60.0);
        assert!((teeth[1].1.x - teeth[1].0.x).abs() < 1e-9);
    }

    #[test]
    fn heat_endpoints() {
        assert_eq!(heat_rgb(0.0), (0x4C, 0x8D, 0xFF));
        assert_eq!(heat_rgb(1.0), (0xE5, 0x3E, 0x3E));
        assert_eq!(heat_rgb(0.5), (0xFF, 0x95, 0x00));
    }

    #[test]
    fn comb_frame_circle_matches_radius() {
        // Quarter circle r=100 centered at origin, from (100,0) to (0,100):
        // kappa-form control length.
        let k = 0.5522847498 * 100.0;
        let (base, nrm, curv) = comb_frame(
            Point2::new(100., 0.),
            Point2::new(100., k),
            Point2::new(k, 100.),
            Point2::new(0., 100.),
            0.5,
        )
        .unwrap();
        // On the circle, normal points at the center, |k| = 1/100.
        let r = base.x.hypot(base.y);
        assert!((r - 100.).abs() < 0.5, "r={r}");
        assert!((curv.abs() - 0.01).abs() < 0.002, "k={curv}");
        // Inside normal + base ≈ center.
        let cx = base.x + nrm.0 * r;
        let cy = base.y + nrm.1 * r;
        assert!(cx.hypot(cy) < 1.0, "c=({cx},{cy})");
    }

    #[test]
    fn comb_teeth_straight_are_minimal_left() {
        let teeth = comb_teeth(
            Point2::new(0., 0.),
            Point2::new(30., 0.),
            Point2::new(60., 0.),
            Point2::new(90., 0.),
            4,
            1.0,
            400.,
            48.,
        );
        assert_eq!(teeth.len(), 4);
        for (base, tip) in teeth {
            // Straight: minimal 2px teeth on the left normal (+y in doc?
            // doc y-up vs screen flip handled by paint mapping both ends).
            assert!(approx(base.y, 0.));
            assert!((tip.x - base.x).abs() < 1e-9);
            assert!(tip.y > base.y); // left of +x travel
        }
    }

    #[test]
    fn smooth_chain_is_collinear_thirds() {
        let (doc, _, s2, j) = bez_chain();
        let mut joints = HashMap::new();
        joints.insert(j, JointData::default()); // G1
        let (c1, c2) = derive_segment(&doc, &joints, s2).unwrap();
        assert!(approx(c1.x, 100. + 100. / 3.) && approx(c1.y, 0.));
        assert!(approx(c2.x, 200. - 100. / 3.) && approx(c2.y, 0.));
    }

    #[test]
    fn corner_aims_along_own_span() {
        let (doc, _, s2, j) = bez_chain();
        let mut joints = HashMap::new();
        joints.insert(
            j,
            JointData {
                grade: Grade::G0,
                ..Default::default()
            },
        );
        // Own chord J->N is straight: same thirds here; kink shows on bends.
        let (c1, _) = derive_segment(&doc, &joints, s2).unwrap();
        assert!(approx(c1.x, 100. + 100. / 3.) && approx(c1.y, 0.));

        // Bent chain: P(0,100) -> J(100,0) -> N(200,0). G0 exit aims J->N.
        let mut doc = Document::new();
        let p = doc.add_point(Point2::new(0., 100.));
        let j = doc.add_point(Point2::new(100., 0.));
        let n = doc.add_point(Point2::new(200., 0.));
        let hb: Vec<_> = (0..4)
            .map(|_| doc.add_point(Point2::new(0., 0.)))
            .collect();
        doc.add_bezier_segment(p, hb[0], hb[1], j);
        let s2 = doc.add_bezier_segment(j, hb[2], hb[3], n);
        let mut joints = HashMap::new();
        joints.insert(
            j,
            JointData {
                grade: Grade::G0,
                ..Default::default()
            },
        );
        let (c1, _) = derive_segment(&doc, &joints, s2).unwrap();
        // J->N direction is (1,0): arm straight out, kink preserved.
        assert!(approx(c1.y, 0.) && c1.x > 100.);
        // Same joint smooth: chord P->N = (200,-100) normalized.
        joints.insert(j, JointData::default());
        let (c1, _) = derive_segment(&doc, &joints, s2).unwrap();
        let l = (200f64).hypot(100.);
        assert!(approx(c1.x, 100. + 200. / l * 100. / 3.));
        assert!(approx(c1.y, 0. - 100. / l * 100. / 3.));
    }

    #[test]
    fn unmanaged_spans_are_skipped() {
        let (doc, _, s2, _) = bez_chain();
        assert!(derive_segment(&doc, &HashMap::new(), s2).is_none());
    }

    #[test]
    fn degenerate_span_collapses_finite() {
        let mut doc = Document::new();
        let j = doc.add_point(Point2::new(50., 50.));
        let h1 = doc.add_point(Point2::new(0., 0.));
        let h2 = doc.add_point(Point2::new(0., 0.));
        let s = doc.add_bezier_segment(j, h1, h2, j);
        let mut joints = HashMap::new();
        joints.insert(j, JointData::default());
        let (c1, c2) = derive_segment(&doc, &joints, s).unwrap();
        assert!(c1.x.is_finite() && c2.y.is_finite());
        assert!(approx(c1.x, 50.) && approx(c2.y, 50.));
    }

    #[test]
    fn zero_fullness_is_needle_cusp() {
        let (doc, _, s2, j) = bez_chain();
        let mut joints = HashMap::new();
        joints.insert(
            j,
            JointData {
                full_out: 0.0,
                ..Default::default()
            },
        );
        let (c1, _) = derive_segment(&doc, &joints, s2).unwrap();
        assert!(approx(c1.x, 100.) && approx(c1.y, 0.));
    }

    #[test]
    fn end_joint_aims_at_neighbor() {
        let mut doc = Document::new();
        let a = doc.add_point(Point2::new(0., 0.));
        let b = doc.add_point(Point2::new(90., 30.));
        let h1 = doc.add_point(Point2::new(0., 0.));
        let h2 = doc.add_point(Point2::new(0., 0.));
        let s = doc.add_bezier_segment(a, h1, h2, b);
        let mut joints = HashMap::new();
        joints.insert(a, JointData::default());
        let (c1, _) = derive_segment(&doc, &joints, s).unwrap();
        let l = 90f64.hypot(30.);
        assert!(approx(c1.x, 90. / l * l / 3.));
        assert!(approx(c1.y, 30. / l * l / 3.));
    }

    #[test]
    fn g2_matches_incoming_curvature() {
        // Bent incoming span (stored handles) + G2 joint: outgoing arm must
        // match signed curvature at J to bisection precision.
        let mut doc = Document::new();
        let p = doc.add_point(Point2::new(0., 0.));
        let j = doc.add_point(Point2::new(100., 0.));
        let n = doc.add_point(Point2::new(200., 50.));
        let h1 = doc.add_point(Point2::new(40., 0.));
        let h2 = doc.add_point(Point2::new(70., -30.));
        let h3 = doc.add_point(Point2::new(0., 0.));
        let h4 = doc.add_point(Point2::new(0., 0.));
        let s1 = doc.add_bezier_segment(p, h1, h2, j);
        let s2 = doc.add_bezier_segment(j, h3, h4, n);
        // Incoming curvature at J from stored handles.
        let k_in = end_curvature(
            doc.point(p).unwrap(),
            doc.point(h1).unwrap(),
            doc.point(h2).unwrap(),
            doc.point(j).unwrap(),
            false,
        );
        assert!(k_in.is_finite());
        let mut joints = HashMap::new();
        joints.insert(
            j,
            JointData {
                grade: Grade::G2,
                ..Default::default()
            },
        );
        let (c1, _) = derive_segment(&doc, &joints, s2).unwrap();
        assert!(c1.x.is_finite() && c1.y.is_finite());
        // Recompute outgoing curvature with the derived arm + stored c2.
        let stored_c2 = doc.point(h4).unwrap();
        let k_out = end_curvature(
            doc.point(j).unwrap(),
            c1,
            stored_c2,
            doc.point(n).unwrap(),
            true,
        );
        let _ = s1;
        assert!(
            (k_out - k_in).abs() < 1e-6,
            "k_out={k_out} k_in={k_in}"
        );
    }
}
