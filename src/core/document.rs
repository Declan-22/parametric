use super::geometry::Rect;

// The permanent design. "What exists in the document?"
// No GPUI types here — the engine is UI-independent.

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub layers: Vec<Layer>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub shapes: Vec<Shape>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Rectangle(Rect),
    Ellipse(Rect),
}

impl Shape {
    pub fn bounds(&self) -> Rect {
        match self {
            Shape::Rectangle(r) | Shape::Ellipse(r) => *r,
        }
    }

    // Stable string form used by persistence to store `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Rectangle(_) => "rectangle",
            Shape::Ellipse(_) => "ellipse",
        }
    }
}
