use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::core::document::Document;
use crate::core::geometry::Point2;
use crate::core::ids::{PointId, SegmentId};

use super::dims;
use super::pick;

// Constraint gating: which constraint rows a selection can take. Pure functions.

/// Valid bare-point selections (live positions only).
pub(crate) fn gate_points(doc: &Document, selection: &[ElementRef]) -> Vec<PointId> {
    selection
        .iter()
        .filter_map(|el| el.as_point())
        .filter(|id| doc.point(*id).is_some())
        .collect()
}

/// Valid explicitly-selected segments.
pub(crate) fn gate_segs(doc: &Document, selection: &[ElementRef]) -> Vec<SegmentId> {
    selection
        .iter()
        .filter_map(|el| el.as_segment())
        .filter(|id| doc.segment(*id).is_some())
        .collect()
}

/// First segment owning pid as an endpoint (arena order).
pub(crate) fn owner_segment(doc: &Document, pid: PointId) -> Option<SegmentId> {
    doc.all_segments()
        .find(|(_, s)| s.start == pid || s.end == pid)
        .map(|(id, _)| id)
}

/// True when one segment owns both points as its endpoints (gluing
/// them would collapse that edge — never offered).
pub(crate) fn same_segment_owner(doc: &Document, a: PointId, b: PointId) -> bool {
    doc.all_segments().any(|(_, s)| {
        (s.start == a && s.end == b) || (s.start == b && s.end == a)
    })
}

/// Coordinates forced onto pid by the H/V/coincident network, if any
/// (first found per axis; point-on-edge relations don't fix a coord).
/// Used to reject pairs whose existing constraints already force
/// different positions on the constrained axis.
pub(crate) fn required_coords(doc: &Document, pid: PointId) -> (Option<f64>, Option<f64>) {
    use std::collections::HashSet;
    let mut seen: HashSet<PointId> = HashSet::new();
    let mut stack = vec![pid];
    let (mut rx, mut ry) = (None, None);
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        for c in &doc.constraints {
            let other = if c.a == p {
                Some(c.b)
            } else if c.b == p {
                Some(c.a)
            } else {
                None
            };
            let Some(o) = other else { continue };
            let Some(op) = doc.point(o) else { continue };
            match c.kind {
                ConstraintKind::Horizontal => {
                    if ry.is_none() {
                        ry = Some(op.y);
                    }
                    stack.push(o);
                }
                ConstraintKind::Vertical => {
                    if rx.is_none() {
                        rx = Some(op.x);
                    }
                    stack.push(o);
                }
                ConstraintKind::Coincident if c.point_on_segment.is_none() => {
                    if rx.is_none() {
                        rx = Some(op.x);
                    }
                    if ry.is_none() {
                        ry = Some(op.y);
                    }
                    stack.push(o);
                }
                _ => {}
            }
        }
        if rx.is_some() && ry.is_some() {
            break;
        }
    }
    (rx, ry)
}

/// Locked direction of a line whose endpoints carry a matching H/V
/// constraint, if any.
pub(crate) fn locked_dir(doc: &Document, sid: SegmentId) -> Option<(f64, f64)> {
    let s = doc.segment(sid)?;
    if s.kind != crate::core::document::SegmentKind::Line {
        return None;
    }
    doc.constraints.iter().find_map(|c| {
        let pair = (c.a == s.start && c.b == s.end) || (c.a == s.end && c.b == s.start);
        if !pair {
            return None;
        }
        match c.kind {
            ConstraintKind::Horizontal => Some((1.0, 0.0)),
            ConstraintKind::Vertical => Some((0.0, 1.0)),
            _ => None,
        }
    })
}

