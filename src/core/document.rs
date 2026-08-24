use super::constraints::{Constraint, ConstraintKind, Dimension, ElementRef};
use super::geometry::{Point2, Rect};
use super::ids::{FillId, PointId, SegmentId};

// The permanent design. "What exists in the document?"
// No GPUI types here — the engine is UI-independent.
//
// The document knows exactly four kinds of things: points, segments, fills,
// and constraints/dimensions between them. There are no composite shape
// objects — a "rectangle" is simply 4 points, 4 segments sharing endpoints,
// horizontal/vertical constraints, and one fill over the loop, emitted by
// the rectangle tool. Deleting any piece leaves the rest valid.

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub layers: Vec<Layer>,
    points: Arena<Point2>,
    segments: Arena<Segment>,
    fills: Arena<Fill>,
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
    // Arc support lands later; the enum is reserved now so stored data and
    // all match sites already expect it.
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start: PointId,
    pub end: PointId,
    pub kind: SegmentKind,
}

impl Segment {
    fn line(start: PointId, end: PointId) -> Self {
        Self { start, end, kind: SegmentKind::Line }
    }
}

// A fill covers an ordered, closed loop of segments. Each segment must
// chain onto the previous one's endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub segments: Vec<SegmentId>,
}

#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new() }
    }
}

#[derive(Clone, Debug)]
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
            .filter(|(_, _, s)| s.start == id || s.end == id)
            .map(|(idx, generation, _)| SegmentId { idx, generation: generation })
            .collect();
        for sid in dead {
            self.remove_segment(sid);
        }
        self.constraints.retain(|c| c.a != id && c.b != id);
        self.dimensions.retain(|d| d.a != id && d.b != id);
        self.detach_from_layers(ElementRef::Point(id));
        self.points.remove(id.idx).is_some()
    }

    // -- segments --

    pub fn add_segment(&mut self, start: PointId, end: PointId) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment::line(start, end));
        SegmentId { idx, generation: generation }
    }

    pub fn segment(&self, id: SegmentId) -> Option<Segment> {
        self.segments.get((id.idx, id.generation)).copied()
    }

    /// Resolved endpoint positions of a segment.
    pub fn segment_geom(&self, id: SegmentId) -> Option<(Point2, Point2)> {
        let s = self.segment(id)?;
        Some((self.point(s.start)?, self.point(s.end)?))
    }

    /// Removes a segment and any fill loops passing through it.
    pub fn remove_segment(&mut self, id: SegmentId) -> bool {
        for fid in self.fills_referencing(id) {
            self.remove_fill(fid);
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

    // -- layers --

    fn detach_from_layers(&mut self, el: ElementRef) {
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

    /// All points referenced by an element (segment endpoints / loop corners).
    pub fn element_points(&self, el: ElementRef) -> Vec<PointId> {
        match el {
            ElementRef::Point(p) => vec![p],
            ElementRef::Segment(s) => match self.segment(s) {
                Some(seg) => vec![seg.start, seg.end],
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
        let c = Constraint { kind, a, b };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    pub fn add_dimension(&mut self, dim: Dimension) {
        self.dimensions.push(dim);
    }

    // -- raw inserts (persistence round-trips ids exactly) --

    pub fn insert_point_with_id(&mut self, id: PointId, pos: Point2) {
        Self::reserve(&mut self.points, id.idx, id.generation);
        self.points.set_at(id.idx, pos.clamped());
    }

    pub fn insert_segment_with_id(&mut self, id: SegmentId, start: PointId, end: PointId, kind: SegmentKind) {
        Self::reserve(&mut self.segments, id.idx, id.generation);
        self.segments.set_at(id.idx, Segment { start, end, kind });
    }

    pub fn insert_fill_with_id(&mut self, id: FillId, segments: Vec<SegmentId>) {
        Self::reserve(&mut self.fills, id.idx, id.generation);
        self.fills.set_at(id.idx, Fill { segments });
    }

    fn reserve<T>(arena: &mut Arena<T>, idx: u32, generation: u32) {
        while arena.slots.len() <= idx as usize {
            arena.slots.push(Slot { generation: 0, value: None });
        }
        arena.slots[idx as usize].generation = generation;
    }
}
