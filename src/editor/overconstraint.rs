use crate::core::constraints::{ConstraintKind, DimMode, DimTarget};
use crate::core::document::Document;
use crate::core::ids::{PointId, SegmentId};
use crate::editor::{BlockChip, Editor, LockKind, ToastRequest};

// Over-constraint explanations: every rejection is a short summary
// sentence, the exact existing locks as icon chips, and a one-line fix.
// The solver itself only reports residuals, so these helpers inspect the
// H/V/coincident network + stored dimensions touching the attempted
// geometry.

fn fmt_n(v: f64) -> String {
    format!("{v:.2}")
}

fn point_label(doc: &Document, pid: PointId) -> String {
    if let Some(p) = doc.point(pid) {
        format!("({:.1}, {:.1})", p.x, p.y)
    } else {
        "unknown point".to_string()
    }
}

fn seg_label(_doc: &Document, sid: SegmentId) -> String {
    format!("edge {}", sid.idx)
}

fn point_pair(a: PointId, b: PointId) -> String {
    format!("P{}–P{}", a.idx, b.idx)
}

fn seg_points(doc: &Document, sid: SegmentId) -> Vec<PointId> {
    let Some(s) = doc.segment(sid) else { return Vec::new() };
    let mut v = vec![s.start, s.end];
    for extra in [s.ctrl, s.center].into_iter().flatten() {
        if !v.contains(&extra) {
            v.push(extra);
        }
    }
    v
}

/// Existing locks touching any of `points`, as icon chips (capped so the
/// toast stays readable). Dimensions only count when they pin point
/// positions directly.
fn blocks_touching(doc: &Document, points: &[PointId]) -> Vec<BlockChip> {
    let touches = |p: PointId| points.contains(&p);
    let mut out: Vec<BlockChip> = Vec::new();
    let mut push = |kind: LockKind, label: String| {
        if out.len() < 3
            && !out.iter().any(|c| c.kind == kind && c.label == label)
        {
            out.push(BlockChip { kind, label });
        }
    };
    for c in &doc.constraints {
        if !(touches(c.a) || touches(c.b)) {
            continue;
        }
        match c.kind {
            ConstraintKind::Horizontal => push(
                LockKind::Horizontal,
                format!("Horizontal · {}", point_pair(c.a, c.b)),
            ),
            ConstraintKind::Vertical => push(
                LockKind::Vertical,
                format!("Vertical · {}", point_pair(c.a, c.b)),
            ),
            ConstraintKind::Coincident => {
                if let Some(edge) = c.point_on_segment {
                    push(
                        LockKind::Coincident,
                        format!("P{} on {}", c.a.idx, seg_label(doc, edge)),
                    );
                } else {
                    push(
                        LockKind::Coincident,
                        format!("Coincident · {}", point_pair(c.a, c.b)),
                    );
                }
            }
            ConstraintKind::Tangent => {
                if let Some((x, y)) = c.tangent_segments {
                    push(
                        LockKind::Tangent,
                        format!(
                            "Tangent · {} – {}",
                            seg_label(doc, x),
                            seg_label(doc, y)
                        ),
                    );
                } else {
                    push(
                        LockKind::Tangent,
                        format!("Tangent · {}", point_pair(c.a, c.b)),
                    );
                }
            }
            ConstraintKind::Parallel => {
                if let Some((x, y)) = c.tangent_segments {
                    push(
                        LockKind::Parallel,
                        format!(
                            "Parallel · {} – {}",
                            seg_label(doc, x),
                            seg_label(doc, y)
                        ),
                    );
                } else {
                    push(
                        LockKind::Parallel,
                        format!("Parallel · {}", point_pair(c.a, c.b)),
                    );
                }
            }
            ConstraintKind::Perpendicular => {
                if let Some((x, y)) = c.tangent_segments {
                    push(
                        LockKind::Perpendicular,
                        format!(
                            "Perpendicular · {} – {}",
                            seg_label(doc, x),
                            seg_label(doc, y)
                        ),
                    );
                } else {
                    push(
                        LockKind::Perpendicular,
                        format!("Perpendicular · {}", point_pair(c.a, c.b)),
                    );
                }
            }
        }
    }
    for d in &doc.dimensions {
        let hit = match d.target {
            DimTarget::Points { a, b, .. } => touches(a) || touches(b),
            _ => false,
        };
        if hit {
            push(LockKind::Dimension, format!("Dimension = {}", fmt_n(d.value)));
        }
    }
    out
}

fn kind_title(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Horizontal => "Can't add Horizontal",
        ConstraintKind::Vertical => "Can't add Vertical",
        ConstraintKind::Coincident => "Can't glue these points",
        ConstraintKind::Tangent => "Can't add Tangent",
        ConstraintKind::Parallel => "Can't add Parallel",
        ConstraintKind::Perpendicular => "Can't add Perpendicular",
    }
}

