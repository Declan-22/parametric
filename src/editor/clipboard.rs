use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::core::document::{ContinuityMode, Document, Segment, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::{FillId, PathId, PointId, SegmentId};
use crate::editor::Editor;

// Copy/paste: serializes the ACTUAL selected elements (points, lines,
// arcs, fills, plus constraints internal to the copied set) — not just a
// bounding rectangle. Paste rebuilds everything with fresh ids, offset by
// a small amount so the copy doesn't land exactly on the original.

const OFFSET: f64 = 12.0;

impl Editor {
    pub fn serialize_selection(&self) -> Option<String> {
        if self.selection.is_empty() {
            return None;
        }
        let mut pts: Vec<(PointId, Point2)> = Vec::new();
        let mut segs: Vec<(SegmentId, Segment)> = Vec::new();
        let mut fills: Vec<(FillId, Vec<SegmentId>)> = Vec::new();

        fn push_pt(doc: &Document, pid: PointId, pts: &mut Vec<(PointId, Point2)>) {
            if !pts.iter().any(|(id, _)| *id == pid)
                && let Some(p) = doc.point(pid)
            {
                pts.push((pid, p));
            }
        }

        let mut paths: Vec<(
            PathId,
            Vec<SegmentId>,
            bool,
            Vec<ContinuityMode>,
            Vec<(PointId, PointId)>,
        )> = Vec::new();

        fn push_seg(
            doc: &Document,
            sid: SegmentId,
            pts: &mut Vec<(PointId, Point2)>,
            segs: &mut Vec<(SegmentId, Segment)>,
        ) {
            if let Some(seg) = doc.segment(sid) {
                push_pt(doc, seg.start, pts);
                push_pt(doc, seg.end, pts);
                if let Some(c) = seg.ctrl {
                    push_pt(doc, c, pts);
                }
                if let Some(c) = seg.center {
                    push_pt(doc, c, pts);
                }
                if let Some(h) = seg.handle_out {
                    push_pt(doc, h, pts);
                }
                if let Some(h) = seg.handle_in {
                    push_pt(doc, h, pts);
                }
                if !segs.iter().any(|(id, _)| *id == sid) {
                    segs.push((sid, seg));
                }
            }
        }

        for &el in &self.selection {
            match el {
                ElementRef::Point(pid) => push_pt(&self.doc, pid, &mut pts),
                ElementRef::Segment(sid) => {
                    push_seg(&self.doc, sid, &mut pts, &mut segs);
                }
                ElementRef::Fill(fid) => {
                    if let Some(f) = self.doc.fill(fid) {
                        let ids: Vec<SegmentId> = f.segments.clone();
                        for &s in &ids {
                            push_seg(&self.doc, s, &mut pts, &mut segs);
                        }
                        if !fills.iter().any(|(id, _)| *id == fid) {
                            fills.push((fid, ids));
                        }
                    }
                }
                ElementRef::Path(pid) => {
                    if let Some(p) = self.doc.path(pid) {
                        let ids: Vec<SegmentId> = p.segments.clone();
                        for &s in &ids {
                            push_seg(&self.doc, s, &mut pts, &mut segs);
                        }
                        if !paths.iter().any(|(id, _, _, _, _)| *id == pid) {
                            paths.push((
                                pid,
                                ids,
                                p.closed,
                                p.continuity.clone(),
                                p.handles.clone(),
                            ));
                        }
                    }
                }
            }
        }
        if pts.is_empty() {
            return None;
        }
        let idx = |id: PointId| -> i64 {
            pts.iter()
                .position(|(p, _)| *p == id)
                .map(|i| i as i64)
                .unwrap_or(-1)
        };
        let seg_idx = |id: SegmentId| -> i64 {
            segs.iter()
                .position(|(p, _)| *p == id)
                .map(|i| i as i64)
                .unwrap_or(-1)
        };

        let mut s = String::from("parametric/v2");
        for (_, p) in &pts {
            s += &format!("|P:{:.3},{:.3}", p.x, p.y);
        }
        for (_, seg) in &segs {
            let k = match seg.kind {
                SegmentKind::Line => "L",
                SegmentKind::Ruler => "M",
                SegmentKind::Arc => "A",
                SegmentKind::Bezier => "B",
            };
            // Handles append after the arc fields; old readers parse the
            // first six and ignore the rest.
            s += &format!(
                "|S:{},{},{},{},{},{},{},{}",
                k,
                idx(seg.start),
                idx(seg.end),
                seg.stroke_width,
                seg.ctrl.map(|c| idx(c)).unwrap_or(-1),
                seg.center.map(|c| idx(c)).unwrap_or(-1),
                seg.handle_out.map(|h| idx(h)).unwrap_or(-1),
                seg.handle_in.map(|h| idx(h)).unwrap_or(-1),
            );
        }
        for (_, f) in &fills {
            s += "|F:";
            let list: Vec<String> = f
                .iter()
                .map(|sid| seg_idx(*sid).to_string())
                .collect();
            s += &list.join(",");
        }
        for (_, segs_ids, closed, modes, pairs) in &paths {
            let list: Vec<String> = segs_ids
                .iter()
                .map(|sid| seg_idx(*sid).to_string())
                .collect();
            let cont: Vec<String> = modes
                .iter()
                .map(|m| ContinuityMode::code(*m).to_string())
                .collect();
            // Handle pairs as flat point indices (two per anchor); old
            // readers split only three parts and ignore this one.
            let hp: Vec<String> = pairs
                .iter()
                .flat_map(|(h0, h1)| [idx(*h0).to_string(), idx(*h1).to_string()])
                .collect();
            s += &format!(
                "|H:{}:{}:{}:{}",
                if *closed { 1 } else { 0 },
                list.join(","),
                cont.join(","),
                hp.join(",")
            );
        }
        // Constraints fully inside the copied point set (rectangle H/V
        // edges, coincident bonds within the group).
        for c in &self.doc.constraints {
            let (ia, ib) = (idx(c.a), idx(c.b));
            if ia >= 0 && ib >= 0 {
                s += &format!("|C:{},{},{}", c.kind.as_str(), ia, ib);
            }
        }
        Some(s)
    }

    pub fn paste_serialized(&mut self, text: &str) -> bool {
        let Some(payload) = text.strip_prefix("parametric/v2|") else {
            return false;
        };
        let mut pts: Vec<Point2> = Vec::new();
        #[allow(clippy::type_complexity)]
        let mut seg_specs: Vec<(
            SegmentKind,
            usize,
            usize,
            f64,
            Option<usize>,
            Option<usize>,
            Option<usize>,
            Option<usize>,
        )> = Vec::new();
        let mut fill_specs: Vec<Vec<usize>> = Vec::new();
        let mut path_specs: Vec<(bool, Vec<usize>, Vec<ContinuityMode>, Vec<usize>)> = Vec::new();
        let mut con_specs: Vec<(ConstraintKind, usize, usize)> = Vec::new();

        for part in payload.split('|') {
            let mut it = part.splitn(2, ':');
            let tag = it.next().unwrap_or("");
            let rest = it.next().unwrap_or("");
            match tag {
                "P" => {
                    let mut xy = rest.split(',');
                    let (Some(x), Some(y)) = (
                        xy.next().and_then(|v| v.parse::<f64>().ok()),
                        xy.next().and_then(|v| v.parse::<f64>().ok()),
                    ) else {
                        continue;
                    };
                    pts.push(Point2::new(x, y));
                }
                "S" => {
                    let f: Vec<&str> = rest.split(',').collect();
                    if f.len() < 6 {
                        continue;
                    }
                    let kind = match f[0] {
                        "L" => SegmentKind::Line,
                        "M" => SegmentKind::Ruler,
                        "A" => SegmentKind::Arc,
                        "B" => SegmentKind::Bezier,
                        _ => continue,
                    };
                    let (Ok(si), Ok(ei)) = (f[1].parse::<usize>(), f[2].parse::<usize>()) else {
                        continue;
                    };
                    let sw = f[3].parse::<f64>().unwrap_or(0.);
                    let opt = |i: usize| {
                        f.get(i)
                            .and_then(|v| v.parse::<i64>().ok())
                            .filter(|v| *v >= 0)
                            .map(|v| v as usize)
                    };
                    let ci = opt(4);
                    let ce = opt(5);
                    let h0 = opt(6);
                    let h1 = opt(7);
                    seg_specs.push((kind, si, ei, sw, ci, ce, h0, h1));
                }
                "F" => {
                    let list: Vec<usize> = rest
                        .split(',')
                        .filter_map(|v| v.parse::<usize>().ok())
                        .collect();
                    if !list.is_empty() {
                        fill_specs.push(list);
                    }
                }
                "H" => {
                    // paths: closed:seg,seg:mode,mode:h0,h1,...
                    let mut parts = rest.splitn(4, ':');
                    let closed = parts.next().is_some_and(|v| v == "1");
                    let segs: Vec<usize> = parts
                        .next()
                        .unwrap_or("")
                        .split(',')
                        .filter_map(|v| v.parse::<usize>().ok())
                        .collect();
                    let modes: Vec<ContinuityMode> = parts
                        .next()
                        .unwrap_or("")
                        .split(',')
                        .filter_map(|v| v.parse::<u8>().ok())
                        .map(ContinuityMode::from_code)
                        .collect();
                    let handles: Vec<usize> = parts
                        .next()
                        .unwrap_or("")
                        .split(',')
                        .filter_map(|v| v.parse::<usize>().ok())
                        .collect();
                    if !segs.is_empty() {
                        path_specs.push((closed, segs, modes, handles));
                    }
                }
                "C" => {
                    let f: Vec<&str> = rest.split(',').collect();
                    if f.len() < 3 {
                        continue;
                    }
                    let kind = match f[0] {
                        "coincident" => ConstraintKind::Coincident,
                        "horizontal" => ConstraintKind::Horizontal,
                        "vertical" => ConstraintKind::Vertical,
                        "tangent" => ConstraintKind::Tangent,
                        _ => continue,
                    };
                    if let (Ok(a), Ok(b)) = (f[1].parse::<usize>(), f[2].parse::<usize>()) {
                        con_specs.push((kind, a, b));
                    }
                }
                _ => {}
            }
        }
        if pts.is_empty() {
            return false;
        }

        let layer_id = self.doc.layers[0].id;
        let ids: Vec<PointId> = pts
            .iter()
            .map(|p| self.doc.add_point(Point2::new(p.x + OFFSET, p.y + OFFSET)))
            .collect();
        let mut new_segs: Vec<SegmentId> = Vec::new();
        for (kind, si, ei, sw, ci, ce, h0, h1) in &seg_specs {
            let (Some(&sp), Some(&ep)) = (ids.get(*si), ids.get(*ei)) else {
                continue;
            };
            let sid = match kind {
                SegmentKind::Line => self.doc.add_stroked_segment(sp, ep, *sw),
                SegmentKind::Ruler => self.doc.add_segment_kind(sp, ep, SegmentKind::Ruler),
                SegmentKind::Arc => {
                    let (Some(ci), Some(ce)) = (ci.and_then(|i| ids.get(i)), ce.and_then(|i| ids.get(i)))
                    else {
                        continue;
                    };
                    self.doc.add_arc_segment(sp, *ci, ep, *ce)
                }
                SegmentKind::Bezier => {
                    let (Some(h0), Some(h1)) =
                        (h0.and_then(|i| ids.get(i)), h1.and_then(|i| ids.get(i)))
                    else {
                        continue;
                    };
                    self.doc.add_bezier_segment(sp, *h0, *h1, ep)
                }
            };
            new_segs.push(sid);
        }
        let mut new_fills: Vec<FillId> = Vec::new();
        for list in &fill_specs {
            let segs: Vec<SegmentId> = list
                .iter()
                .filter_map(|i| new_segs.get(*i).copied())
                .collect();
            if segs.len() >= 3 {
                new_fills.push(self.doc.add_fill(segs));
            }
        }
        let mut new_paths: Vec<PathId> = Vec::new();
        for (closed, list, modes, handles) in &path_specs {
            let segs: Vec<SegmentId> = list
                .iter()
                .filter_map(|i| new_segs.get(*i).copied())
                .collect();
            if segs.is_empty() {
                continue;
            }
            // Anchors implied by the pasted order; missing handle refs
            // (old payloads) collapse onto their anchor.
            let mut anchors: Vec<PointId> = Vec::new();
            for &sid in &segs {
                if let Some(s) = self.doc.segment(sid) {
                    anchors.push(s.start);
                }
            }
            if !closed
                && let Some(&last) = segs.last()
                && let Some(s) = self.doc.segment(last)
            {
                anchors.push(s.end);
            }
            let mut pairs: Vec<(PointId, PointId)> = Vec::new();
            for (i, a) in anchors.iter().enumerate() {
                let h = |k: usize| {
                    handles
                        .get(i * 2 + k)
                        .and_then(|idx| ids.get(*idx))
                        .copied()
                        .unwrap_or(*a)
                };
                pairs.push((h(0), h(1)));
            }
            new_paths.push(self.doc.add_path(segs, *closed, modes.clone(), pairs));
        }
        for (kind, a, b) in &con_specs {
            if let (Some(&pa), Some(&pb)) = (ids.get(*a), ids.get(*b)) {
                self.doc.add_constraint(*kind, pa, pb);
            }
        }

        // Push everything to the layer and select the top-level results.
        for pid in &ids {
            self.doc.push_to_layer(layer_id, ElementRef::Point(*pid));
        }
        let mut sel: Vec<ElementRef> = Vec::new();
        for sid in &new_segs {
            self.doc.push_to_layer(layer_id, ElementRef::Segment(*sid));
            sel.push(ElementRef::Segment(*sid));
        }
        for fid in &new_fills {
            self.doc.push_to_layer(layer_id, ElementRef::Fill(*fid));
            sel.push(ElementRef::Fill(*fid));
        }
        for pid in &new_paths {
            self.doc.push_to_layer(layer_id, ElementRef::Path(*pid));
            sel.push(ElementRef::Path(*pid));
        }
        if sel.is_empty() {
            sel = ids.iter().map(|pid| ElementRef::Point(*pid)).collect();
        }
        self.selection = sel;
        true
    }
}
