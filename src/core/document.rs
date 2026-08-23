use super::geometry::{Point2, Rect};
use super::ids::{PointId, ShapeId};

// The permanent design. "What exists in the document?"
// No GPUI types here â€” the engine is UI-independent.
//
// Storage model: flat arenas (Vec slots + generation counters) instead of
// nested ownership. Points are first-class entities so constraints can bind
// to them; shapes reference points rather than owning coordinates.

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub layers: Vec<Layer>,
    points: PointArena,
    shapes: ShapeArena,
    // Dimension constraints: locked width/height per shape.
    pub dim_constraints: Vec<DimConstraint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DimConstraint {
    pub shape: ShapeId,
    pub width: bool,
    pub value: f64,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    // -- points --

    pub fn add_point(&mut self, pos: Point2) -> PointId {
        self.points.insert(pos.clamped())
    }

    pub fn point(self, id: PointId) -> Option<Point2> {
        self.points.get(id)
    }

    pub fn move_point(&mut self, id: PointId, to: Point2) {
        self.points.set(id, to.clamped());
    }

    // -- shapes --

    pub fn add_shape(
        &mut self,
        layer_id: u64,
        kind: ShapeKind,
        corners: [PointId; 2],
    ) -> ShapeId {
        let shape = Shape { layer: layer_id, kind, corners };
        let id = self.shapes.insert(shape);
        if let Some(layer) = self.layer_mut(layer_id) {
            layer.shape_ids.push(id);
        }
        id
    }

    pub fn shape_bounds(&self, id: ShapeId) -> Option<Rect> {
        let s = self.shapes.get(id)?;
        let a = self.points.get(s.corners[0])?;
        let b = self.points.get(s.corners[1])?;
        Some(Rect::from_points(a, b))
    }

    pub fn shape_kind(&self, id: ShapeId) -> Option<ShapeKind> {
        self.shapes.get(id).map(|s| s.kind)
    }

    // Moves both corner points so the shape's bounds become `new_bounds`.
    pub fn set_shape_corners(&mut self, id: ShapeId, new_bounds: Rect) -> bool {
        let corners = match self.shapes.get(id) {
            Some(s) => s.corners,
            None => return false,
        };
        let a = Point2::new(
            new_bounds.origin.x,
            new_bounds.origin.y,
        );
        let b = Point2::new(
            new_bounds.origin.x + new_bounds.size.w,
            new_bounds.origin.y + new_bounds.size.h,
        );
        self.points.set(corners[0], a.clamped());
        self.points.set(corners[1], b.clamped());
        true
    }

    // Translates both corner points of a shape by delta.
    pub fn translate_shape(&mut self, id: ShapeId, delta: Point2) -> bool {
        let corners = match self.shapes.get(id) {
            Some(s) => s.corners,
            None => return false,
        };
        for pid in corners {
            if let Some(p) = self.points.get(pid) {
                self.points
                    .set(pid, Point2::new(p.x + delta.x, p.y + delta.y));
            }
        }
        true
    }

    pub fn dim_lock(&mut self, shape: ShapeId, width: bool, value: f64) {
        if let Some(c) = self
            .dim_constraints
            .iter_mut()
            .find(|c| c.shape == shape && c.width == width)
        {
            c.value = value;
        } else {
            self.dim_constraints.push(DimConstraint { shape, width, value });
        }
    }

    pub fn dim_unlock(&mut self, shape: ShapeId, width: bool) {
        self.dim_constraints
            .retain(|c| !(c.shape == shape && c.width == width));
    }

    pub fn dim_value(&self, shape: ShapeId, width: bool) -> Option<f64> {
        self.dim_constraints
            .iter()
            .find(|c| c.shape == shape && c.width == width)
            .map(|c| c.value)
    }

    // Removes a shape entirely: layer entry, arena slot (freed with a
    // generation bump), and any dimension constraints referencing it.
    pub fn remove_shape(&mut self, id: ShapeId) -> bool {
        let Some(slot) = self.shapes.data.get_mut(id.idx as usize) else {
            return false;
        };
        if slot.generation != id.generation {
            return false;
        }
        let Some(shape) = slot.shape.take() else {
            return false;
        };
        slot.generation += 1;
        self.shapes.free.push(id.idx);
        self.dim_constraints.retain(|c| c.shape != id);
        let layer_id = shape.layer;
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == layer_id) {
            layer.shape_ids.retain(|&s| s != id);
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
}

impl ShapeKind {
    // Stable string form used by persistence to store `kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            ShapeKind::Rectangle => "rectangle",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rectangle" => Some(ShapeKind::Rectangle),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub layer: u64,
    pub kind: ShapeKind,
    // Opposite corners in document space; derived bounds replace stored x/y/w/h.
    pub corners: [PointId; 2],
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub shape_ids: Vec<ShapeId>,
}

// -- arenas --

#[derive(Clone, Debug, Default)]
struct PointArena {
    pos: Vec<Point2>,
    generation: Vec<u32>,
    free: Vec<u32>,
}

impl PointArena {
    fn insert(&mut self, p: Point2) -> PointId {
        match self.free.pop() {
            Some(idx) => {
                self.pos[idx as usize] = p;
                PointId { idx, generation: self.generation[idx as usize] }
            }
            None => {
                self.pos.push(p);
                self.generation.push(0);
                PointId { idx: (self.pos.len() - 1) as u32, generation: 0 }
            }
        }
    }

    fn get(&self, id: PointId) -> Option<Point2> {
        self.pos
            .get(id.idx as usize)
            .filter(|_| self.generation[id.idx as usize] == id.generation)
            .copied()
    }

    fn set(&mut self, id: PointId, p: Point2) {
        if let Some(slot) = self.pos.get_mut(id.idx as usize) {
            if self.generation[id.idx as usize] == id.generation {
                *slot = p;
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ShapeArena {
    data: Vec<ShapeSlot>,
    free: Vec<u32>,
}

#[derive(Clone, Debug)]
struct ShapeSlot {
    generation: u32,
    shape: Option<Shape>,
}

impl ShapeArena {
    fn insert(&mut self, shape: Shape) -> ShapeId {
        if let Some(idx) = self.free.pop() {
            let slot = &mut self.data[idx as usize];
            slot.shape = Some(shape);
            return ShapeId { idx, generation: slot.generation };
        }
        self.data.push(ShapeSlot { generation: 0, shape: Some(shape) });
        ShapeId { idx: (self.data.len() - 1) as u32, generation: 0 }
    }

    fn get(&self, id: ShapeId) -> Option<&Shape> {
        let slot = self.data.get(id.idx as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.shape.as_ref()
    }
}