/// Geometric-constraint rejection (floating menu / hotkeys): a short
/// summary, the blocking locks as chips, and the fix.
pub(crate) fn explain_constraint(
    doc: &Document,
    kind: ConstraintKind,
    a: PointId,
    b: PointId,
    summary: String,
    hint: String,
) -> ToastRequest {
    ToastRequest::error(kind_title(kind), summary, hint, blocks_touching(doc, &[a, b]))
}

/// Dimension rejection: short summary + fix, chips naming the locks.
pub(crate) fn explain_dimension(
    doc: &Document,
    target: DimTarget,
    attempted: f64,
) -> ToastRequest {
    match target {
        DimTarget::Points { a, b, mode } => {
            let (pa, pb) = (doc.point(a), doc.point(b));
            let current = match (pa, pb) {
                (Some(pa), Some(pb)) => Some(match mode {
                    DimMode::Aligned => ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2)).sqrt(),
                    DimMode::X => (pb.x - pa.x).abs(),
                    DimMode::Y => (pb.y - pa.y).abs(),
                }),
                _ => None,
            };
            let axis = match mode {
                DimMode::Aligned => "distance",
                DimMode::X => "width",
                DimMode::Y => "height",
            };
            // Same-pair dimension already holding a different value?
            let held = doc.dimensions.iter().find_map(|d| match d.target {
                DimTarget::Points { a: x, b: y, mode: m }
                    if m == mode && ((x == a && y == b) || (x == b && y == a)) =>
                {
                    Some(d.value)
                }
                _ => None,
            });
            // Coincident glue forcing zero?
            let glued = doc.constraints.iter().any(|c| {
                c.kind == ConstraintKind::Coincident
                    && c.point_on_segment.is_none()
                    && ((c.a == a && c.b == b) || (c.a == b && c.b == a))
            });
            let (body, hint) = if let Some(held) = held {
                (
                    format!("This {axis} is already {}.", fmt_n(held)),
                    "Edit that dimension instead of placing another.".to_string(),
                )
            } else if glued {
                (
                    "These points are glued together.".to_string(),
                    "Unglue them first, then dimension.".to_string(),
                )
            } else {
                let now = current.map(fmt_n).unwrap_or_else(|| "?".to_string());
                (
                    format!(
                        "Locks hold this {axis} at {now}, not {att}.",
                        att = fmt_n(attempted),
                    ),
                    "Relax a lock, or match the value.".to_string(),
                )
            };
            ToastRequest::error(
                "Dimension conflicts",
                body,
                hint,
                blocks_touching(doc, &[a, b]),
            )
        }
        DimTarget::EdgeMid { a, b, mode } => {
            let axis = match mode {
                DimMode::Aligned => "gap",
                DimMode::X => "width",
                DimMode::Y => "height",
            };
            let mut pts = seg_points(doc, a);
            pts.extend(seg_points(doc, b));
            ToastRequest::error(
                "Dimension conflicts",
                format!(
                    "The {axis} between these edges can't reach {att}.",
                    att = fmt_n(attempted),
                ),
                "An edge lock is holding it — relax one first.".to_string(),
                blocks_touching(doc, &pts),
            )
        }
        DimTarget::PointLine { p, line } => {
            let mut pts = vec![p];
            pts.extend(seg_points(doc, line));
            ToastRequest::error(
                "Dimension conflicts",
                "This point can't move to that distance.".to_string(),
                "Check its Coincident / H / V locks.".to_string(),
                blocks_touching(doc, &pts),
            )
        }
        DimTarget::Lines { a, b } => {
            let para = doc.constraints.iter().any(|c| {
                c.kind == ConstraintKind::Parallel
                    && c.tangent_segments.is_some_and(|(x, y)| {
                        (x == a && y == b) || (x == b && y == a)
                    })
            });
            let mut pts = seg_points(doc, a);
            pts.extend(seg_points(doc, b));
            let (body, hint) = if !para {
                (
                    "These edges aren't Parallel, so the gap is undefined.".to_string(),
                    "Add Parallel first, then dimension.".to_string(),
                )
            } else {
                (
                    "Locks hold the current spacing.".to_string(),
                    "Relax one first.".to_string(),
                )
            };
            ToastRequest::error("Dimension conflicts", body, hint, blocks_touching(doc, &pts))
        }
        DimTarget::Angle { a, b } => {
            let mut pts = seg_points(doc, a);
            pts.extend(seg_points(doc, b));
            ToastRequest::error(
                "Angle conflicts",
                "Edge locks already fix these directions.".to_string(),
                "Remove the orientation lock first.".to_string(),
                blocks_touching(doc, &pts),
            )
        }
        DimTarget::Radius { seg } => ToastRequest::error(
            "Radius conflicts",
            "Tangent or endpoint locks fix this curvature.".to_string(),
            "Release them first.".to_string(),
            blocks_touching(doc, &seg_points(doc, seg)),
        ),
        DimTarget::CurveLength { seg } => ToastRequest::error(
            "Length conflicts",
            "Locked endpoints can't spread to that length.".to_string(),
            "Free an endpoint first.".to_string(),
            blocks_touching(doc, &seg_points(doc, seg)),
        ),
    }
}

