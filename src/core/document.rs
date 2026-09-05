use super::constraints::{Constraint, ConstraintKind, Dimension, ElementRef};
use super::geometry::{Point2, Rect};
use super::ids::{FillId, PathId, PointId, SegmentId};

// The permanent design. "What exists in the document?"
// No GPUI types here — the engine is UI-independent.
//
// The document knows exactly four kinds of things: points, segments, fills,
// and constraints/dimensions between them. There are no composite shape
// objects — a "rectangle" is simply 4 points, 4 segments sharing endpoints,
// horizontal/vertical constraints, and one fill over the loop, emitted by
// the rectangle tool. Deleting any piece leaves the rest valid.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocSettings {
    pub show_grid: bool,
    pub snap_to_grid: bool,
    pub snap_to_objects: bool,
}

impl Default for DocSettings {
    fn default() -> Self {
        Self { show_grid: true, snap_to_grid: false, snap_to_objects: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    // View/snap toggles. Saved PER design; new designs seed from the
    // app-level "last used" prefs (Registry) instead of hard-coded values.
    pub settings: DocSettings,
    pub layers: Vec<Layer>,
    points: Arena<Point2>,
    segments: Arena<Segment>,
    fills: Arena<Fill>,
    paths: Arena<Path>,
    // Geometric constraints (H/V/coincident) binding point pairs.
    pub constraints: Vec<Constraint>,
    // Dimensional measurements; a locked dimension doubles as a distance
    // constraint during edits.
    pub dimensions: Vec<Dimension>,
}

// -- entities --

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Line,
    // A measuring ruler: renders with procedural inch ticks and labels,
    // carries no constraints, no fill, no dims.
    Ruler,
    // Circular arc through start, ctrl (a point ON the arc), end. Becomes
    // a full circle when start/end share a Coincident constraint.
    Arc,
    // Cubic Bezier from start to end. handle_out (P1, outgoing from start)
    // and handle_in (P2, incoming to end) are REAL document points, like
    // arc ctrls — directly pickable, snappable and solver-visible. A
    // missing handle degenerates to its endpoint (straight line).
    Bezier,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start: PointId,
    pub end: PointId,
    pub kind: SegmentKind,
    // Screen-px stroke rendered for standalone lines; 0 = invisible
    // geometry (rectangle edges, etc.).
    pub stroke_width: f64,
    // Arc control point (a REAL point on the arc) for kind == Arc.
    // None for lines/rulers/beziers. Endpoints + ctrl define the circumcircle.
    pub ctrl: Option<PointId>,
    // Circumcenter point for arcs — a REAL document point that stays
    // centered. None for non-arcs. Enables snapping/constraints/hover.
    pub center: Option<PointId>,
    // Bezier handles (REAL points) for kind == Bezier: handle_out leaves
    // start, handle_in arrives at end. None for non-beziers.
    pub handle_out: Option<PointId>,
    pub handle_in: Option<PointId>,
}

impl Segment {
    fn line(start: PointId, end: PointId) -> Self {
        Self { start, end, kind: SegmentKind::Line, stroke_width: 0., ctrl: None, center: None, handle_out: None, handle_in: None }
    }

    fn with_kind(start: PointId, end: PointId, kind: SegmentKind) -> Self {
        Self { start, end, kind, stroke_width: 0., ctrl: None, center: None, handle_out: None, handle_in: None }
    }
}

// Continuity of one path anchor (docs/pen-tool.md section 13). Stored on
// the path, not the point: one shared point can be sharp in one path and
// smooth in another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuityMode {
    Corner,
    Smooth,
    Symmetric,
    Free,
}

impl ContinuityMode {
    pub fn code(self) -> u8 {
        match self {
            ContinuityMode::Corner => 0,
            ContinuityMode::Smooth => 1,
            ContinuityMode::Symmetric => 2,
            ContinuityMode::Free => 3,
        }
    }

    pub fn from_code(code: u8) -> ContinuityMode {
        match code {
            1 => ContinuityMode::Smooth,
            2 => ContinuityMode::Symmetric,
            3 => ContinuityMode::Free,
            _ => ContinuityMode::Corner,
        }
    }
}

