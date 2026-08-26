use crate::core::constraints::ElementRef;
use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{FillId, PointId, SegmentId};
use crate::editor::Camera;

// Unified picking. Every hover/click/marquee routes through these functions
// so there is exactly ONE notion of "what is under the cursor".

pub struct Picker<'a> {
    pub doc: &'a Document,
    pub camera: &'a Camera,
    // Tolerance in document units (screen px / zoom).
    pub tol: f64,
}

impl<'a> Picker<'a> {
    pub fn new(doc: &'a Document, camera: &'a Camera, tol_px: f64) -> Self {
        Self { doc, camera, tol: tol_px / camera.zoom }
    }

    /// Nearest element under the cursor. Points win over segments, segments
    /// over fills — the finest-grained editable thing is picked.
    pub fn element(&self, at: Point2) -> Option<ElementRef> {
        self.point(at)
            .map(ElementRef::Point)
            .or_else(|| self.segment(at).map(ElementRef::Segment))
            .or_else(|| self.fill(at).map(ElementRef::Fill))
    }

    pub fn point(&self, at: Point2) -> Option<PointId> {
        let mut best: Option<(f64, PointId)> = None;
        for (id, p) in self.doc.all_points() {
            let d = distance(p, at);
            if d <= self.tol && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    pub fn segment(&self, at: Point2) -> Option<SegmentId> {
        let mut best: Option<(f64, SegmentId)> = None;
        for (id, seg) in self.doc.all_segments() {
            if seg.kind != SegmentKind::Line && seg.kind != SegmentKind::Ruler {
                // Arcs hit-test against their sampled polyline.
                if seg.kind == SegmentKind::Arc
                    && let Some(samples) =
                        crate::editor::arc::segment_samples(self.doc, id, 32)
                {
                    let d = samples
                        .windows(2)
                        .map(|w| point_segment_distance(at, w[0], w[1]))
                        .fold(f64::MAX, f64::min);
                    if d <= self.tol && best.map_or(true, |(bd, _)| d < bd) {
                        best = Some((d, id));
                    }
                }
                continue;
            }
            let Some((a, b)) = self.doc.segment_geom(id) else {
                continue;
            };
            let d = point_segment_distance(at, a, b);
            if d <= self.tol && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Innermost closed loop containing the point.
    pub fn fill(&self, at: Point2) -> Option<FillId> {
        let mut best: Option<(f64, FillId)> = None;
        for (id, _) in self.doc.all_fills() {
            let Some(points) = self.loop_points(id) else {
                continue;
            };
            if !point_in_polygon(at, &points) {
                continue;
            }
            let area = polygon_area(&points);
            // Smallest area wins: nested loops pick the inner one.
            if best.map_or(true, |(ba, _)| area < ba) {
                best = Some((area, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Ordered corner positions of a fill loop, or None if broken/open.
    pub fn loop_points(&self, id: FillId) -> Option<Vec<Point2>> {
        loop_points(self.doc, id)
    }

    /// Elements fully inside the band, plus segments crossing it (partial
    /// pickup — crossing an edge selects just that edge).
    pub fn marquee(&self, band: Rect) -> Vec<ElementRef> {
        let mut out = Vec::new();
        // Points/segments owned by a fill are governed by the FILL arm
        // below (containment or the >50% edge rule); they must never be
        // picked up again as standalone elements.
        let in_fill = |sid: SegmentId| {
            self.doc.all_fills().any(|(_, f)| f.segments.contains(&sid))
        };
        let point_in_fill = |pid: PointId| {
            self.doc.all_fills().any(|(_, f)| f.segments.iter().any(|&s| {
                self.doc
                    .segment(s)
                    .is_some_and(|seg| seg.start == pid || seg.end == pid)
            }))
        };
        for layer in &self.doc.layers {
            for &el in &layer.elements {
                match el {
                    ElementRef::Point(pid) => {
                        if point_in_fill(pid) {
                            continue;
                        }
                        if let Some(p) = self.doc.point(pid)
                            && band.contains(p)
                        {
                            out.push(el);
                        }
                    }
                    ElementRef::Segment(sid) => {
                        if in_fill(sid) {
                            continue;
                        }
                        // Must actually TOUCH the band first (crossing or
                        // containing geometry) — parallel_coverage alone
                        // ignores lateral distance and would grab distant
                        // aligned lines.
                        if !self.segment_in_or_crossing(sid, band) {
                            continue;
                        }
                        // Partial pickup by ACTUAL covered length (param
                        // clip of the segment against the band): past half
                        // -> whole line; otherwise just endpoint(s) inside
                        // the band.
                        let ends = self
                            .doc
                            .segment(sid)
                            .map(|s| (s.start, s.end));
                        let frac = ends.map_or(0., |(sa, sb)| {
                            self.doc
                                .point(sa)
                                .zip(self.doc.point(sb))
                                .map_or(0., |(pa, pb)| {
                                    let len = distance(pa, pb);
                                    if len < 1e-9 {
                                        return 0.;
                                    }
                                    (clipped_len(pa, pb, band) / len).min(1.)
                                })
                        });
                        let both_inside = ends.map_or(false, |(sa, sb)| {
                            self.doc.point(sa).map_or(false, |p| band.contains(p))
                                && self.doc.point(sb).map_or(false, |p| band.contains(p))
                        });
                        if frac > 0.5 || both_inside {
                            out.push(el);
                        } else if let Some((sa, sb)) = ends {
                            if self
                                .doc
                                .point(sa)
                                .map_or(false, |p| band.contains(p))
                            {
                                out.push(ElementRef::Point(sa));
                            }
                            if self
                                .doc
                                .point(sb)
                                .map_or(false, |p| band.contains(p))
                            {
                                out.push(ElementRef::Point(sb));
                            }
                        }
                    }
                    ElementRef::Fill(fid) => {
                        let contained = self
                            .loop_points(fid)
                            .is_some_and(|pts| pts.iter().all(|p| band.contains(*p)));
                        if contained {
                            out.push(el);
                            continue;
                        }
                        // Partial coverage rules, per spec:
                        // 1. Exactly ONE loop corner inside the band -> that
                        //    corner's two edges, any depth.
                        // 2. Otherwise an edge joins iff the band touches it
                        //    AND covers more than half its own length.
                        if let Some(f) = self.doc.fill(fid) {
                            let pts = self.loop_points(fid).unwrap_or_default();
                            let inside: Vec<usize> = pts
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| band.contains(**p))
                                .map(|(i, _)| i)
                                .collect();
                            let mut grabbed: Vec<SegmentId> = Vec::new();
                            if inside.len() == 1 {
                                // Corner i joins edges (i-1, i).
                                let n = f.segments.len();
                                let i = inside[0];
                                grabbed.push(f.segments[(i + n - 1) % n]);
                                grabbed.push(f.segments[i]);
                            }
                            for &sid in &f.segments {
                                if grabbed.contains(&sid) {
                                    continue;
                                }
                                if !self.segment_in_or_crossing(sid, band) {
                                    continue;
                                }
                                if self.parallel_coverage(sid, band) > 0.5 {
                                    grabbed.push(sid);
                                }
                            }
                            for sid in grabbed {
                                if !out.iter().any(|e| e.as_segment() == Some(sid)) {
                                    out.push(ElementRef::Segment(sid));
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn segment_in_or_crossing(&self, sid: SegmentId, band: Rect) -> bool {
        let Some((a, b)) = self.doc.segment_geom(sid) else {
            return false;
        };
        band.contains(a) || band.contains(b) || segment_intersects_rect(a, b, band)
    }

    // Bounding region of a fill's loop corners.
    fn fill_region(&self, id: FillId) -> Option<Rect> {
        let pts = self.loop_points(id)?;
        let mut acc: Option<Rect> = None;
        for &p in &pts {
            let r = Rect::from_points(p, p);
            acc = Some(match acc {
                Some(a) => a.union(&r),
                None => r,
            });
        }
        acc
    }

    // Fraction of a segment's length (along its dominant axis) that lies
    // inside the band. The gate for partial marquee grabs: > 0.5 selects.
    fn parallel_coverage(&self, sid: SegmentId, band: Rect) -> f64 {
        let Some((a, b)) = self.doc.segment_geom(sid) else {
            return 0.;
        };
        let parallel_x = (a.x - b.x).abs() >= (a.y - b.y).abs();
        let (alo, ahi) = if parallel_x {
            (a.x.min(b.x), a.x.max(b.x))
        } else {
            (a.y.min(b.y), a.y.max(b.y))
        };
        let (blo, bhi) = if parallel_x {
            (
                band.origin.x.min(band.origin.x + band.size.w),
                band.origin.x.max(band.origin.x + band.size.w),
            )
        } else {
            (
                band.origin.y.min(band.origin.y + band.size.h),
                band.origin.y.max(band.origin.y + band.size.h),
            )
        };
        let span = (ahi - alo).abs().max(1e-9);
        let lo = alo.max(blo);
        let hi = ahi.min(bhi);
        if hi <= lo {
            return 0.;
        }
        ((hi - lo) / span).min(1.)
    }
}

pub fn distance(a: Point2, b: Point2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Ordered corner positions of a closed fill loop, or None if the loop is
/// broken (missing segment) or open (ends don't chain back).
pub fn loop_points(doc: &Document, id: FillId) -> Option<Vec<Point2>> {
    let f = doc.fill(id)?;
    if f.segments.len() < 3 {
        return None;
    }
    let mut out = Vec::with_capacity(f.segments.len());
    let mut cursor: Option<PointId> = None;
    for (i, &sid) in f.segments.iter().enumerate() {
        let seg = doc.segment(sid)?;
        match i {
            // First segment contributes BOTH corners.
            0 => {
                out.push(doc.point(seg.start)?);
                out.push(doc.point(seg.end)?);
                cursor = Some(seg.end);
            }
            _ => {
                let expected = cursor?;
                let next = if seg.start == expected {
                    seg.end
                } else if seg.end == expected {
                    seg.start
                } else {
                    return None;
                };
                cursor = Some(next);
                // Skip the closing corner — it duplicates pts[0].
                if doc.point(next) != out.first().copied() {
                    out.push(doc.point(next)?);
                }
            }
        }
    }
    // Closed: last endpoint must equal the first corner.
    if cursor? != doc.segment(f.segments[0])?.start {
        return None;
    }
    Some(out)
}

/// Distance from p to the ab segment (clamped to the endpoints).
pub fn point_segment_distance(p: Point2, a: Point2, b: Point2) -> f64 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    if len_sq == 0. {
        return distance(p, a);
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len_sq).clamp(0., 1.);
    distance(p, Point2::new(a.x + t * abx, a.y + t * aby))
}

pub fn midpoint(a: Point2, b: Point2) -> Point2 {
    Point2::new((a.x + b.x) / 2., (a.y + b.y) / 2.)
}

/// Length of the ab segment actually INSIDE rect r (parametric clip).
fn clipped_len(a: Point2, b: Point2, r: Rect) -> f64 {
    let (xlo, xhi) = (
        r.origin.x.min(r.origin.x + r.size.w),
        r.origin.x.max(r.origin.x + r.size.w),
    );
    let (ylo, yhi) = (
        r.origin.y.min(r.origin.y + r.size.h),
        r.origin.y.max(r.origin.y + r.size.h),
    );
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t0: f64 = 0.;
    let mut t1: f64 = 1.;
    for (p, d, lo, hi) in [(a.x, dx, xlo, xhi), (a.y, dy, ylo, yhi)] {
        if d.abs() < 1e-12 {
            if p < lo || p > hi {
                return 0.;
            }
        } else {
            let (ta, tb) = ((lo - p) / d, (hi - p) / d);
            let (ta, tb) = (ta.min(tb), ta.max(tb));
            t0 = t0.max(ta);
            t1 = t1.min(tb);
        }
    }
    if t1 <= t0 {
        return 0.;
    }
    (t1 - t0) * distance(a, b)
}


// Even-odd ray cast.
fn point_in_polygon(p: Point2, pts: &[Point2]) -> bool {
    let mut inside = false;
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (pi, pj) = (pts[i], pts[j]);
        if (pi.y > p.y) != (pj.y > p.y)
            && p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y) + pi.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn polygon_area(pts: &[Point2]) -> f64 {
    // Signed shoelace, absolute value for comparisons.
    let mut sum = 0.;
    let n = pts.len();
    if n < 3 {
        return 0.;
    }
    let mut j = n - 1;
    for i in 0..n {
        sum += (pts[j].x + pts[i].x) * (pts[j].y - pts[i].y);
        j = i;
    }
    (sum / 2.).abs()
}

fn segment_intersects_rect(a: Point2, b: Point2, r: Rect) -> bool {
    let tl = r.origin;
    let tr = Point2::new(r.origin.x + r.size.w, r.origin.y);
    let br = Point2::new(r.origin.x + r.size.w, r.origin.y + r.size.h);
    let bl = Point2::new(r.origin.x, r.origin.y + r.size.h);
    segments_cross(a, b, tl, tr)
        || segments_cross(a, b, tr, br)
        || segments_cross(a, b, br, bl)
        || segments_cross(a, b, bl, tl)
}

fn segments_cross(p1: Point2, p2: Point2, p3: Point2, p4: Point2) -> bool {
    let d = |a: Point2, b: Point2, c: Point2| {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    };
    let d1 = d(p3, p4, p1);
    let d2 = d(p3, p4, p2);
    let d3 = d(p1, p2, p3);
    let d4 = d(p1, p2, p4);
    ((d1 > 0.) != (d2 > 0.)) && ((d3 > 0.) != (d4 > 0.))
}