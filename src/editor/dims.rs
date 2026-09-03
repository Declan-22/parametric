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

const DIM_OFFSET_PX: f64 = 18.0;

/// Screen-space render data for one constraint chip. `out` is the
/// preferred spot (just off the constrained edge's outer side); `in_` is
/// the flipped spot used when `out` would collide with a dim container or
/// another chip. The layer animates between them.
#[derive(Clone, Debug)]
pub struct ConstraintMarker {
    pub key: String,
    pub constraint: crate::core::constraints::Constraint,
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
    // Constraint ink (empty_text_secondary) for TOOL-created dimensions;
    // transient measurement previews stay accent.
    pub constraint: bool,
    // Some for tool-created dims (index into doc.dimensions) — drives
    // hover recolor, editing, and hit-testing. None for transient previews.
    pub dim_index: Option<usize>,
    pub hovered: bool,
    pub editing: bool,
}

/// Screen-space render data for one ANGLE dimension: a dashed arc between
/// the two lines with the value container riding on it.
#[derive(Clone, Debug)]
pub struct AngleDimRender {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub a0: f32,
    pub sweep: f32,
    pub label_cx: f32,
    pub label_cy: f32,
    pub text: String,
    pub constraint: bool,
    pub dim_index: Option<usize>,
    pub hovered: bool,
    pub editing: bool,
}