// An ordered chain of segments sharing endpoints: the pen tool's path
// object. Anchors are the joints (each segment's start, plus the final end
// when open); `continuity` and `handles` run parallel to those anchors.
// Each anchor owns its handle pair (outgoing, incoming) — shared anchors
// keep independent tangents per path. Closed paths loop back onto their
// first anchor (shared ID, genuinely closed).
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub segments: Vec<SegmentId>,
    pub closed: bool,
    pub continuity: Vec<ContinuityMode>,
    pub handles: Vec<(PointId, PointId)>,
}

// A fill covers an ordered, closed loop of segments. Each segment must
// chain onto the previous one's endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub segments: Vec<SegmentId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub elements: Vec<ElementRef>,
}

impl Layer {
    pub fn retain_element(&mut self, el: ElementRef) {
        self.elements.retain(|&e| e != el);
    }
}

// -- arena --

/// Generational slot storage: stable ids survive deletes without dangling
/// references. A stale id resolves to None instead of wrong data.
#[derive(Clone, Debug, PartialEq)]
struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

impl<T> Arena<T> {
    /// Returns (idx, generation) for the freshly stored value.
    fn insert(&mut self, value: T) -> (u32, u32) {
        match self.free.pop() {
            Some(idx) => {
                let slot = &mut self.slots[idx as usize];
                slot.value = Some(value);
                (idx, slot.generation)
            }
            None => {
                self.slots.push(Slot { generation: 0, value: Some(value) });
                ((self.slots.len() - 1) as u32, 0)
            }
        }
    }

    fn remove(&mut self, idx: u32) -> Option<T> {
        let slot = self.slots.get_mut(idx as usize)?;
        let value = slot.value.take()?;
        slot.generation += 1;
        self.free.push(idx);
        Some(value)
    }

    fn generation(&self, idx: u32) -> Option<u32> {
        self.slots.get(idx as usize).map(|s| s.generation)
    }

    fn get(&self, id: (u32, u32)) -> Option<&T> {
        let slot = self.slots.get(id.0 as usize)?;
        if slot.generation != id.1 {
            return None;
        }
        slot.value.as_ref()
    }

    fn get_mut(&mut self, id: (u32, u32)) -> Option<&mut T> {
        let slot = self.slots.get_mut(id.0 as usize)?;
        if slot.generation != id.1 {
            return None;
        }
        slot.value.as_mut()
    }

    fn iter(&self) -> impl Iterator<Item = (u32, u32, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.value.as_ref().map(|v| (i as u32, s.generation, v)))
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Places a value at an exact slot (raw restore path).
    fn set_at(&mut self, idx: u32, value: T) {
        if let Some(slot) = self.slots.get_mut(idx as usize) {
            slot.value = Some(value);
        }
    }
}