/// Curve tangent direction at the document location nearest `near`.
/// Orientation-agnostic (callers compare with abs dot).
pub(crate) fn curve_tangent_at(
    doc: &Document,
    sid: SegmentId,
    near: Point2,
) -> Option<(f64, f64)> {
    use crate::core::document::SegmentKind as SK;
    let s = doc.segment(sid)?;
    match s.kind {
        SK::Arc => {
            let (Some(a), Some(b), Some(c)) = (
                doc.point(s.start),
                doc.point(s.end),
                s.ctrl.and_then(|id| doc.point(id)),
            ) else {
                return None;
            };
            let (o, r) = crate::editor::arc::circumcircle(a, b, c)?;
            if r < 1e-9 {
                return None;
            }
            // Project `near` onto the circle, then perpendicular.
            let (dx, dy) = (near.x - o.x, near.y - o.y);
            let l = (dx * dx + dy * dy).sqrt().max(1e-9);
            Some((-dy / l, dx / l))
        }
        SK::Bezier => {
            let (h1, h2) = s.bezier_handles();
            let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                doc.point(s.start),
                h1.and_then(|id| doc.point(id)),
                h2.and_then(|id| doc.point(id)),
                doc.point(s.end),
            ) else {
                return None;
            };
            Some(crate::editor::bezier::nearest_on_curve(p0, c1, c2, p1, near, 48).2)
        }
        _ => None,
    }
}

/// Whether two segments cross (sine of their directions above epsilon).
/// Shared by pick resolution and the Angle/Distance kind switch.
pub(crate) fn geoms_cross(
    ga: (crate::core::geometry::Point2, crate::core::geometry::Point2),
    gb: (crate::core::geometry::Point2, crate::core::geometry::Point2),
) -> bool {
    let (u1, _) = dims::dim_axes(ga.1.x - ga.0.x, ga.1.y - ga.0.y);
    let (u2, _) = dims::dim_axes(gb.1.x - gb.0.x, gb.1.y - gb.0.y);
    (u1.0 * u2.1 - u1.1 * u2.0).abs() >= 1e-3
}

/// Whether two segments' directions cross (false for parallel pairs
/// and missing geometry).
pub(crate) fn lines_cross(doc: &Document, a: SegmentId, b: SegmentId) -> bool {
    match (doc.segment_geom(a), doc.segment_geom(b)) {
        (Some(ga), Some(gb)) => geoms_cross(ga, gb),
        _ => false,
    }
}

/// HV candidate: (a, b, segment-if-from-line). Explicit line segments
/// win; else two bare points that don't share one segment and whose
/// inferred axis isn't already forced apart.
pub(crate) fn hv_candidates(
    doc: &Document,
    selection: &[ElementRef],
) -> Option<(PointId, PointId, Option<SegmentId>)> {
    use crate::core::document::SegmentKind as SK;
    if let Some(sid) = selection.iter().find_map(|el| el.as_segment()) {
        if let Some(s) = doc.segment(sid) {
            if s.kind == SK::Line
                && doc.point(s.start).is_some()
                && doc.point(s.end).is_some()
            {
                return Some((s.start, s.end, Some(sid)));
            }
        }
    }
    let pts = gate_points(doc, selection);
    if pts.len() < 2 {
        return None;
    }
    let (a, b) = (pts[0], pts[1]);
    if same_segment_owner(doc, a, b) {
        return None;
    }
    let (Some(pa), Some(pb)) = (doc.point(a), doc.point(b)) else {
        return None;
    };
    // Same dominant-axis inference as the apply path.
    let horizontal = (pa.y - pb.y).abs() <= (pa.x - pb.x).abs();
    let (ra, rb) = (required_coords(doc, a), required_coords(doc, b));
    let conflict = if horizontal {
        matches!((ra.1, rb.1), (Some(x), Some(y)) if (x - y).abs() > 1e-6)
    } else {
        matches!((ra.0, rb.0), (Some(x), Some(y)) if (x - y).abs() > 1e-6)
    };
    if conflict {
        return None;
    }
    Some((a, b, None))
}

/// Whether gluing a and b is compatible with forced coordinates.
pub(crate) fn coincident_feasible(doc: &Document, a: PointId, b: PointId) -> bool {
    let (ra, rb) = (required_coords(doc, a), required_coords(doc, b));
    let x_ok = match (ra.0, rb.0) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-6,
        _ => true,
    };
    let y_ok = match (ra.1, rb.1) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-6,
        _ => true,
    };
    x_ok && y_ok
}

