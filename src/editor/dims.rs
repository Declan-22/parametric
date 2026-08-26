use super::ruler;
use crate::editor::pick;
use super::Editor;
use crate::core::document::Document;
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::PointId;

// Dimension render-data computation. Produces screen-space DimRender
// entries consumed by BOTH the painted lines and the DOM label layer.
// Rulers never participate: no dims while drawing one or with one
// selected — their markings are vector geometry, not dimensions.

const PREVIEW_DIM_OFFSET_DOC: f64 = 18.0;

/// Screen-space render data for one constraint chip. `out` is the
/// preferred spot (just off the constrained edge's outer side); `in_` is
/// the flipped spot used when `out` would collide with a dim container or
/// another chip. The layer animates between them.
#[derive(Clone, Debug)]
pub struct ConstraintMarker {
    pub key: String,
    pub constraint: crate::core::constraints::Constraint,
    // Icon selection: coincident chip, else vertical/horizontal arrow.
    pub vertical: bool,
    pub coincident: bool,
    // Visibility: only when the owning element is hovered/selected or the
    // constraint is actively channeling a drag.
    pub visible: bool,
    // Filled styling (accent bg + white icon): active-constraining, owning
    // element selected, or chip clicked.
    pub emphasized: bool,
    // Clicked chip: border switches to theme.accent_border.
    pub clicked: bool,
    // Dashed guide line (screen px) for distant H/V pairs.
    pub guide: Option<[f32; 4]>,
    // Cursor is over this chip (editor-side hit test).
    pub hovered: bool,
    pub cx_out: f32,
    pub cy_out: f32,
    pub cx_in: f32,
    pub cy_in: f32,
    pub flipped: bool,
}

/// Screen-space render data for one dimension.
#[derive(Clone, Debug)]
pub struct DimRender {
    pub ax: f32,
    pub ay: f32,
    pub bx: f32,
    pub by: f32,
    pub lax: f32,
    pub lay: f32,
    pub lbx: f32,
    pub lby: f32,
    pub label_cx: f32,
    pub label_cy: f32,
    pub text: String,
    // Additional extension lines (screen x1,y1,x2,y2) reaching from other
    // objects' extremes to this dim line.
    pub extra_ext: Vec<[f32; 4]>,
}

pub fn update(ed: &mut Editor) {
    ed.dim_renders.clear();

    // Ruler interactions suppress dims entirely (labels are part of the
    // ruler's own vector rendering in paint).
    if ed.pending_ruler.is_some() {
        return;
    }
    if selection_is_ruler(ed) {
        return;
    }

    // RESIZE-driven dims: while a drag is active, show a slanted dim for
    // every edge whose LENGTH actually changed since the gesture started.
    // Selection alone shows nothing; pure translation changes no lengths,
    // so it stays dim-free. Collinear edges (same angle, same infinite
    // line) collapse to ONE dim — the one closest to bottom-right.
    if let Some(drag) = &ed.dragging {
        let mut starts: Vec<(PointId, Point2)> = drag.points.clone();
        for &(pid, s) in &drag.aux {
            if !starts.iter().any(|&(id, _)| id == pid) {
                starts.push((pid, s));
            }
        }
        let start_of = |pid: PointId| -> Option<Point2> {
            starts
                .iter()
                .find(|&&(id, _)| id == pid)
                .map(|&(_, s)| s)
                .or_else(|| ed.doc.point(pid))
        };
        let mut edges: Vec<(Point2, Point2)> = Vec::new();
        for (_, seg) in ed.doc.all_segments() {
            if seg.kind == crate::core::document::SegmentKind::Ruler {
                continue;
            }
            let (Some(sa), Some(sb)) = (start_of(seg.start), start_of(seg.end)) else {
                continue;
            };
            let (Some(ca), Some(cb)) =
                (ed.doc.point(seg.start), ed.doc.point(seg.end))
            else {
                continue;
            };
            // Ignore sub-pixel / solver-residue length changes; only REAL
            // resizes light up dims. Constrained corners that are pinned
            // by their neighbors jitter by tiny amounts under projection.
            if (pick::distance(sa, sb) - pick::distance(ca, cb)).abs() > 0.5 {
                edges.push((ca, cb));
            }
        }
        for (a, b) in dedup_collinear(edges) {
            push_line_dim(ed, a, b);
        }
    }

    // Live line preview: slanted length dim while drawing.
    if let Some(p) = &ed.pending_line {
        let (_, b) = p.snapped(ed.shift);
        push_line_dim(ed, p.start, b);
    }

    // Live preview: bounding box W/H while creating or interacting.
    if let Some(b) = preview_bounds(ed) {
        if b.size.w > 0. {
            ed.dim_renders.push(linear_dim(
                &ed.doc,
                &ed.camera,
                Point2::new(b.origin.x, b.origin.y + b.size.h),
                Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h),
                PREVIEW_DIM_OFFSET_DOC,
                b.size.w,
            ));
        }
        if b.size.h > 0. {
            // Bottom-right -> top-right so the LEFT normal points right
            // (outside the shape).
            ed.dim_renders.push(linear_dim(
                &ed.doc,
                &ed.camera,
                Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h),
                Point2::new(b.origin.x + b.size.w, b.origin.y),
                PREVIEW_DIM_OFFSET_DOC,
                b.size.h,
            ));
        }
    }

    // Stored dimensions: rendered at their own angle and offset.
    let dims = ed.doc.dimensions.clone();
    for d in &dims {
        let (Some(a), Some(b)) = (ed.doc.point(d.a), ed.doc.point(d.b)) else {
            continue;
        };
        let len = d.value.unwrap_or_else(|| dist(a, b));
        ed.dim_renders.push(linear_dim(&ed.doc, &ed.camera, a, b, d.offset, len));
    }

    update_constraint_markers(ed);
}

