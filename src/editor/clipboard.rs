use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::core::document::{Document, Segment, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::{FillId, PointId, SegmentId};
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

        for &el in &self.selection {
            match el {
                ElementRef::Point(pid) => push_pt(&self.doc, pid, &mut pts),
                ElementRef::Segment(sid) => {
                    if let Some(seg) = self.doc.segment(sid) {
                        push_pt(&self.doc, seg.start, &mut pts);
                        push_pt(&self.doc, seg.end, &mut pts);
                        if let Some(c) = seg.ctrl {
                            push_pt(&self.doc, c, &mut pts);
                        }
                        if let Some(c) = seg.center {
                            push_pt(&self.doc, c, &mut pts);
                        }
                        if !segs.iter().any(|(id, _)| *id == sid) {
                            segs.push((sid, seg));
                        }
                    }
                }
                ElementRef::Fill(fid) => {
                    if let Some(f) = self.doc.fill(fid) {
                        let ids: Vec<SegmentId> = f.segments.clone();
                        for &s in &ids {
                            if let Some(seg) = self.doc.segment(s) {
                                push_pt(&self.doc, seg.start, &mut pts);
                                push_pt(&self.doc, seg.end, &mut pts);
                                if let Some(c) = seg.ctrl {
                                    push_pt(&self.doc, c, &mut pts);
                                }
                                if let Some(c) = seg.center {
                                    push_pt(&self.doc, c, &mut pts);
                                }
                                if !segs.iter().any(|(id, _)| *id == s) {
                                    segs.push((s, seg));
                                }
                            }
                        }
                        if !fills.iter().any(|(id, _)| *id == fid) {
                            fills.push((fid, ids));
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
            };
            s += &format!(
                "|S:{},{},{},{},{},{}",
                k,
                idx(seg.start),
                idx(seg.end),
                seg.stroke_width,
                seg.ctrl.map(|c| idx(c)).unwrap_or(-1),
                seg.center.map(|c| idx(c)).unwrap_or(-1),
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
        let mut seg_specs: Vec<(SegmentKind, usize, usize, f64, Option<usize>, Option<usize>)> =
            Vec::new();
        let mut fill_specs: Vec<Vec<usize>> = Vec::new();
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
                        _ => continue,
                    };
                    let (Ok(si), Ok(ei)) = (f[1].parse::<usize>(), f[2].parse::<usize>()) else {
                        continue;
                    };
                    let sw = f[3].parse::<f64>().unwrap_or(0.);
                    let ci = f[4].parse::<i64>().ok().filter(|v| *v >= 0).map(|v| v as usize);
                    let ce = f[5].parse::<i64>().ok().filter(|v| *v >= 0).map(|v| v as usize);                    seg_specs.push((kind, si, ei, sw, ci, ce));
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
                "C" => {
                    let f: Vec<&str> = rest.split(',').collect();
                    if f.len() < 3 {
                        continue;
                    }
                    let kind = match f[0] {
                        "coincident" => ConstraintKind::Coincident,
                        "horizontal" => ConstraintKind::Horizontal,
                        "vertical" => ConstraintKind::Vertical,
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
        for (kind, si, ei, sw, ci, ce) in &seg_specs {
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
        if sel.is_empty() {
            sel = ids.iter().map(|pid| ElementRef::Point(*pid)).collect();
        }
        self.selection = sel;
        true
    }
}