pub fn update(ed: &mut Editor) {
    ed.dim_renders.clear();
    ed.angle_dim_renders.clear();

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
        let mut edges: Vec<(Point2, Point2, Option<Point2>)> = Vec::new();
        for (sid, seg) in ed.doc.all_segments() {
            if seg.kind == crate::core::document::SegmentKind::Ruler
                || seg.kind == crate::core::document::SegmentKind::Arc
            {
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
                let prefer = edge_outward_normal(ed, sid)
                    .or_else(|| partner_vote_normal(ed, sid));
                edges.push((ca, cb, prefer));
            }
        }
        for (a, b, prefer) in dedup_collinear(edges) {
            push_line_dim(ed, a, b, prefer);
        }
    }

    // Circle tool stage 3: true radius (center -> cursor).
    if let Some(pc) = &ed.pending_circle
        && let (Some(a), Some(b)) = (pc.a, pc.b)
        && let Some((center, r)) = crate::editor::arc::circumcircle(a, b, pc.cursor)
    {
        ed.dim_renders.push(linear_dim(
            &ed.doc, &ed.camera, center, pc.cursor, 0., r,
        ));
    }

    // Arc resize: dragging ANY point of an arc (endpoints, on-arc ctrl,
    // or center) shows its radius — so unconstrained drags, chord moves,
    // and center drags all get feedback.
    if let Some(drag) = &ed.dragging {
        let dragged: std::collections::HashSet<_> =
            drag.points.iter().map(|(id, _)| *id).collect();
        for (seg_id, seg) in ed.doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Arc {
                continue;
            }
            // A persisted radius dimension is the authoritative annotation;
            // suppress the temporary resize accent so the two annotations do
            // not overlap or disagree while the arc is dragged.
            if ed.doc.dimensions.iter().any(|d| {
                matches!(
                    d.target,
                    crate::core::constraints::DimTarget::Radius { seg: sid } if sid == seg_id
                )
            }) {
                continue;
            }
            let is_dragged = seg.ctrl.is_some_and(|c| dragged.contains(&c))
                || dragged.contains(&seg.start)
                || dragged.contains(&seg.end)
                || seg.center.is_some_and(|c| dragged.contains(&c));
            if !is_dragged {
                continue;
            }
            let (Some(a), Some(b), Some(c)) = (
                ed.doc.point(seg.start),
                ed.doc.point(seg.end),
                seg.ctrl.and_then(|id| ed.doc.point(id)),
            ) else {
                continue;
            };
            if let Some((center, r)) = crate::editor::arc::circumcircle(a, b, c) {
                // Radius dim from true center to the on-arc point.
                ed.dim_renders.push(linear_dim(
                    &ed.doc, &ed.camera, center, c, 0., r,
                ));
            }
        }
    }

    // Live line preview: slanted length dim while drawing.
    if let Some(p) = &ed.pending_line {
        let (_, b) = p.snapped(ed.shift);
        push_line_dim(ed, p.start, b, None);
    }

    // Live preview: bounding box W/H while creating or interacting.
    if let Some(b) = preview_bounds(ed) {
        if b.size.w > 0. {
            ed.dim_renders.push(linear_dim(
                &ed.doc,
                &ed.camera,
                Point2::new(b.origin.x, b.origin.y + b.size.h),
                Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h),
                DIM_OFFSET_PX,
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
                DIM_OFFSET_PX,
                b.size.h,
            ));
        }
    }

    // Stored dimensions: rendered at their own angle and placement.
    let dims = ed.doc.dimensions.clone();
    for (i, d) in dims.iter().enumerate() {
        let editing = ed
            .dim_input.as_ref()
            .is_some_and(|input| input.existing == Some(i));
        let text = if editing {
            // Editing shows the current value highlighted; typing replaces it.
            let buffer = ed.dim_input.as_ref().map(|input| input.buffer.clone()).unwrap_or_default();
            Some(if buffer.is_empty() {
                match &d.target {
                    // Angles always read as positive 0..360 — the signed
                    // sweep only encodes which sector the arc occupies.
                    crate::core::constraints::DimTarget::Angle { .. } => {
                        format!("{:.1}\u{00B0}", d.value.abs())
                    }
                    _ => crate::ui::canvas::fmt_dim(d.value),
                }
            } else {
                buffer
            })
        } else {
            None
        };
        let hovered = ed.hovered_dim == Some(i)
            || ed.selected_dim == Some(i)
            || ed.dim_drag.as_ref().map(|d| d.index) == Some(i);
        push_dim_target(ed, &d.target, d.offset, d.slide, d.value, text, Some(i), hovered, editing);
    }

    // Dimension tool: the frozen value-input state, then the live preview
    // following the cursor while a pick pair is assembled.
    if ed.tool == crate::editor::Tool::Dimension {
        if let Some(input) = ed.dim_input.clone() {
            let text = if input.buffer.is_empty() {
                None
            } else {
                Some(input.buffer.clone())
            };
            let idx = input.existing;
            push_dim_target(ed, &input.target, input.offset, input.slide, input.measured, text, idx, true, true);
        } else if let Some(target) = ed.dim_target
            && let Some(cur) = ed.last_cursor
        {
            let doc_p = ed
                .camera
                .screen_to_unit(Point2::new(f64::from(cur.x), f64::from(cur.y)));
            if let Some((mode, offset, slide, measured)) = ed.dim_placement(target, doc_p) {
                // The cursor-decided mode MUST ride into the render target —
                // without it the X/Y preview would draw as an aligned dim.
                push_dim_target(
                    ed,
                    &target.with_mode(mode),
                    offset,
                    slide,
                    measured,
                    None,
                    None,
                    false,
                    false,
                );
            }
        }
    }

    update_constraint_markers(ed);
}