/// Coincident candidate: explicit point pairs (never same-segment),
/// else a lone point to its nearest selected-segment endpoint (never
/// itself), else the nearest cross-segment endpoint pair. All pairs
/// feasibility-checked.
pub(crate) fn coincident_candidates(
    doc: &Document,
    selection: &[ElementRef],
) -> Option<(PointId, PointId)> {
    let points = gate_points(doc, selection);
    let segs = gate_segs(doc, selection);
    if points.len() >= 2 {
        let (a, b) = (points[0], points[1]);
        if a != b && !same_segment_owner(doc, a, b) && coincident_feasible(doc, a, b) {
            return Some((a, b));
        }
        return None;
    }
    let mut ends: Vec<(usize, PointId, Point2)> = Vec::new();
    for (si, sid) in segs.iter().enumerate() {
        if let Some(s) = doc.segment(*sid) {
            if let Some(p) = doc.point(s.start) {
                ends.push((si, s.start, p));
            }
            if let Some(p) = doc.point(s.end) {
                ends.push((si, s.end, p));
            }
        }
    }
    if points.len() == 1 && !ends.is_empty() {
        let Some(p) = doc.point(points[0]) else {
            return None;
        };
        let best = ends
            .iter()
            .filter(|(_, id, _)| *id != points[0])
            .min_by(|(_, _, a), (_, _, b)| {
                pick::distance(*a, p)
                    .partial_cmp(&pick::distance(*b, p))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(&(_, id, _)) = best {
            if coincident_feasible(doc, points[0], id) {
                return Some((points[0], id));
            }
        }
        return None;
    }
    if ends.len() >= 2 {
        let mut best: Option<(f64, PointId, PointId)> = None;
        for (i, &(sa, a, pa)) in ends.iter().enumerate() {
            for &(sb, b, pb) in &ends[i + 1..] {
                if sa == sb || a == b {
                    continue;
                }
                let d = pick::distance(pa, pb);
                if best.map_or(true, |(bd, _, _)| d < bd) && coincident_feasible(doc, a, b) {
                    best = Some((d, a, b));
                }
            }
        }
        return best.map(|(_, a, b)| (a, b));
    }
    None
}

/// Merge pairs: close (within tol), unglued bare-point pairs — the old
/// bond-popup condition. Only these may merge; mass selections never
/// collapse to one point.
pub(crate) fn merge_candidate_pairs(
    doc: &Document,
    selection: &[ElementRef],
    tol: f64,
) -> Vec<(PointId, PointId)> {
    let points = gate_points(doc, selection);
    let mut out = Vec::new();
    for (i, &a) in points.iter().enumerate() {
        let Some(pa) = doc.point(a) else { continue };
        for &b in &points[i + 1..] {
            if a == b {
                continue;
            }
            let Some(pb) = doc.point(b) else { continue };
            if pick::distance(pa, pb) > tol {
                continue;
            }
            let glued = doc.constraints.iter().any(|c| {
                c.kind == ConstraintKind::Coincident
                    && ((c.a == a && c.b == b) || (c.a == b && c.b == a))
            });
            if glued {
                continue;
            }
            out.push((a, b));
        }
    }
    out
}

/// Tangent candidate: (line, other, contact, collinear). Explicit
/// segments first, then segments implied by bare-point owners, so two
/// bare points on two lines can still go tangent-collinear. Ids are
/// always distinct; contact is the joint (nearest line end to the
/// other's ends).
pub(crate) fn tangent_candidate(
    doc: &Document,
    selection: &[ElementRef],
) -> Option<(SegmentId, SegmentId, PointId, bool)> {
    use crate::core::document::SegmentKind as SK;
    let mut lines: Vec<SegmentId> = Vec::new();
    let mut curves: Vec<SegmentId> = Vec::new();
    let mut push_seg = |sid: SegmentId| {
        let Some(s) = doc.segment(sid) else { return };
        if s.kind == SK::Line {
            if !lines.contains(&sid) && lines.len() < 2 {
                lines.push(sid);
            }
        } else if s.is_curve() {
            if !curves.contains(&sid) && curves.len() < 2 {
                curves.push(sid);
            }
        }
    };
    for el in selection {
        if let Some(sid) = el.as_segment() {
            push_seg(sid);
        }
    }
    for pid in gate_points(doc, selection) {
        if let Some(oid) = owner_segment(doc, pid) {
            push_seg(oid);
        }
    }
    let (line, other, collinear) = if let Some(&line) = lines.first() {
        if let Some(&c) = curves.first() {
            (line, c, false)
        } else if lines.len() >= 2 && lines[1] != line {
            (line, lines[1], true)
        } else {
            return None;
        }
    } else if curves.len() >= 2 {
        (curves[0], curves[1], false)
    } else {
        return None;
    };
    let (Some(lseg), Some(cseg)) = (doc.segment(line), doc.segment(other)) else {
        return None;
    };
    let mut best: Option<(f64, PointId)> = None;
    for lid in [lseg.start, lseg.end] {
        let Some(lp) = doc.point(lid) else { continue };
        for cid in [cseg.start, cseg.end] {
            let Some(cp) = doc.point(cid) else { continue };
            let d = pick::distance(lp, cp);
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, lid));
            }
        }
    }
    let (_, contact) = best?;
    Some((line, other, contact, collinear))
}

