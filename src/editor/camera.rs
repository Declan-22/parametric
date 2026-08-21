use crate::core::geometry::Point2;

pub const MIN_ZOOM: f64 = 0.01;
pub const MAX_ZOOM: f64 = 100.0;

// Transforms document units <-> screen pixels for the current view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Document-space position rendered at the screen origin.
    pub pan: Point2,
    pub zoom: f64,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            pan: Point2::new(0., 0.),
            zoom: 1.0,
        }
    }

    pub fn unit_to_screen(&self, p: Point2) -> Point2 {
        Point2::new(
            (p.x - self.pan.x) * self.zoom,
            (p.y - self.pan.y) * self.zoom,
        )
    }

    pub fn screen_to_unit(&self, p: Point2) -> Point2 {
        Point2::new(p.x / self.zoom + self.pan.x, p.y / self.zoom + self.pan.y)
    }

    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}