// Constraint chips. Positioning is deterministic per structural case:
//  - pair forms an actual EDGE -> chip centered on the segment midpoint;
//  - distant H/V pair -> dashed guide line along the shared axis (rendered
//    by the canvas pass) with the chip at its midpoint;
//  - POINT constraints (coincident junctions, future anchors) -> chip 18px
//    above the point, clear of the handle dot.
fn update_constraint_markers(ed: &mut Editor) {
    ed.constraint_markers.clear();
    const CHIP_ABOVE_PX: f32 = 18.;
    const HANDLE_R: f32 = 5.;

    // Constraints actively channeling the current drag. Two patterns:
    //  - exactly ONE endpoint is a PRIMARY drag target (the constraint
    //    holds its axis while the point moves), or
    //  - NO primary endpoints but exactly one FOLLOWER (constraint chains:
    //    a bonded partner or slide-neighbor being towed along).
    let (prim, foll) = ed
        .dragging
        .as_ref()
        .map(|d| {
            (
                d.points.iter().map(|&(id, _)| id).collect::<Vec<_>>(),
                d.aux.iter().map(|&(id, _)| id).collect::<Vec<_>>(),
            )
        })
        .unwrap_or((Vec::new(), Vec::new()));

    let constraints = ed.doc.constraints.clone();
    for c in constraints {
        let (Some(a), Some(b)) = (ed.doc.point(c.a), ed.doc.point(c.b)) else {
            continue;
        };
        let ma = ed.camera.unit_to_screen(a);
        let mb = ed.camera.unit_to_screen(b);
        let coincident_pt = c.kind == crate::core::constraints::ConstraintKind::Coincident;

        // -- position: deterministic per structural case --
        let mut guide: Option<[f32; 4]> = None;
        let is_edge_pair = |s: crate::core::document::Segment| {
            (s.start == c.a && s.end == c.b) || (s.start == c.b && s.end == c.a)
        };
        let has_own_edge = ed.doc.all_segments().any(|(_, s)| is_edge_pair(s));
        let mid = (((ma.x + mb.x) / 2.) as f32, ((ma.y + mb.y) / 2.) as f32);
        let (cx, cy) = if coincident_pt {
            // Point constraint: hover above the junction, clear of the dot.
            (mid.0, mid.1 - HANDLE_R - 2. - CHIP_ABOVE_PX)
        } else if has_own_edge {
            // The pair IS an edge: chip directly on it.
            mid
        } else {
            // Distant pair: dashed guide along the shared axis, chip at
            // its midpoint.
            let g = match c.kind {
                crate::core::constraints::ConstraintKind::Horizontal => {
                    [ma.x.min(mb.x), ma.y, ma.x.max(mb.x), ma.y]
                }
                _ => [ma.x, ma.y.min(mb.y), ma.x, ma.y.max(mb.y)],
            };
            guide = Some([g[0] as f32, g[1] as f32, g[2] as f32, g[3] as f32]);
            (((g[0] + g[2]) / 2.) as f32, ((g[1] + g[3]) / 2.) as f32)
        };

        let in_prim = |p: PointId| prim.contains(&p);
        let in_foll = |p: PointId| foll.contains(&p);
        let (pa, pb) = (in_prim(c.a), in_prim(c.b));
        let active = match ((pa as u8) + (pb as u8)) {
            1 => true,
            0 => in_foll(c.a) != in_foll(c.b),
            _ => false,
        };
        let clicked = ed.selected_constraints.contains(&c);

        // A constraint belongs to everything TOUCHING either of its
        // endpoints: the endpoints themselves, segments ending there, and
        // fills whose loop passes through. Hovering or selecting ANY of
        // those surfaces the chip; selecting does so emphasized.
        let touches = |el: crate::core::constraints::ElementRef| -> bool {
            match el {
                crate::core::constraints::ElementRef::Point(p) => p == c.a || p == c.b,
                crate::core::constraints::ElementRef::Segment(sid) => ed
                    .doc
                    .segment(sid)
                    .is_some_and(|s| s.start == c.a || s.start == c.b || s.end == c.a || s.end == c.b),
                crate::core::constraints::ElementRef::Fill(fid) => ed
                    .doc
                    .element_points(crate::core::constraints::ElementRef::Fill(fid))
                    .iter()
                    .any(|&p| p == c.a || p == c.b),
            }
        };
        let hovered = ed.hover.map_or(false, |h| touches(h));
        let sel_touched = ed.selection.iter().any(|&e| touches(e));
        let visible = if ed.dragging.is_some() {
            // Mid-drag: only constraints actively channeling THIS gesture
            // (or an explicitly clicked chip) surface — static selection
            // visibility would just clutter the resize.
            active || clicked
        } else {
            active || clicked || hovered || sel_touched
        };
        let emphasized = visible && (active || clicked || sel_touched);

        let key = format!(
            "cmark-{}-{}-{}-{}-{}",
            c.kind.as_str(),
            c.a.idx,
            c.a.generation,
            c.b.idx,
            c.b.generation
        );
        let is_hovered = ed.hovered_constraint.as_deref() == Some(key.as_str());
        ed.constraint_markers.push(ConstraintMarker {
            key,
            constraint: c,
            vertical: c.kind != crate::core::constraints::ConstraintKind::Vertical,
            coincident: coincident_pt,
            cx_out: cx,
            cy_out: cy,
            cx_in: cx,
            cy_in: cy,
            flipped: false,
            visible,
            emphasized,
            clicked,
            guide,
            hovered: is_hovered,
        });
    }
}

