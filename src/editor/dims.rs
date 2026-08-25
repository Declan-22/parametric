use super::ruler;
use super::Editor;
use crate::core::document::Document;
use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{FillId, PointId};

// Dimension render-data computation. Produces screen-space DimRender
// entries consumed by BOTH the painted lines and the DOM label layer.
// Rulers never participate: no dims while drawing one or with one
// selected — their markings are vector geometry, not dimensions.

const PREVIEW_DIM_OFFSET_DOC: f64 = 18.0;

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

    // Multi-object selection: every object shows its OWN W+H dims, plus a
    // TOTAL pair whose extension lines reach each object's nearest extreme.
    let sel_fills: Vec<FillId> = ed
        .selection
        .iter()
        .filter_map(|el| el.as_fill())
        .filter(|fid| ed.doc.fill(*fid).is_some())
        .collect();
    if !sel_fills.is_empty() {
        for fid in &sel_fills {
            if let Some(b) = ed.doc.fill_bounds(*fid) {
                push_wh_dims(ed, b);
            }
        }
        if sel_fills.len() > 1 {
            push_total_dims(ed, &sel_fills);
        }
        return;
    }

    // A lone selected edge shows the dim of the axis being resized:
    // left/right edges -> WIDTH dim under the shape; top/bottom -> HEIGHT
    // dim right of the shape. Applies WHILE dragging too. Ruler segments
    // are excluded above.
    if ed.pending_shape.is_none()
        && ed.selection.len() == 1
        && let Some(sid) = ed.selection[0].as_segment()
    {
        for (fid, f) in ed.doc.all_fills() {
            if !f.segments.contains(&sid) {
                continue;
            }
            if let (Some(bounds), Some((a, b))) =
                (ed.doc.fill_bounds(fid), ed.doc.segment_geom(sid))
            {
                if (b.x - a.x).abs() <= 1e-9 {
                    let bl = Point2::new(bounds.origin.x, bounds.origin.y + bounds.size.h);
                    let br = Point2::new(
                        bounds.origin.x + bounds.size.w,
                        bounds.origin.y + bounds.size.h,
                    );
                    ed.dim_renders.push(linear_dim(
                        &ed.doc,
                        &ed.camera,
                        bl,
                        br,
                        PREVIEW_DIM_OFFSET_DOC,
                        bounds.size.w,
                    ));
                } else {
                    let tr = Point2::new(bounds.origin.x + bounds.size.w, bounds.origin.y);
                    let br = Point2::new(
                        bounds.origin.x + bounds.size.w,
                        bounds.origin.y + bounds.size.h,
                    );
                    ed.dim_renders.push(linear_dim(
                        &ed.doc,
                        &ed.camera,
                        br,
                        tr,
                        PREVIEW_DIM_OFFSET_DOC,
                        bounds.size.h,
                    ));
                }
                return;
            }
        }
    }

    // Dragging a point of a closed loop: show that loop's W+H dims.
    if ed.pending_shape.is_none()
        && ed.selection.len() == 1
        && let Some(pid) = ed.selection[0].as_point()
        && let Some(fid) = fill_containing_point(&ed.doc, pid)
        && let Some(b) = ed.doc.fill_bounds(fid)
    {
        push_wh_dims(ed, b);
        return;
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
}

fn selection_is_ruler(ed: &Editor) -> bool {
    ed.selection.len() == 1
        && ed
            .selection[0]
            .as_segment()
            .is_some_and(|sid| ruler::is_ruler(&ed.doc, sid))
}

fn push_wh_dims(ed: &mut Editor, b: Rect) {
    if b.size.w > 0. {
        let bl = Point2::new(b.origin.x, b.origin.y + b.size.h);
        let br = Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h);
        ed.dim_renders.push(linear_dim(&ed.doc, &ed.camera, bl, br, PREVIEW_DIM_OFFSET_DOC, b.size.w));
    }
    if b.size.h > 0. {
        let br = Point2::new(b.origin.x + b.size.w, b.origin.y + b.size.h);
        let tr = Point2::new(b.origin.x + b.size.w, b.origin.y);
        ed.dim_renders.push(linear_dim(&ed.doc, &ed.camera, br, tr, PREVIEW_DIM_OFFSET_DOC, b.size.h));
    }
}