/// Renders one dimension target (stored, preview, or value-input) at the
/// given placement. All tool-created dims use the constraint ink
/// (empty_text_secondary); `text` overrides the formatted value (typed
/// input). Distances get the standard px/in container; angles get the
/// dashed arc with a degree readout.
fn push_dim_target(
    ed: &mut Editor,
    target: &crate::core::constraints::DimTarget,
    offset: f64,
    slide: f64,
    value: f64,
    text: Option<String>,
    dim_index: Option<usize>,
    hovered: bool,
    editing: bool,
) {
    use crate::core::constraints::DimTarget;
    let zoom = ed.camera.zoom;
    match target {
        DimTarget::Points { a, b, mode } => {
            let (Some(pa), Some(pb)) = (ed.doc.point(*a), ed.doc.point(*b)) else {
                return;
            };
            let zoom = ed.camera.zoom;
            let scr = |p: Point2| ed.camera.unit_to_screen(p);
            use crate::core::constraints::DimMode;
            let mut r = match mode {
                DimMode::Aligned => {
                    let mut r = linear_dim(&ed.doc, &ed.camera, pa, pb, offset * zoom, value);
                    r.text = text.unwrap_or_else(|| crate::ui::canvas::fmt_dim(value));
                    // Container rides the dim line FROM ITS START (the first
                    // endpoint's foot) — anchoring it to the line's midpoint made
                    // it unreachable past halfway.
                    let (sa, sb) = (scr(pa), scr(pb));
                    let dx = sb.x - sa.x;
                    let dy = sb.y - sa.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let s = slide.clamp(0., pick::distance(pa, pb));
                    r.label_cx = r.lax + (dx / l * s * zoom) as f32;
                    r.label_cy = r.lay + (dy / l * s * zoom) as f32;
                    r
                }
                // X/Y modes: the dim line is an axis-aligned span measuring
                // |dx| / |dy|, with vertical/horizontal extension stubs
                // down to each measured point. `offset` is the signed
                // distance of the dim line from the FIRST point along the
                // other axis; `slide` positions the container along it.
                DimMode::X => {
                    let y_off = pa.y + offset;
                    let (sa, sb) = (scr(pa), scr(pb));
                    let ly = scr(Point2::new(pa.x, y_off)).y;
                    let (lax, lbx) = (sa.x, sb.x);
                    let t = (slide / (pb.x - pa.x).abs().max(1e-9)).clamp(0., 1.);
                    DimRender {
                        ax: sa.x as f32,
                        ay: sa.y as f32,
                        bx: sb.x as f32,
                        by: sb.y as f32,
                        lax: lax as f32,
                        lay: ly as f32,
                        lbx: lbx as f32,
                        lby: ly as f32,
                        label_cx: (lax + (lbx - lax) * t) as f32,
                        label_cy: ly as f32,
                        text: text.unwrap_or_else(|| crate::ui::canvas::fmt_dim(value)),
                        extra_ext: Vec::new(),
                        constraint: true,
                        dim_index,
                        hovered,
                        editing,
                    }
                }
                DimMode::Y => {
                    let x_off = pa.x + offset;
                    let (sa, sb) = (scr(pa), scr(pb));
                    let lx = scr(Point2::new(x_off, pa.y)).x;
                    let (lay, lby) = (sa.y, sb.y);
                    let t = (slide / (pb.y - pa.y).abs().max(1e-9)).clamp(0., 1.);
                    DimRender {
                        ax: sa.x as f32,
                        ay: sa.y as f32,
                        bx: sb.x as f32,
                        by: sb.y as f32,
                        lax: lx as f32,
                        lay: lay as f32,
                        lbx: lx as f32,
                        lby: lby as f32,
                        label_cx: lx as f32,
                        label_cy: (lay + (lby - lay) * t) as f32,
                        text: text.unwrap_or_else(|| crate::ui::canvas::fmt_dim(value)),
                        extra_ext: Vec::new(),
                        constraint: true,
                        dim_index,
                        hovered,
                        editing,
                    }
                }
            };
            r.constraint = true;
            r.dim_index = dim_index;
            r.hovered = hovered;
            r.editing = editing;
            ed.dim_renders.push(r);
        }
        DimTarget::PointLine { p, line } => {
            let (Some(sp), Some((la, lb))) =
                (ed.doc.point(*p), ed.doc.segment_geom(*line))
            else {
                return;
            };
            let (u, n) = dim_axes(lb.x - la.x, lb.y - la.y);
            let prel = (sp.x - la.x, sp.y - la.y);
            let measured = prel.0 * n.0 + prel.1 * n.1;
            // Measured span: the point to its perpendicular foot on the
            // line. Dim line: parallel to the line, from the placed slide
            // position to the point's parallel — extension stubs close the
            // path.
            let foot = Point2::new(la.x + u.0 * prel.0, la.y + u.1 * prel.1);
            let f = Point2::new(la.x + u.0 * slide, la.y + u.1 * slide);
            let p2 = Point2::new(f.x + n.0 * measured, f.y + n.1 * measured);
            // Container rides the perpendicular dim line at the cursor's
            // placed height, clamped onto the segment (never past its ends).
            let ride = offset.clamp(0.0, measured.abs());
            let lp = Point2::new(f.x + n.0 * ride, f.y + n.1 * ride);
            let (s, e, fp, f2) = (
                ed.camera.unit_to_screen(sp),
                ed.camera.unit_to_screen(foot),
                ed.camera.unit_to_screen(f),
                ed.camera.unit_to_screen(p2),
            );
            let lps = ed.camera.unit_to_screen(lp);
            ed.dim_renders.push(DimRender {
                ax: s.x as f32,
                ay: s.y as f32,
                bx: e.x as f32,
                by: e.y as f32,
                lax: f2.x as f32,
                lay: f2.y as f32,
                lbx: fp.x as f32,
                lby: fp.y as f32,
                label_cx: lps.x as f32,
                label_cy: lps.y as f32,
                text: text.unwrap_or_else(|| crate::ui::canvas::fmt_dim(value)),
                extra_ext: Vec::new(),
                constraint: true,
                dim_index,
                hovered,
                editing,
            });
        }
        DimTarget::Lines { a, b } => {
            let (Some((a0, _)), Some((b0, b1))) =
                (ed.doc.segment_geom(*a), ed.doc.segment_geom(*b))
            else {
                return;
            };
            let (u, n) = dim_axes(b1.x - b0.x, b1.y - b0.y);
            let gap = (b0.x - a0.x) * n.0 + (b0.y - a0.y) * n.1;
            // Measured span at the placed slide: perpendicular between the
            // lines. Dim line: parallel to them at the placed offset,
            // extended past both stubs for the arrowheads.
            let p1 = Point2::new(a0.x + u.0 * slide, a0.y + u.1 * slide);
            let p2 = Point2::new(p1.x + n.0 * gap, p1.y + n.1 * gap);
            let ext = 14. / zoom;
            let e1 = Point2::new(p1.x + n.0 * offset - u.0 * ext, p1.y + n.1 * offset - u.1 * ext);
            let e2 = Point2::new(p2.x + n.0 * offset + u.0 * ext, p2.y + n.1 * offset + u.1 * ext);
            let (s1, s2, fe1, fe2) = (
                ed.camera.unit_to_screen(p1),
                ed.camera.unit_to_screen(p2),
                ed.camera.unit_to_screen(e1),
                ed.camera.unit_to_screen(e2),
            );
            ed.dim_renders.push(DimRender {
                ax: s1.x as f32,
                ay: s1.y as f32,
                bx: s2.x as f32,
                by: s2.y as f32,
                lax: fe1.x as f32,
                lay: fe1.y as f32,
                lbx: fe2.x as f32,
                lby: fe2.y as f32,
                label_cx: ((fe1.x + fe2.x) / 2.) as f32,
                label_cy: ((fe1.y + fe2.y) / 2.) as f32,
                text: text.unwrap_or_else(|| crate::ui::canvas::fmt_dim(value)),
                extra_ext: Vec::new(),
                constraint: true,
                dim_index,
                hovered,
                editing,
            });
        }
        DimTarget::Angle { a, b } => {
            // `value` carries the SIGNED sweep (degrees) for angle dims.
            let Some((v, da, sweep, frac, r_doc)) = dim_angle_geometry(
                ed,
                *a,
                *b,
                None,
                value.to_radians(),
                offset.abs(),
                slide,
            ) else {
                return;
            };
            let a_ang = da.1.atan2(da.0);
            let zoom = ed.camera.zoom;
            let vc = ed.camera.unit_to_screen(v);
            let r_px = (r_doc * zoom) as f32;
            let th = a_ang + sweep * frac;
            let lp = Point2::new(
                vc.x + r_px as f64 * th.cos(),
                vc.y + r_px as f64 * th.sin(),
            );
            ed.angle_dim_renders.push(AngleDimRender {
                cx: vc.x as f32,
                cy: vc.y as f32,
                r: r_px,
                a0: a_ang as f32,
                sweep: sweep as f32,
                label_cx: lp.x as f32,
                label_cy: lp.y as f32,
                text: text.unwrap_or_else(|| format!("{:.1}\u{00B0}", value.abs())),
                constraint: true,
                dim_index,
                hovered,
                editing,
            });
        }
        DimTarget::Radius { seg } => {
            let Some(seg_d) = ed.doc.segment(*seg) else {
                return;
            };
            let (Some(a), Some(b)) =
                (ed.doc.point(seg_d.start), ed.doc.point(seg_d.end))
            else {
                return;
            };
            let Some(c) = seg_d.ctrl.and_then(|id| ed.doc.point(id)) else {
                return;
            };
            let Some((center, r)) = crate::editor::arc::circumcircle(a, b, c) else {
                return;
            };
            if r < 1e-9 {
                return;
            }
            // Dashed line from the center to the on-arc bend point; the
            // value container rides it at the placed fraction.
            let frac = slide.clamp(0.25, 1.0);
            let sc = ed.camera.unit_to_screen(center);
            let ec = ed.camera.unit_to_screen(c);
            let lp = Point2::new(
                sc.x + (ec.x - sc.x) * frac as f64,
                sc.y + (ec.y - sc.y) * frac as f64,
            );
            ed.dim_renders.push(DimRender {
                ax: sc.x as f32,
                ay: sc.y as f32,
                bx: ec.x as f32,
                by: ec.y as f32,
                lax: sc.x as f32,
                lay: sc.y as f32,
                lbx: ec.x as f32,
                lby: ec.y as f32,
                label_cx: lp.x as f32,
                label_cy: lp.y as f32,
                text: text.unwrap_or_else(|| crate::ui::canvas::fmt_dim(value)),
                extra_ext: Vec::new(),
                constraint: true,
                dim_index,
                hovered,
                editing,
            });
        }
    }
}