fn selection_is_ruler(ed: &Editor) -> bool {
    ed.selection.len() == 1
        && ed
            .selection[0]
            .as_segment()
            .is_some_and(|sid| ruler::is_ruler(&ed.doc, sid))
}

// Slanted length dim for a line/edge: runs parallel to the edge at full
// length. Offset side is chosen by DOMINANT AXIS so it never flips on
// near-90-degree jitter: vertical-ish edges put the dim on the RIGHT,
// horizontal-ish edges put it BELOW.
fn push_line_dim(ed: &mut Editor, a: Point2, b: Point2) {
    if pick::distance(a, b) <= 1e-6 {
        return;
    }
    let vertical_edge = (b.x - a.x).abs() <= (b.y - a.y).abs();
    // linear_dim offsets along the LEFT normal of (a, b): (-dy, dx)/len.
    // Pick endpoint order so that normal points +x (vertical edge) or
    // +y (horizontal edge).
    let (a, b) = if vertical_edge {
        // want -dy > 0  =>  order descending y.
        if a.y < b.y { (b, a) } else { (a, b) }
    } else {
        // want dx > 0  =>  order ascending x.
        if a.x > b.x { (b, a) } else { (a, b) }
    };
    ed.dim_renders.push(linear_dim(
        &ed.doc,
        &ed.camera,
        a,
        b,
        PREVIEW_DIM_OFFSET_DOC,
        pick::distance(a, b),
    ));
}

// Bounds shown by preview dims: the pending rubber band only.
fn preview_bounds(ed: &Editor) -> Option<Rect> {
    if let Some(p) = &ed.pending_shape {
        let b = p.bounds();
        return (b.size.w > 0. && b.size.h > 0.).then_some(b);
    }
    None
}

