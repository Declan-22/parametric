// Generational IDs: stable handles into arena storage. A stale ID (pointing
// at a slot whose generation moved on) resolves to None instead of wrong data.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointId {
    pub idx: u32,
    pub generation: u32,
}

impl PointId {
    pub const NONE: Self = Self { idx: u32::MAX, generation: 0 };
    pub fn is_some(self) -> bool {
        self.idx != u32::MAX
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId {
    pub idx: u32,
    pub generation: u32,
}

