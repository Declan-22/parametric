// Fundamental geometry for the design engine. f64 document units —
// pixels are a view concept handled by the editor camera.

pub const COORD_LIMIT: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn clamped(self) -> Self {
        Self {
            x: self.x.clamp(-COORD_LIMIT, COORD_LIMIT),
            y: self.y.clamp(-COORD_LIMIT, COORD_LIMIT),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub origin: Point2,
    pub size: Size2,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            origin: Point2::new(x, y),
            size: Size2::new(w, h),
        }
    }

    pub fn center(&self) -> Point2 {
        Point2::new(
            self.origin.x + self.size.w / 2.,
            self.origin.y + self.size.h / 2.,
        )
    }

    pub fn contains(&self, p: Point2) -> bool {
        p.x >= self.origin.x
            && p.x <= self.origin.x + self.size.w
            && p.y >= self.origin.y
            && p.y <= self.origin.y + self.size.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size2 {
    pub w: f64,
    pub h: f64,
}

impl Size2 {
    pub fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}