/// Merges resize-dim candidates whose endpoints ALIGN: same angle and the
/// same projected span along that direction (rectangle top/bottom, or two
/// stacked identical lines). Keeps the one closest to bottom-right.
/// Merely-parallel-but-offset edges each keep their own dim.
fn dedup_collinear(edges: Vec<(Point2, Point2)>) -> Vec<(Point2, Point2)> {
    // Projected [lo, hi] interval of an edge along the FIRST edge's
    // direction, plus center sum for the bottom-right tiebreak.
    let mut out: Vec<(Point2, Point2)> = Vec::new();
    for (a, b) in edges {
        let mut merged = false;
        for o in out.iter_mut() {
            let d1 = Point2::new(b.x - a.x, b.y - a.y);
            let d2 = Point2::new(o.1.x - o.0.x, o.1.y - o.0.y);
            let l1 = (d1.x * d1.x + d1.y * d1.y).sqrt();
            let l2 = (d2.x * d2.x + d2.y * d2.y).sqrt();
            if l1 < 1e-9 || l2 < 1e-9 {
                continue;
            }
            let cross = d1.x * d2.y - d1.y * d2.x;
            if cross.abs() / (l1 * l2) > 1e-6 {
                continue; // not parallel
            }
            // Projected spans along d1 (sign-normalized).
            let u = Point2::new(d1.x / l1, d1.y / l1);
            let span = |p0: Point2, p1: Point2| {
                let t0 = (p0.x - a.x) * u.x + (p0.y - a.y) * u.y;
                let t1 = (p1.x - a.x) * u.x + (p1.y - a.y) * u.y;
                (t0.min(t1), t0.max(t1))
            };
            let (lo_new, hi_new) = span(a, b);
            let (lo_old, hi_old) = span(o.0, o.1);
            if (lo_new - lo_old).abs() > 0.5 || (hi_new - hi_old).abs() > 0.5 {
                continue; // parallel but offset
            }
            // Keep the bottom-right-most center.
            let c_new = (a.x + b.x) / 2. + (a.y + b.y) / 2.;
            let c_old = (o.0.x + o.1.x) / 2. + (o.0.y + o.1.y) / 2.;
            if c_new > c_old {
                *o = (a, b);
            }
            merged = true;
            break;
        }
        if !merged {
            out.push((a, b));
        }
    }
    out
}

/// Builds screen-space render data for a dimension between two doc points.
/// `offset_doc` shifts the dim line along the LEFT normal of b-a; `value`
/// is the displayed measurement.
fn linear_dim(doc: &Document, cam: &super::Camera, a: Point2, b: Point2, offset_doc: f64, value: f64) -> DimRender {
    let scr = |p: Point2| cam.unit_to_screen(p);
    let sa = scr(a);
    let sb = scr(b);
    let dx = sb.x - sa.x;
    let dy = sb.y - sa.y;
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    // Left normal in screen space (y down).
    let nx = -dy / len;
    let ny = dx / len;
    let off = offset_doc * cam.zoom;
    let lax = sa.x + nx * off;
    let lay = sa.y + ny * off;
    let lbx = sb.x + nx * off;
    let lby = sb.y + ny * off;
    DimRender {
        ax: sa.x as f32,
        ay: sa.y as f32,
        bx: sb.x as f32,
        by: sb.y as f32,
        lax: lax as f32,
        lay: lay as f32,
        lbx: lbx as f32,
        lby: lby as f32,
        label_cx: ((lax + lbx) / 2.) as f32,
        label_cy: ((lay + lby) / 2.) as f32,
        text: crate::ui::canvas::fmt_dim(value),
        extra_ext: Vec::new(),
    }
}

fn dist(a: Point2, b: Point2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rect_edges_merge_to_two() {
        let tl = Point2::new(0., 0.);
        let tr = Point2::new(100., 0.);
        let br = Point2::new(100., 80.);
        let bl = Point2::new(0., 80.);
        let edges = vec![(tl, tr), (tr, br), (br, bl), (bl, tl)];
        let merged = dedup_collinear(edges);
        assert_eq!(merged.len(), 2, "merged: {merged:?}");
    }

    #[test]
    fn offset_parallel_edges_stay_separate() {
        let a = Point2::new(0., 0.);
        let b = Point2::new(100., 0.);
        // Same span on x but shifted 10 units down: NOT aligned endpoints.
        let c = Point2::new(3., 10.);
        let d = Point2::new(97., 10.);
        let merged = dedup_collinear(vec![(a, b), (c, d)]);
        assert_eq!(merged.len(), 2, "merged: {merged:?}");
    }
}