/// Unit direction + LEFT normal of a vector (doc space is y-down like the
/// screen, so the same handedness applies).
pub(crate) fn dim_axes(dx: f64, dy: f64) -> ((f64, f64), (f64, f64)) {
    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
    let u = (dx / l, dy / l);
    (u, (-u.1, u.0))
}

/// One ray of an angle dimension: a point on the line plus its unit
/// direction.
#[allow(dead_code)]
fn dim_angle_ray(ed: &Editor, sid: crate::core::ids::SegmentId) -> Option<(Point2, (f64, f64))> {
    let (a, b) = ed.doc.segment_geom(sid)?;
    let (u, _) = dim_axes(b.x - a.x, b.y - a.y);
    Some((a, u))
}

/// Full angle-dimension geometry in the RAW ray convention: rays point
/// along each segment's start->end direction from the intersection vertex,
/// the SAME convention the solver's angle equation uses - label, arc and
/// constraint can never disagree. Returns the vertex, ray A's unit
/// direction, the SIGNED sweep (radians) on the picked side, the
/// container's fractional position along it, and the arc radius in doc
/// units. During placement the live cursor picks the sweep's sector,
/// magnitude and fraction; stored/input dims use the stored signed sweep.
pub(crate) fn dim_angle_geometry(
    ed: &Editor,
    a: crate::core::ids::SegmentId,
    b: crate::core::ids::SegmentId,
    cursor: Option<Point2>,
    sweep_stored_rad: f64,
    radius_doc: f64,
    slide: f64,
) -> Option<(Point2, (f64, f64), f64, f64, f64)> {
    const TAU: f64 = std::f64::consts::TAU;
    let (a0p, a1p) = ed.doc.segment_geom(a)?;
    let (b0p, b1p) = ed.doc.segment_geom(b)?;
    let (mut u_a, _) = dim_axes(a1p.x - a0p.x, a1p.y - a0p.y);
    let (mut u_b, _) = dim_axes(b1p.x - b0p.x, b1p.y - b0p.y);
    let v = line_intersection(a0p, u_a, b0p, u_b)?;
    // VERTEX-ORIENTED rays: each ray points from the intersection TOWARD
    // that segment's actual geometry (its midpoint). The raw start->end
    // direction can point away from the corner (an edge emitted
    // bottom-to-top at a top-left corner), which made rectangle corner
    // dims report the reflex sector (270/-90) and draw the arc on the
    // wrong side of the edges.
    let orient = |u: (f64, f64), p0: Point2, p1: Point2| -> (f64, f64) {
        let mid = Point2::new((p0.x + p1.x) / 2., (p0.y + p1.y) / 2.);
        if (mid.x - v.x) * u.0 + (mid.y - v.y) * u.1 < 0. {
            (-u.0, -u.1)
        } else {
            u
        }
    };
    u_a = orient(u_a, a0p, a1p);
    u_b = orient(u_b, b0p, b1p);
    // Raw signed angle between the two ray directions (-PI, PI].
    let full = (u_a.0 * u_b.1 - u_a.1 * u_b.0).atan2(u_a.0 * u_b.0 + u_a.1 * u_b.1);
    if full.abs() < 1e-6 {
        return None;
    }
    let a_ang = u_a.1.atan2(u_a.0);
    let norm = |mut t: f64| {
        while t < 0. {
            t += TAU;
        }
        while t >= TAU {
            t -= TAU;
        }
        t
    };
    let (sweep, r_doc, frac) = if let Some(cur) = cursor {
        let dx = cur.x - v.x;
        let dy = cur.y - v.y;
        let r = (dx * dx + dy * dy).sqrt();
        if r < 1e-9 {
            return None;
        }
        let rel = norm(dy.atan2(dx) - a_ang);
        // The two candidate sweeps between the rays: +|full| (CCW from
        // ray A) and -(|full| + (TAU - |full|) - |full|)... simply: the
        // CW complement (TAU - |full|), signed opposite to full.
        let (sweep, frac) = if full >= 0. {
            if rel <= full {
                (full, (rel / full).clamp(0., 1.))
            } else {
                (full - TAU, ((TAU - rel) / (TAU - full)).clamp(0., 1.))
            }
        } else {
            let span = TAU + full; // |full|
            if rel >= span {
                (full, ((rel - span) / -full).clamp(0., 1.))
            } else {
                (full + TAU, (rel / span).clamp(0., 1.))
            }
        };
        if sweep.abs() < 1e-6 {
            return None;
        }
        (sweep, r, frac)
    } else {
        let r = radius_doc.abs();
        if r < 1e-9 {
            return None;
        }
        (sweep_stored_rad, r, slide.clamp(0., 1.))
    };
    Some((v, u_a, sweep, frac, r_doc))
}
/// Intersection of two infinite lines p1 + t*d1 and p2 + s*d2.
pub(crate) fn line_intersection(
    p1: Point2,
    d1: (f64, f64),
    p2: Point2,
    d2: (f64, f64),
) -> Option<Point2> {
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = ((p2.x - p1.x) * d2.1 - (p2.y - p1.y) * d2.0) / denom;
    Some(Point2::new(p1.x + d1.0 * t, p1.y + d1.1 * t))
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
        let (cx, cy) = if c.kind == crate::core::constraints::ConstraintKind::Parallel
            && let Some((first, second)) = c.tangent_segments
            && let (Some((fa, fb)), Some((sa, sb))) = (ed.doc.segment_geom(first), ed.doc.segment_geom(second))
        {
            let fm = ed.camera.unit_to_screen(Point2::new((fa.x + fb.x) / 2., (fa.y + fb.y) / 2.));
            let sm = ed.camera.unit_to_screen(Point2::new((sa.x + sb.x) / 2., (sa.y + sb.y) / 2.));
            guide = Some([fm.x as f32, fm.y as f32, sm.x as f32, sm.y as f32]);
            (((fm.x + sm.x) / 2.) as f32, ((fm.y + sm.y) / 2.) as f32)
        } else if coincident_pt {
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
    // Multiple constraints attached to one point share a single horizontal
    // chip row. The previous placement gave every chip the same point-based
    // anchor, so the overlay stacked vertically and obscured itself.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, marker) in ed.constraint_markers.iter().enumerate() {
        // Group only chips that are physically attached to the same anchor.
        // Sharing an endpoint is not sufficient: adjacent rectangle edges
        // share corners but their H/V chips belong on their own edge. Point
        // constraints use the same screen anchor; duplicate constraints on
        // one exact edge may share its edge midpoint.
        if let Some(group) = groups.iter_mut().find(|group| {
            let first = &ed.constraint_markers[group[0]];
            let first_point = first.constraint.a == first.constraint.b
                || first.constraint.kind == crate::core::constraints::ConstraintKind::Coincident;
            let marker_point = marker.constraint.a == marker.constraint.b
                || marker.constraint.kind == crate::core::constraints::ConstraintKind::Coincident;
            let same_point = first_point && marker_point
                && (first.cx_out - marker.cx_out).abs() < 1.0
                && (first.cy_out - marker.cy_out).abs() < 1.0;
            let same_edge = first.constraint.tangent_segments.is_none()
                && marker.constraint.tangent_segments.is_none()
                && ((first.constraint.a == marker.constraint.a && first.constraint.b == marker.constraint.b)
                    || (first.constraint.a == marker.constraint.b && first.constraint.b == marker.constraint.a));
            same_point || same_edge
        }) {
            group.push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    for indices in &groups {
        if indices.len() < 2 { continue; }
        let base_x = indices.iter().map(|&i| ed.constraint_markers[i].cx_out).sum::<f32>()
            / indices.len() as f32;
        let base_y = indices.iter().map(|&i| ed.constraint_markers[i].cy_out).sum::<f32>()
            / indices.len() as f32;
        let width = 22.0_f32;
        let start = base_x - width * (indices.len() as f32 - 1.0) / 2.0;
        for (row, &i) in indices.iter().enumerate() {
            ed.constraint_markers[i].cx_out = start + row as f32 * width;
            ed.constraint_markers[i].cy_out = base_y;
            ed.constraint_markers[i].cx_in = ed.constraint_markers[i].cx_out;
            ed.constraint_markers[i].cy_in = base_y;
        }
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
// length. Side selection, in priority order:
//  1. `prefer` normal (screen space) — outward of a fill or away from
//     connected partners;
//  2. otherwise DOMINANT AXIS so lone lines never flip on near-90-degree
//     jitter (vertical-ish edges -> RIGHT, horizontal-ish -> BELOW).
fn push_line_dim(ed: &mut Editor, a: Point2, b: Point2, prefer: Option<Point2>) {
    if pick::distance(a, b) <= 1e-6 {
        return;
    }
    let left_normal_agrees = |a: Point2, b: Point2| -> bool {
        let n = prefer.unwrap();
        // Left normal of screen-space a->b is (-dy, dx).
        let sa = ed.camera.unit_to_screen(a);
        let sb = ed.camera.unit_to_screen(b);
        let dx = sb.x - sa.x;
        let dy = sb.y - sa.y;
        -dy * n.x + dx * n.y > 0.
    };
    let (a, b) = match prefer {
        Some(n) => {
            if left_normal_agrees(a, b) { (a, b) } else { (b, a) }
        }
        None => {
            let vertical_edge = (b.x - a.x).abs() <= (b.y - a.y).abs();
            if vertical_edge {
                if a.y < b.y { (b, a) } else { (a, b) }
            } else {
                if a.x > b.x { (b, a) } else { (a, b) }
            }
        }
    };
    ed.dim_renders.push(linear_dim(
        &ed.doc,
        &ed.camera,
        a,
        b,
        DIM_OFFSET_PX,
        pick::distance(a, b),
    ));
}

/// Tier-1 side rule: when the segment is an edge of a fill loop, return
/// the OUTWARD screen-space normal derived from the loop's winding.
fn edge_outward_normal(ed: &Editor, sid: crate::core::ids::SegmentId) -> Option<Point2> {
    let seg = ed.doc.segment(sid)?;
    let (Some(pa), Some(pb)) = (ed.doc.point(seg.start), ed.doc.point(seg.end)) else {
        return None;
    };
    for (fid, f) in ed.doc.all_fills() {
        if !f.segments.contains(&sid) {
            continue;
        }
        let Some(pts) = pick::loop_points(&ed.doc, fid) else {
            continue;
        };
        if pts.len() < 3 {
            continue;
        }
        // Winding sign (shoelace, y-down): positive => interior on LEFT of
        // each traversal edge, so outward = right normal; negative flips.
        let mut area = 0.;
        for i in 0..pts.len() {
            let p = pts[i];
            let q = pts[(i + 1) % pts.len()];
            area += p.x * q.y - q.x * p.y;
        }
        if area.abs() < 1e-9 {
            continue;
        }
        // Traversal direction across THIS edge (either orientation).
        let forward = pts.iter().enumerate().any(|(i, p)| {
            *p == pa && pts[(i + 1) % pts.len()] == pb
        });
        let d = if forward {
            Point2::new(pb.x - pa.x, pb.y - pa.y)
        } else {
            Point2::new(pa.x - pb.x, pa.y - pb.y)
        };
        let outward = if area > 0. {
            Point2::new(d.y, -d.x) // right normal
        } else {
            Point2::new(-d.y, d.x) // left normal
        };
        // Convert to SCREEN space direction (same handedness; just scale by zoom>0, so direction unchanged).
        let len = (outward.x * outward.x + outward.y * outward.y).sqrt();
        return Some(Point2::new(outward.x / len, outward.y / len));
    }
    None
}

/// Tier-2 side rule: no fill membership but connected partners — vote from
/// each partner's far endpoint; the dim goes on the side AWAY from them.
fn partner_vote_normal(ed: &Editor, sid: crate::core::ids::SegmentId) -> Option<Point2> {
    let seg = ed.doc.segment(sid)?;
    let (Some(pa), Some(pb)) = (ed.doc.point(seg.start), ed.doc.point(seg.end)) else {
        return None;
    };
    let mut vote = 0.;
    for (other_sid, other) in ed.doc.all_segments() {
        if other_sid == sid || other.kind == crate::core::document::SegmentKind::Ruler {
            continue;
        }
        let Some((oa, ob)) = ed.doc.segment_geom(other_sid) else {
            continue;
        };
        // Shares exactly one endpoint with our edge?
        let far = if other.start == seg.start || other.end == seg.start {
            Some((if other.start == seg.start { ob } else { oa }, pa))
        } else if other.start == seg.end || other.end == seg.end {
            Some((if other.start == seg.end { ob } else { oa }, pb))
        } else {
            None
        };
        if let Some((far_pt, joint)) = far {
            if pick::distance(far_pt, joint) < 1e-9 {
                continue;
            }
            let d = Point2::new(pb.x - pa.x, pb.y - pa.y);
            let w = Point2::new(far_pt.x - joint.x, far_pt.y - joint.y);
            // cross(d, w) > 0 => partner lies on LEFT-normal side.
            vote += d.x * w.y - d.y * w.x;
        }
    }
    if vote.abs() < 1e-9 {
        return None;
    }
    // Away from partners: opposite the majority side.
    let l = Point2::new(-(pb.y - pa.y), pb.x - pa.x);
    let len = (l.x * l.x + l.y * l.y).sqrt();
    let s = if vote > 0. { -1. } else { 1. };
    Some(Point2::new(l.x / len * s, l.y / len * s))
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
fn dedup_collinear(edges: Vec<(Point2, Point2, Option<Point2>)>) -> Vec<(Point2, Point2, Option<Point2>)> {
    // Projected [lo, hi] interval of an edge along the FIRST edge's
    // direction, plus center sum for the bottom-right tiebreak.
    let mut out: Vec<(Point2, Point2, Option<Point2>)> = Vec::new();
    for (a, b, pref) in edges {
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
                *o = (a, b, pref);
            }
            merged = true;
            break;
        }
        if !merged {
            out.push((a, b, pref));
        }
    }
    out
}

/// Builds screen-space render data for a dimension between two doc points.
/// `offset_px` is a constant screen-space distance along the LEFT normal of
/// b-a, intentionally NOT scaled by zoom so the dim line and its container
/// sit at the same pixel offset at any zoom (like a CAD overlay).
fn linear_dim(doc: &Document, cam: &super::Camera, a: Point2, b: Point2, offset_px: f64, value: f64) -> DimRender {
    let scr = |p: Point2| cam.unit_to_screen(p);
    let sa = scr(a);
    let sb = scr(b);
    let dx = sb.x - sa.x;
    let dy = sb.y - sa.y;
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    // Left normal in screen space (y down).
    let nx = -dy / len;
    let ny = dx / len;
    let off = offset_px;
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
        constraint: false,
        dim_index: None,
        hovered: false,
        editing: false,
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
        let edges = vec![
            (tl, tr, None),
            (tr, br, None),
            (br, bl, None),
            (bl, tl, None),
        ];
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
        let merged = dedup_collinear(vec![(a, b, None), (c, d, None)]);
        assert_eq!(merged.len(), 2, "merged: {merged:?}");
    }
}
