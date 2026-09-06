use crate::core::geometry::{Point2, Rect};
use crate::core::ids::{PointId, SegmentId};

// Tool definitions and per-tool pending drag state. Each tool owns a small
// pending-geometry struct; the commit logic lives on Editor.

// Active canvas tool. Move/Pan are modes; shape tools emit element
// composites (the document has no "rectangle" object).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Move,
    Pan,
    Line,
    Rectangle,
    Circle,
    Ruler,
    Dimension,
    Pen,
    ConstraintHorizontalVertical,
    ConstraintTangent,
    ConstraintCoincident,
    ConstraintParallel,
    ConstraintPerpendicular,
}

/// Pen sub-mode: one tool draws lines, arcs, and beziers.
/// Explicit switch only (menu or L/B/A) — drag never changes mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PenMode {
    #[default]
    Line,
    Bezier,
    Arc,
}

impl PenMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PenMode::Line => "line",
            PenMode::Bezier => "bezier",
            PenMode::Arc => "arc",
        }
    }
}

// In-progress bezier span. Clicks are points ON the curve, and every
// point owns TWO handles (in/out — like the committed spans, whose joint
// shows the previous span's far handle plus the next span's near handle):
//  - click, release, click: straight span p0 -> p1 (handles auto);
//  - second press + drag, release: p1 fixes at press, the drag shapes the
//    handle NEAREST the cursor (h1 at the p0 side, h2 at the p1 side).
// `cursor` tracks the live preview.
#[derive(Clone, Copy, Debug)]
pub struct PendingBezier {
    pub p0: Point2,
    pub p1: Option<Point2>,
    pub h1: Option<Point2>,
    pub h2: Option<Point2>,
    pub cursor: Point2,
}

impl PendingBezier {
    /// Effective handles for preview/commit. The drag point is the FORWARD
    /// handle (the visible pair pulled out of the placed point); the curve
    /// itself rides the OPPOSITE side — `c2` mirrors the drag across the
    /// endpoint — so the bend goes opposite the drag, smooth-point style.
    /// Untouched sides fall back to auto thirds.
    pub fn effective(&self, end: Point2) -> (Point2, Point2) {
        let lerp = |t: f64| {
            Point2::new(
                self.p0.x + (end.x - self.p0.x) * t,
                self.p0.y + (end.y - self.p0.y) * t,
            )
        };
        let c1 = self.h1.unwrap_or_else(|| lerp(1. / 3.));
        let c2 = self
            .h2
            .map(|h| Point2::new(2. * end.x - h.x, 2. * end.y - h.y))
            .unwrap_or_else(|| lerp(2. / 3.));
        (c1, c2)
    }
}

// Unified pen preview: exactly one arm is live, matching `PenMode`.
#[derive(Clone, Copy, Debug)]
pub struct PendingPen {
    pub mode: PenMode,
    pub line: Option<PendingLine>,
    pub bezier: Option<PendingBezier>,
    pub circle: Option<PendingCircle>,
}

impl PendingPen {
    pub fn for_mode(mode: PenMode, at: Point2) -> Self {
        match mode {
            PenMode::Line => Self {
                mode,
                line: Some(PendingLine { start: at, cursor: at }),
                bezier: None,
                circle: None,
            },
            PenMode::Bezier => Self {
                mode,
                line: None,
                bezier: Some(PendingBezier { p0: at, p1: None, h1: None, h2: None, cursor: at }),
                circle: None,
            },
            PenMode::Arc => Self {
                mode,
                line: None,
                bezier: None,
                circle: Some(PendingCircle { a: Some(at), b: None, cursor: at }),
            },
        }
    }
}

// In-progress rectangle being dragged out (tool-side preview only).
#[derive(Clone, Copy, Debug)]
pub struct PendingShape {
    pub start: Point2,
    pub cursor: Point2,
    // Shift held: keep width == height (perfect square).
    pub proportional: bool,
}