// -- Document --

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    // -- points --

    pub fn add_point(&mut self, pos: Point2) -> PointId {
        let (idx, generation) = self.points.insert(pos.clamped());
        PointId { idx, generation: generation }
    }

    pub fn point(&self, id: PointId) -> Option<Point2> {
        self.points.get((id.idx, id.generation)).copied()
    }

    pub fn move_point(&mut self, id: PointId, to: Point2) {
        if let Some(p) = self.points.get_mut((id.idx, id.generation)) {
            *p = to.clamped();
        }
    }

    /// Translates several points by delta in one pass.
    pub fn move_points(&mut self, ids: &[PointId], delta: Point2) {
        for &id in ids {
            if let Some(p) = self.points.get_mut((id.idx, id.generation)) {
                *p = Point2::new(p.x + delta.x, p.y + delta.y).clamped();
            }
        }
    }

    /// Removes a point plus everything that depends on it: touching
    /// segments, fills through those segments, constraints, dimensions.
    pub fn remove_point(&mut self, id: PointId) -> bool {
        let dead: Vec<SegmentId> = self
            .segments
            .iter()
            .filter(|(_, _, s)| {
                s.start == id || s.end == id || s.ctrl == Some(id) || s.center == Some(id)
                    || s.handle_out == Some(id) || s.handle_in == Some(id)
            })
            .map(|(idx, generation, _)| SegmentId { idx, generation: generation })
            .collect();
        for sid in dead {
            self.remove_segment(sid);
        }
        self.constraints.retain(|c| c.a != id && c.b != id);
        // Dimensions touching the deleted point die too; point references
        // inside line/angle targets invalidate those dimensions as well.
        self.dimensions.retain(|d| match d.target {
            super::constraints::DimTarget::Points { a, b, .. } => a != id && b != id,
            super::constraints::DimTarget::PointLine { p, .. } => p != id,
            _ => true,
        });
        self.detach_from_layers(ElementRef::Point(id));
        self.points.remove(id.idx).is_some()
    }

    // -- segments --

    pub fn add_segment(&mut self, start: PointId, end: PointId) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment::line(start, end));
        SegmentId { idx, generation }
    }

    /// Adds a segment with an explicit kind (ruler, future arcs).
    pub fn add_segment_kind(&mut self, start: PointId, end: PointId, kind: SegmentKind) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment::with_kind(start, end, kind));
        SegmentId { idx, generation }
    }

    /// Adds a standalone stroked line (the line tool's output).
    pub fn add_stroked_segment(&mut self, start: PointId, end: PointId, stroke_width: f64) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment {
            start,
            end,
            kind: SegmentKind::Line,
            stroke_width,
            ctrl: None,
            center: None,
            handle_out: None,
            handle_in: None,
        });
        SegmentId { idx, generation }
    }

    /// Adds a circular arc through start -> ctrl -> end, with a real
    /// center point (kept in sync by the editor).
    pub fn add_arc_segment(
        &mut self,
        start: PointId,
        ctrl: PointId,
        end: PointId,
        center: PointId,
    ) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment {
            start,
            end,
            kind: SegmentKind::Arc,
            stroke_width: 0.,
            ctrl: Some(ctrl),
            center: Some(center),
            handle_out: None,
            handle_in: None,
        });
        SegmentId { idx, generation }
    }

    /// Adds a cubic Bezier from start to end with real handle points:
    /// handle_out (P1) leaves start, handle_in (P2) arrives at end.
    pub fn add_bezier_segment(
        &mut self,
        start: PointId,
        handle_out: PointId,
        handle_in: PointId,
        end: PointId,
    ) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment {
            start,
            end,
            kind: SegmentKind::Bezier,
            stroke_width: 0.,
            ctrl: None,
            center: None,
            handle_out: Some(handle_out),
            handle_in: Some(handle_in),
        });
        SegmentId { idx, generation }
    }

    /// Resolved cubic control points (P0, P1, P2, P3). A missing handle
    /// degenerates to its endpoint, so beziers stay renderable even when
    /// partially constructed or partially restored.
    pub fn bezier_geom(&self, id: SegmentId) -> Option<(Point2, Point2, Point2, Point2)> {
        let s = self.segment(id)?;
        if s.kind != SegmentKind::Bezier {
            return None;
        }
        let p0 = self.point(s.start)?;
        let p3 = self.point(s.end)?;
        let p1 = s.handle_out.and_then(|h| self.point(h)).unwrap_or(p0);
        let p2 = s.handle_in.and_then(|h| self.point(h)).unwrap_or(p3);
        Some((p0, p1, p2, p3))
    }

    pub fn segment(&self, id: SegmentId) -> Option<Segment> {
        self.segments.get((id.idx, id.generation)).copied()
    }

    /// Resolved endpoint positions of a segment.
    pub fn segment_geom(&self, id: SegmentId) -> Option<(Point2, Point2)> {
        let s = self.segment(id)?;
        Some((self.point(s.start)?, self.point(s.end)?))
    }

    /// Removes a segment: fill loops through it die, paths splice it out
    /// (a path left with no segments dies with it).
    pub fn remove_segment(&mut self, id: SegmentId) -> bool {
        for fid in self.fills_referencing(id) {
            self.remove_fill(fid);
        }
        let emptied: Vec<PathId> = self
            .paths
            .iter()
            .filter(|(_, _, p)| p.segments.contains(&id))
            .map(|(idx, generation, _)| PathId { idx, generation })
            .collect();
        for pid in emptied {
            let drop_path = match self.paths.get((pid.idx, pid.generation)) {
                Some(p) => {
                    let mut segs = p.segments.clone();
                    segs.retain(|&s| s != id);
                    if segs.is_empty() {
                        true
                    } else {
                        if let Some(path) = self.paths.get_mut((pid.idx, pid.generation)) {
                            path.segments = segs;
                            // Continuity/handles run parallel to the anchors;
                            // a splice re-indexes joints, so reset rather
                            // than keep values on the wrong anchors.
                            let want = path.segments.len() + !path.closed as usize;
                            path.continuity = vec![ContinuityMode::Corner; want];
                            path.handles.clear();
                        }
                        // Collapsed pairs keep the parallel invariant (anchor
                        // count may have changed; read fresh, then fill).
                        let anchors = self.path_anchors(pid).unwrap_or_default();
                        if let Some(path) = self.paths.get_mut((pid.idx, pid.generation)) {
                            path.handles = anchors.iter().map(|&a| (a, a)).collect();
                        }
                        false
                    }
                }
                None => false,
            };
            if drop_path {
                self.remove_path(pid);
            }
        }
        self.detach_from_layers(ElementRef::Segment(id));
        self.segments.remove(id.idx).is_some()
    }

    fn fills_referencing(&self, sid: SegmentId) -> Vec<FillId> {
        self.fills
            .iter()
            .filter(|(_, _, f)| f.segments.contains(&sid))
            .map(|(idx, generation, _)| FillId { idx, generation: generation })
            .collect()
    }

    /// All segments in the document (id + payload).
    pub fn all_segments(&self) -> impl Iterator<Item = (SegmentId, Segment)> + '_ {
        self.segments.iter().map(|(idx, generation, s)| (SegmentId { idx, generation: generation }, *s))
    }

    /// All points in the document (id + position).
    pub fn all_points(&self) -> impl Iterator<Item = (PointId, Point2)> + '_ {
        self.points.iter().map(|(idx, generation, p)| (PointId { idx, generation: generation }, *p))
    }

    /// All fills in the document (id + payload).
    pub fn all_fills(&self) -> impl Iterator<Item = (FillId, &Fill)> + '_ {
        self.fills.iter().map(|(idx, generation, f)| (FillId { idx, generation: generation }, f))
    }

    // -- fills --

    pub fn add_fill(&mut self, segments: Vec<SegmentId>) -> FillId {
        let (idx, generation) = self.fills.insert(Fill { segments });
        FillId { idx, generation: generation }
    }

    pub fn fill(&self, id: FillId) -> Option<&Fill> {
        self.fills.get((id.idx, id.generation))
    }

    pub fn remove_fill(&mut self, id: FillId) -> bool {
        self.detach_from_layers(ElementRef::Fill(id));
        self.fills.remove(id.idx).is_some()
    }

    // -- paths --

    /// Adds a path over ordered segments. `continuity` and `handles` run
    /// parallel to the anchors (each segment's start, plus the final end
    /// when open); short vecs pad (Corner / collapsed at the anchor), long
    /// ones truncate.
    pub fn add_path(
        &mut self,
        segments: Vec<SegmentId>,
        closed: bool,
        continuity: Vec<ContinuityMode>,
        handles: Vec<(PointId, PointId)>,
    ) -> PathId {
        let anchors: Vec<PointId> = {
            let mut out = Vec::with_capacity(segments.len() + 1);
            for &sid in &segments {
                if let Some(seg) = self.segment(sid) {
                    out.push(seg.start);
                }
            }
            if !closed
                && let Some(&last) = segments.last()
                && let Some(seg) = self.segment(last)
            {
                out.push(seg.end);
            }
            out
        };
        let want = anchors.len();
        let mut modes = continuity;
        modes.truncate(want);
        while modes.len() < want {
            modes.push(ContinuityMode::Corner);
        }
        let mut pairs = handles;
        pairs.truncate(want);
        if let Some(&fallback) = anchors.first() {
            while pairs.len() < want {
                pairs.push((fallback, fallback));
            }
        }
        let (idx, generation) = self.paths.insert(Path {
            segments,
            closed,
            continuity: modes,
            handles: pairs,
        });
        PathId { idx, generation }
    }

    /// Every anchor's handle pair, parallel to `path_anchors`.
    pub fn path_handle_pairs(&self, id: PathId) -> Option<Vec<(PointId, PointId)>> {
        self.path(id).map(|p| p.handles.clone())
    }

    pub fn path(&self, id: PathId) -> Option<&Path> {
        self.paths.get((id.idx, id.generation))
    }

    pub fn path_mut(&mut self, id: PathId) -> Option<&mut Path> {
        self.paths.get_mut((id.idx, id.generation))
    }

    pub fn remove_path(&mut self, id: PathId) -> bool {
        self.detach_from_layers(ElementRef::Path(id));
        self.paths.remove(id.idx).is_some()
    }

    /// All paths in the document (id + payload).
    pub fn all_paths(&self) -> impl Iterator<Item = (PathId, &Path)> + '_ {
        self.paths.iter().map(|(idx, generation, p)| (PathId { idx, generation }, p))
    }

    /// Ordered anchor points of a path: each segment's start, plus the
    /// final end when the path is open.
    pub fn path_anchors(&self, id: PathId) -> Option<Vec<PointId>> {
        let path = self.path(id)?;
        let mut out = Vec::with_capacity(path.segments.len() + 1);
        for &sid in &path.segments {
            out.push(self.segment(sid)?.start);
        }
        if !path.closed
            && let Some(&last) = path.segments.last()
        {
            out.push(self.segment(last)?.end);
        }
        Some(out)
    }

    /// Every point a path owns: anchors plus bezier handles.
    pub fn path_points(&self, id: PathId) -> Vec<PointId> {
        let mut out = self.path_anchors(id).unwrap_or_default();
        if let Some(path) = self.path(id) {
            for &sid in &path.segments {
                if let Some(seg) = self.segment(sid) {
                    for h in [seg.handle_out, seg.handle_in].into_iter().flatten() {
                        if !out.contains(&h) {
                            out.push(h);
                        }
                    }
                }
            }
        }
        out
    }

    // -- layers --

    pub(crate) fn detach_from_layers(&mut self, el: ElementRef) {
        for layer in &mut self.layers {
            layer.retain_element(el);
        }
    }

    pub fn push_to_layer(&mut self, layer_id: u64, el: ElementRef) {
        if let Some(layer) = self.layer_mut(layer_id) {
            layer.elements.push(el);
        }
    }

    // -- derived geometry --

    /// Bounding rect of a set of points.
    pub fn bounds_of_points<'a>(&self, ids: impl IntoIterator<Item = &'a PointId>) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        for &id in ids {
            let p = self.point(id)?;
            let r = Rect::from_points(p, p);
            acc = Some(match acc {
                Some(a) => a.union(&r),
                None => r,
            });
        }
        acc
    }

    /// Bounding rect of a closed fill loop.
    pub fn fill_bounds(&self, id: FillId) -> Option<Rect> {
        let f = self.fill(id)?;
        let pts: Vec<PointId> = f
            .segments
            .iter()
            .filter_map(|&sid| self.segment(sid))
            .flat_map(|s| [s.start, s.end])
            .collect();
        self.bounds_of_points(&pts)
    }

    /// All points referenced by an element (segment endpoints / loop corners
    /// / path anchors + handles).
    pub fn element_points(&self, el: ElementRef) -> Vec<PointId> {
        match el {
            ElementRef::Point(p) => vec![p],
            ElementRef::Segment(s) => match self.segment(s) {
                Some(seg) => {
                    let mut v = vec![seg.start, seg.end];
                    if let Some(c) = seg.ctrl {
                        v.push(c);
                    }
                    if let Some(c) = seg.center {
                        v.push(c);
                    }
                    if let Some(h) = seg.handle_out {
                        v.push(h);
                    }
                    if let Some(h) = seg.handle_in {
                        v.push(h);
                    }
                    v
                }
                None => Vec::new(),
            },
            ElementRef::Fill(f) => match self.fill(f) {
                Some(fill) => fill
                    .segments
                    .iter()
                    .filter_map(|&sid| self.segment(sid))
                    .flat_map(|s| [s.start, s.end])
                    .collect(),
                None => Vec::new(),
            },
            ElementRef::Path(p) => self.path_points(p),
        }
    }

    /// Deduplicated points of a set of elements.
    pub fn selection_points(&self, els: &[ElementRef]) -> Vec<PointId> {
        let mut out = Vec::new();
        for el in els {
            for p in self.element_points(*el) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        out
    }

    // -- constraints / dimensions --

    pub fn add_constraint(&mut self, kind: ConstraintKind, a: PointId, b: PointId) {
        let c = Constraint { kind, a, b, tangent_segments: None, point_on_segment: None };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    pub fn add_tangent_constraint(
        &mut self,
        line: SegmentId,
        arc: SegmentId,
        point: PointId,
    ) {
        let c = Constraint {
            kind: ConstraintKind::Tangent,
            a: point,
            b: point,
            tangent_segments: Some((line, arc)),
            point_on_segment: None,
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    pub fn add_parallel_constraint(&mut self, first: SegmentId, second: SegmentId) {
        let (Some(a), Some(b)) = (self.segment(first), self.segment(second)) else { return };
        let c = Constraint {
            kind: ConstraintKind::Parallel,
            a: a.start,
            b: b.start,
            // Reuse the owning-segment pair already carried by tangent
            // constraints; the kind determines how the pair is interpreted.
            tangent_segments: Some((first, second)),
            point_on_segment: None,
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    /// Constrains `point` to a new/selected point that lies on `segment`.
    /// Keeping the edge point as a real point makes the relationship
    /// persistent and lets the solver preserve it when either object moves.
    pub fn add_point_on_segment_constraint(
        &mut self,
        point: PointId,
        edge_point: PointId,
        segment: SegmentId,
    ) {
        let c = Constraint {
            kind: ConstraintKind::Coincident,
            a: point,
            b: edge_point,
            tangent_segments: None,
            point_on_segment: Some(segment),
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    /// Fuses `drop` into `keep`: every reference to `drop` (segments,
    /// constraints, dimensions, layer listings) is rewritten to `keep`,
    /// then `drop` is deleted. Degenerate self-referential constraints and
    /// dimensions are dropped.
    pub fn merge_point(&mut self, keep: PointId, drop: PointId) {
        if keep == drop || self.point(drop).is_none() {
            return;
        }
        for slot in &mut self.segments.slots {
            if let Some(s) = &mut slot.value {
                if s.start == drop {
                    s.start = keep;
                }
                if s.end == drop {
                    s.end = keep;
                }
                if s.ctrl == Some(drop) {
                    s.ctrl = Some(keep);
                }
                if s.center == Some(drop) {
                    s.center = Some(keep);
                }
                if s.handle_out == Some(drop) {
                    s.handle_out = Some(keep);
                }
                if s.handle_in == Some(drop) {
                    s.handle_in = Some(keep);
                }
            }
        }
        for c in &mut self.constraints {
            if c.a == drop {
                c.a = keep;
            }
            if c.b == drop {
                c.b = keep;
            }
        }
        for d in &mut self.dimensions {
            match &mut d.target {
                super::constraints::DimTarget::Points { a, b, .. } => {
                    if *a == drop {
                        *a = keep;
                    }
                    if *b == drop {
                        *b = keep;
                    }
                }
                super::constraints::DimTarget::PointLine { p, .. } => {
                    if *p == drop {
                        *p = keep;
                    }
                }
                _ => {}
            }
        }
        self.constraints.retain(|c| c.a != c.b);
        self.dimensions.retain(|d| match d.target {
            super::constraints::DimTarget::Points { a, b, .. } => a != b,
            _ => true,
        });
        self.detach_from_layers(ElementRef::Point(drop));
        self.points.remove(drop.idx);
    }

    pub fn add_dimension(&mut self, dim: Dimension) {
        self.dimensions.push(dim);
    }

    /// Removes dimensions whose referenced geometry no longer exists, then
    /// points that nothing references anymore (no segment endpoints, no
    /// constraint, no dimension). Called after deletions so a deleted shape
    /// takes its dimensions and corner points with it.
    pub fn sweep_orphans(&mut self) {
        use super::constraints::DimTarget;
        // Drop dims whose referenced geometry vanished.
        let dims = self.dimensions.clone();
        self.dimensions = dims
            .into_iter()
            .filter(|d| match &d.target {
                DimTarget::Points { a, b, .. } => {
                    self.point(*a).is_some() && self.point(*b).is_some()
                }
                DimTarget::PointLine { p, line } => {
                    self.point(*p).is_some() && self.segment(*line).is_some()
                }
                DimTarget::Lines { a, b } | DimTarget::Angle { a, b } => {
                    self.segment(*a).is_some() && self.segment(*b).is_some()
                }
                DimTarget::Radius { seg } => self.segment(*seg).is_some(),
            })
            .collect();
        let dims = self.dimensions.clone();
        // A point survives only while something references it: segments,
        // path anchor pairs, constraints, or dimensions. Note a dimension
        // counts as a reference: dims pin their geometry.
        let referenced = |id: PointId| -> bool {
            self.all_segments().any(|(_, s)| {
                s.start == id || s.end == id || s.ctrl == Some(id) || s.center == Some(id)
                    || s.handle_out == Some(id) || s.handle_in == Some(id)
            }) || self.all_paths().any(|(_, p)| {
                p.handles.iter().any(|&(h0, h1)| h0 == id || h1 == id)
            }) || self.constraints.iter().any(|c| c.a == id || c.b == id)
                || dims.iter().any(|d| match &d.target {
                    DimTarget::Points { a, b, .. } => *a == id || *b == id,
                    DimTarget::PointLine { p, .. } => *p == id,
                    _ => false,
                })
        };
        let dead: Vec<PointId> = self
            .all_points()
            .filter(|(id, _)| !referenced(*id))
            .map(|(id, _)| id)
            .collect();
        for id in dead {
            self.points.remove(id.idx);
            self.detach_from_layers(super::constraints::ElementRef::Point(id));
        }
    }

    // -- raw inserts (persistence round-trips ids exactly) --

    pub fn insert_point_with_id(&mut self, id: PointId, pos: Point2) {
        Self::reserve(&mut self.points, id.idx, id.generation);
        self.points.set_at(id.idx, pos.clamped());
    }

    pub fn insert_segment_with_id(
        &mut self,
        id: SegmentId,
        start: PointId,
        end: PointId,
        kind: SegmentKind,
        stroke_width: f64,
        ctrl: Option<PointId>,
        center: Option<PointId>,
        handle_out: Option<PointId>,
        handle_in: Option<PointId>,
    ) {
        Self::reserve(&mut self.segments, id.idx, id.generation);
        self.segments.set_at(
            id.idx,
            Segment { start, end, kind, stroke_width, ctrl, center, handle_out, handle_in },
        );
    }

    pub fn insert_fill_with_id(&mut self, id: FillId, segments: Vec<SegmentId>) {
        Self::reserve(&mut self.fills, id.idx, id.generation);
        self.fills.set_at(id.idx, Fill { segments });
    }

    pub fn insert_path_with_id(
        &mut self,
        id: PathId,
        segments: Vec<SegmentId>,
        closed: bool,
        continuity: Vec<ContinuityMode>,
        handles: Vec<(PointId, PointId)>,
    ) {
        Self::reserve(&mut self.paths, id.idx, id.generation);
        self.paths.set_at(id.idx, Path { segments, closed, continuity, handles });
    }

    fn reserve<T>(arena: &mut Arena<T>, idx: u32, generation: u32) {
        while arena.slots.len() <= idx as usize {
            arena.slots.push(Slot { generation: 0, value: None });
        }
        arena.slots[idx as usize].generation = generation;
    }
}
