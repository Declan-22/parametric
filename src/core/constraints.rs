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

// One stored dimension — the tool-created measurement constraint. Always
// locked: `value` is the distance (doc units) or angle (degrees) the
// geometry is held to (point-pair dims also feed the solver as Distance
// equations; line/angle dims constrain nothing yet but render + persist).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dimension {
    pub target: DimTarget,
    pub value: f64,
    // Placement: signed perpendicular offset of the dim line from the
    // measured geometry, in doc units. For angles: the arc radius.
    pub offset: f64,
    // Placement: slide of the container along the dim line (doc units).
    // For angles: fractional position across the sweep (0..1).
    pub slide: f64,
    // ANGLE dims only: the SIGNED placed sweep in degrees (-360..360) —
    // the rotation direction from the first line's ray to the second,
    // captured at placement so the constraint and the drawn arc always
    // agree (label vs geometry never invert).
    pub sweep: f64,
}

/// What a dimension measures and between what.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimTarget {
    /// Straight-line distance between two points.
    Points { a: PointId, b: PointId },
    /// Perpendicular distance from a point to a line.
    PointLine { p: PointId, line: SegmentId },
    /// Perpendicular distance between two parallel lines.
    Lines { a: SegmentId, b: SegmentId },
    /// Angle between two lines, in degrees.
    Angle { a: SegmentId, b: SegmentId },
    /// Radius of an arc/circle: dashed line from its center to the bend.
    Radius { seg: SegmentId },
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