fn push_total_dims(ed: &mut Editor, fills: &[FillId]) {
    let mut total: Option<Rect> = None;
    for fid in fills {
        if let Some(b) = ed.doc.fill_bounds(*fid) {
            total = Some(match total {
                Some(t) => t.union(&b),
                None => b,
            });
        }
    }
    let Some(u) = total else { return };
    let off = PREVIEW_DIM_OFFSET_DOC;

    // W dim (bottom): verticals from each object's bottom corners down to
    // the dim line's y.
    let mut extras: Vec<(Point2, Point2)> = Vec::new();
    let dim_y = u.origin.y + u.size.h + off;
    for fid in fills {
        if let Some(b) = ed.doc.fill_bounds(*fid) {
            let by = b.origin.y + b.size.h;
            extras.push((Point2::new(b.origin.x, by), Point2::new(b.origin.x, dim_y)));
            extras.push((
                Point2::new(b.origin.x + b.size.w, by),
                Point2::new(b.origin.x + b.size.w, dim_y),
            ));
        }
    }
    let bl = Point2::new(u.origin.x, u.origin.y + u.size.h);
    let br = Point2::new(u.origin.x + u.size.w, u.origin.y + u.size.h);
    ed.dim_renders.push(linear_dim_extras(ed, bl, br, off, u.size.w, &extras));

    // H dim (right): horizontals from each object's right edge across to
    // the dim line's x.
    let mut extras: Vec<(Point2, Point2)> = Vec::new();
    let dim_x = u.origin.x + u.size.w + off;
    for fid in fills {
        if let Some(b) = ed.doc.fill_bounds(*fid) {
            let rx = b.origin.x + b.size.w;
            extras.push((Point2::new(rx, b.origin.y), Point2::new(dim_x, b.origin.y)));
            extras.push((
                Point2::new(rx, b.origin.y + b.size.h),
                Point2::new(dim_x, b.origin.y + b.size.h),
            ));
        }
    }
    let br = Point2::new(u.origin.x + u.size.w, u.origin.y + u.size.h);
    let tr = Point2::new(u.origin.x + u.size.w, u.origin.y);
    ed.dim_renders.push(linear_dim_extras(ed, br, tr, off, u.size.h, &extras));
}

// Bounds shown by preview dims: pending rubber band, else active drag
// points, else the selection.
fn preview_bounds(ed: &Editor) -> Option<Rect> {
    if let Some(p) = &ed.pending_shape {
        let b = p.bounds();
        return (b.size.w > 0. && b.size.h > 0.).then_some(b);
    }
    let ids: Vec<PointId> = if let Some(drag) = &ed.dragging {
        drag.points.iter().map(|(id, _)| *id).collect()
    } else if !ed.selection.is_empty() {
        ed.doc.selection_points(&ed.selection)
    } else {
        return None;
    };
    ed.doc.bounds_of_points(&ids)
}

fn fill_containing_point(doc: &Document, pid: PointId) -> Option<FillId> {
    doc.all_fills()
        .find(|(_, f)| {
            f.segments.iter().any(|&s| {
                doc.segment(s).is_some_and(|seg| seg.start == pid || seg.end == pid)
            })
        })
        .map(|(id, _)| id)
}

fn linear_dim_extras(
    ed: &Editor,
    a: Point2,
    b: Point2,
    offset_doc: f64,
    value: f64,
    extras: &[(Point2, Point2)],
) -> DimRender {
    let mut d = linear_dim(&ed.doc, &ed.camera, a, b, offset_doc, value);
    for &(p, q) in extras {
        let sp = ed.camera.unit_to_screen(p);
        let sq = ed.camera.unit_to_screen(q);
        d.extra_ext.push([sp.x as f32, sp.y as f32, sq.x as f32, sq.y as f32]);
    }
    d
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