impl PendingShape {
    pub fn bounds(&self) -> Rect {
        if !self.proportional {
            return Rect::from_points(self.start, self.cursor);
        }
        let dx = self.cursor.x - self.start.x;
        let dy = self.cursor.y - self.start.y;
        let d = dx.abs().max(dy.abs());
        let constrained = Point2::new(
            self.start.x + d * dx.signum(),
            self.start.y + d * dy.signum(),
        );
        Rect::from_points(self.start, constrained)
    }
}

// In-progress line being drawn out (click-click or press-drag-release).
// Shift snaps the direction to 45-degree increments, same as rulers.
#[derive(Clone, Copy, Debug)]
pub struct PendingLine {
    pub start: Point2,
    pub cursor: Point2,
}

impl PendingLine {
    pub fn snapped(&self, shift: bool) -> (Point2, Point2) {
        let (a, b) = (self.start, self.cursor);
        if !shift {
            return (a, b);
        }
        (a, snap_angle(a, b))
    }
}

/// Snaps b's DIRECTION around anchor a to the nearest 45 degrees,
/// preserving its length. Unlike `snap_direction` there is no length
/// quantization — free-form lines must not lock to inch marks.
pub fn snap_angle(a: Point2, b: Point2) -> Point2 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    if dx == 0. && dy == 0. {
        return b;
    }
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    let len = (dx * dx + dy * dy).sqrt();
    Point2::new(a.x + len * angle.cos(), a.y + len * angle.sin())
}

// In-progress circle/arc: stage 1 has `a` set, stage 2 adds the chord end
// `b`, then the cursor acts as the third (on-arc) point until commit.
#[derive(Clone, Copy, Debug)]
pub struct PendingCircle {
    pub a: Option<Point2>,
    pub b: Option<Point2>,
    pub cursor: Point2,
}

impl PendingCircle {
    pub fn stage(&self) -> u8 {
        match (self.a.is_some(), self.b.is_some()) {
            (false, _) => 1,
            (true, false) => 2,
            _ => 3,
        }
    }
}

// In-progress ruler segment being dragged out. Shift snaps the direction
// to 45-degree increments around the start point.
#[derive(Clone, Copy, Debug)]
pub struct PendingRuler {
    pub start: Point2,
    pub cursor: Point2,
}

impl PendingRuler {
    pub fn snapped(&self, shift: bool) -> (Point2, Point2) {
        let (a, b) = (self.start, self.cursor);
        if !shift {
            return (a, b);
        }
        snap_direction(a, b)
    }
}

// Half-inch length quantum for shift-constrained drags.
pub const HALF_INCH: f64 = 48.0;

// Dimension tool: picks accumulating toward a dimension (a point or a
// whole line per click), then a placed value-input state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimPick {
    Point(PointId),
    Line(SegmentId),
}

// A placed dimension awaiting its value: Enter commits the measured value,
// typing digits + Enter commits the typed one, Esc cancels. `existing`
// marks an EDIT of an already-placed dimension (double-click) — Enter
// updates that dimension instead of creating one.
#[derive(Clone, Debug)]
pub struct DimInput {
    pub target: crate::core::constraints::DimTarget,
    pub offset: f64,
    pub slide: f64,
    pub measured: f64,
    pub buffer: String,
    pub existing: Option<usize>,
}

/// Snaps the b end around anchor a to the nearest 45 degrees AND the
/// length to half-inch steps, preserving intent of precise rulers.
pub fn snap_direction(a: Point2, b: Point2) -> (Point2, Point2) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    if dx == 0. && dy == 0. {
        return (a, b);
    }
    let step = std::f64::consts::FRAC_PI_4;
    let angle = dy.atan2(dx);
    let snapped = (angle / step).round() * step;
    let len = ((dx * dx + dy * dy).sqrt() / HALF_INCH).round() * HALF_INCH;
    (a, Point2::new(a.x + len * snapped.cos(), a.y + len * snapped.sin()))
}
