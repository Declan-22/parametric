use crate::core::geometry::{Point2, Rect};

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
