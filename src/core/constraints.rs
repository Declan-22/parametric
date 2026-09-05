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
    // A line and circular arc share a tangent direction at `point`.
    Tangent,
    // Two straight line segments have parallel directions.
    Parallel,
    // Two straight line segments meet at a right angle.
    Perpendicular,
}

impl ConstraintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConstraintKind::Coincident => "coincident",
            ConstraintKind::Horizontal => "horizontal",
            ConstraintKind::Vertical => "vertical",
            ConstraintKind::Tangent => "tangent",
            ConstraintKind::Parallel => "parallel",
            ConstraintKind::Perpendicular => "perpendicular",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub a: PointId,
    pub b: PointId,
    // Tangent constraints need the two owning segments and the contact
    // point; the point pair above remains useful for chip ownership and for
    // backwards-compatible persistence of the older constraints.
    pub tangent_segments: Option<(SegmentId, SegmentId)>,
    // For a Coincident point-to-edge constraint, `a` is the point and this
    // identifies the edge it must remain on.
    pub point_on_segment: Option<SegmentId>,
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

/// Orientation of a point-pair distance dimension. Fusion-style: the mouse
/// position at placement picks which of the three the dim measures; Aligned
/// is the straight displacement, X/Y the axis-projected spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimMode {
    Aligned,
    X,
    Y,
}

impl DimMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DimMode::Aligned => "",
            DimMode::X => "_x",
            DimMode::Y => "_y",
        }
    }

    pub fn parse_suffix(kind: &str) -> (bool, DimMode) {
        match kind {
            "points_x" => (true, DimMode::X),
            "points_y" => (true, DimMode::Y),
            _ => (kind == "points", DimMode::Aligned),
        }
    }
}

impl DimTarget {
    /// Returns the target with a point-pair mode applied (no-op for target
    /// kinds that carry no mode).
    pub fn with_mode(self, mode: DimMode) -> Self {
        match self {
            DimTarget::Points { a, b, .. } => DimTarget::Points { a, b, mode },
            other => other,
        }
    }
}

/// What a dimension measures and between what.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimTarget {
    /// Distance between two points — straight (Aligned) or the X/Y span.
    Points { a: PointId, b: PointId, mode: DimMode },
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
