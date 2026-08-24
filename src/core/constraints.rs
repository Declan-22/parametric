use super::ids::{FillId, PointId, SegmentId};

// Geometric constraints bind to POINTS, not shapes — every constraint below
// reduces to equations over point positions. A future solver iterates these
// projections over the affected subgraph only; until then the editor applies
// them as drag-time clamps.
//
// Dimensional measurements live in Document::dimensions (bound to a point
// pair, angle-agnostic); a locked Dimension is enforced like a Distance
// constraint but also carries display metadata.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstraintKind {
    // The two points occupy the same location (usually via a shared id,
    // making this implicit; explicit coincidences support distinct ids).
    Coincident,
    // The two points share a Y coordinate.
    Horizontal,
    // The two points share an X coordinate.
    Vertical,
}

impl ConstraintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConstraintKind::Coincident => "coincident",
            ConstraintKind::Horizontal => "horizontal",
            ConstraintKind::Vertical => "vertical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub a: PointId,
    pub b: PointId,
}

// One stored measurement between two points. Renders along whatever angle
// the a->b axis implies (diagonal edges get diagonal dim lines). When
// `value` is Some, the pair behaves as a locked distance during edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dimension {
    pub a: PointId,
    pub b: PointId,
    // Locked length; None = measuring only, follows the geometry freely.
    pub value: Option<f64>,
    // Signed perpendicular offset from the midpoint along the LEFT normal
    // of (b - a), in document units — controls which side the dim line sits on.
    pub offset: f64,
}

// A reference to any first-class document element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElementRef {
    Point(PointId),
    Segment(SegmentId),
    Fill(FillId),
}

impl ElementRef {
    pub fn as_point(self) -> Option<PointId> {
        match self {
            ElementRef::Point(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_segment(self) -> Option<SegmentId> {
        match self {
            ElementRef::Segment(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_fill(self) -> Option<FillId> {
        match self {
            ElementRef::Fill(f) => Some(f),
            _ => None,
        }
    }
}