/// H/V rejection: the forced coordinates that clash, as (summary, fix).
pub(crate) fn hv_detail(
    doc: &Document,
    a: PointId,
    b: PointId,
    horizontal: bool,
) -> (String, String) {
    let (ra, rb) = (Editor::required_coords(doc, a), Editor::required_coords(doc, b));
    if horizontal {
        match (ra.1, rb.1) {
            (Some(x), Some(y)) => (
                format!("Needs one shared Y, but locks hold Y={} vs Y={}.", fmt_n(x), fmt_n(y)),
                "Make those locks agree, then retry.".to_string(),
            ),
            _ => (
                "These points can't level out without breaking a lock.".to_string(),
                "Relax a lock, then retry.".to_string(),
            ),
        }
    } else {
        match (ra.0, rb.0) {
            (Some(x), Some(y)) => (
                format!("Needs one shared X, but locks hold X={} vs X={}.", fmt_n(x), fmt_n(y)),
                "Make those locks agree, then retry.".to_string(),
            ),
            _ => (
                "These points can't align without breaking a lock.".to_string(),
                "Relax a lock, then retry.".to_string(),
            ),
        }
    }
}

/// Coincident (glue) rejection, as (summary, fix).
pub(crate) fn coincident_detail(doc: &Document, a: PointId, b: PointId) -> (String, String) {
    let (ra, rb) = (Editor::required_coords(doc, a), Editor::required_coords(doc, b));
    let mut bits = Vec::new();
    if let (Some(x), Some(y)) = (ra.0, rb.0) {
        if (x - y).abs() > 1e-6 {
            bits.push(format!("X is {} vs {}", fmt_n(x), fmt_n(y)));
        }
    }
    if let (Some(x), Some(y)) = (ra.1, rb.1) {
        if (x - y).abs() > 1e-6 {
            bits.push(format!("Y is {} vs {}", fmt_n(x), fmt_n(y)));
        }
    }
    // A non-zero distance dimension between the same points also blocks glue.
    let dim_hold = doc.dimensions.iter().find_map(|d| match d.target {
        DimTarget::Points { a: x, b: y, .. }
            if (x == a && y == b) || (x == b && y == a) =>
        {
            Some(d.value)
        }
        _ => None,
    });
    if let Some(v) = dim_hold {
        bits.push(format!("a dimension holds them {} apart", fmt_n(v)));
    }
    if bits.is_empty() {
        (
            format!(
                "Gluing {} to {} would drag locked geometry along.",
                point_label(doc, a),
                point_label(doc, b),
            ),
            "Delete or relax those locks first.".to_string(),
        )
    } else {
        (
            format!("Gluing needs one position, but {}.", bits.join(" and ")),
            "Delete or relax those first.".to_string(),
        )
    }
}

/// Parallel/perpendicular rejection, as (summary, fix).
pub(crate) fn line_pair_detail(
    doc: &Document,
    a: SegmentId,
    b: SegmentId,
    want_parallel: bool,
) -> (String, String) {
    let opposite = if want_parallel { "Perpendicular" } else { "Parallel" };
    let has_opposite = doc.constraints.iter().any(|c| {
        c.tangent_segments.is_some_and(|(x, y)| {
            (x == a && y == b) || (x == b && y == a)
        }) && ((want_parallel && c.kind == ConstraintKind::Perpendicular)
            || (!want_parallel && c.kind == ConstraintKind::Parallel))
    });
    if has_opposite {
        (
            format!("This pair already has a {opposite} lock."),
            format!("Delete the {opposite} lock first."),
        )
    } else {
        let locked = Editor::locked_dir(doc, a).is_some() && Editor::locked_dir(doc, b).is_some();
        if locked {
            (
                "Both edges are H/V-locked against this.".to_string(),
                "Free one edge's orientation first.".to_string(),
            )
        } else {
            (
                "Rotating into place would break a lock.".to_string(),
                "Relax the H/V lock or dimension first.".to_string(),
            )
        }
    }
}
