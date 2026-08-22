use super::ids::PointId;

// Constraints bind to points, not shapes — every constraint below reduces to
// equations over point positions. The solver (later pass) iterates these
// projections over the affected subgraph only.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstraintKind {
    Coincident,
    Horizontal,
    Vertical,
    Distance(f64),
}

impl ConstraintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConstraintKind::Coincident => "coincident",
            ConstraintKind::Horizontal => "horizontal",
            ConstraintKind::Vertical => "vertical",
            ConstraintKind::Distance(_) => "distance",
        }
    }

    pub fn value(self) -> Option<f64> {
        match self {
            ConstraintKind::Distance(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub a: PointId,
    pub b: PointId,
    pub kind: ConstraintKind,
}