/// Tangent feasibility against existing locks. Free lines always pass
/// (the solver swings them into place). Locked lines must already ride
/// the curve tangent (~12°); collinear pairs need parallel lock dirs.
pub(crate) fn tangent_feasible(
    doc: &Document,
    line: SegmentId,
    other: SegmentId,
    contact: PointId,
) -> bool {
    use crate::core::document::SegmentKind as SK;
    let Some(oseg) = doc.segment(other) else {
        return false;
    };
    let parallel_dirs = |a: (f64, f64), b: (f64, f64)| {
        (a.0 * b.1 - a.1 * b.0).abs() < 0.05
    };
    if oseg.kind == SK::Line {
        match (locked_dir(doc, line), locked_dir(doc, other)) {
            (Some(a), Some(b)) => parallel_dirs(a, b),
            _ => true,
        }
    } else if oseg.kind == SK::Bezier || doc.segment(line).is_some_and(|s| s.kind == SK::Bezier) {
        true
    } else {
        let Some(d) = locked_dir(doc, line) else {
            return true;
        };
        let Some(cp) = doc.point(contact) else {
            return false;
        };
        let Some(t) = curve_tangent_at(doc, other, cp) else {
            return true;
        };
        (d.0 * t.0 + d.1 * t.1).abs() > 0.978
    }
}

/// Two distinct explicitly-selected lines, if present.
pub(crate) fn line_pair(
    doc: &Document,
    selection: &[ElementRef],
) -> Option<(SegmentId, SegmentId)> {
    use crate::core::document::SegmentKind as SK;
    let mut lines = Vec::new();
    for el in selection {
        if let Some(sid) = el.as_segment()
            && let Some(s) = doc.segment(sid)
            && s.kind == SK::Line
            && !lines.contains(&sid)
        {
            lines.push(sid);
            if lines.len() == 2 {
                break;
            }
        }
    }
    if lines.len() == 2 {
        Some((lines[0], lines[1]))
    } else {
        None
    }
}

/// Parallel/perpendicular feasibility against H/V locks. A free side
/// always passes; two locked sides must already satisfy the relation.
pub(crate) fn line_pair_feasible(
    doc: &Document,
    a: SegmentId,
    b: SegmentId,
    want_parallel: bool,
) -> bool {
    match (locked_dir(doc, a), locked_dir(doc, b)) {
        (Some(x), Some(y)) => {
            let cross = (x.0 * y.1 - x.1 * y.0).abs();
            let dot = (x.0 * y.0 + x.1 * y.1).abs();
            if want_parallel { cross < 0.05 } else { dot < 0.05 }
        }
        _ => true,
    }
}
