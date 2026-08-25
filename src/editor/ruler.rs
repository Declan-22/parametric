use super::Camera;
use crate::core::document::{Document, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::SegmentId;

// The ruler component's procedural vector design: one baseline (the stored
// segment itself) with perpendicular tick dashes across it, 16 per inch
// (96 document px = 1in), plus an inch label at every full inch mark.
//
// This module is the SINGLE source of truth for the pattern — the paint
// layer renders ticks from `ticks()` and text from `labels()`.

pub const INCH: f64 = 96.0;
// Half-inch quantum; also the graduation snap spacing.
pub const HALF_INCH: f64 = INCH / 2.0;
const SIXTEENTH: f64 = INCH / 16.0;

// Tick heights in document units.
const H_SMALL: f64 = 7.0;
const H_QUARTER: f64 = 11.0;
const H_HALF: f64 = 15.0;
const H_INCH: f64 = 19.0;

// Label placement.
const LABEL_OFFSET: f64 = 8.0;

/// One tick: its base point on the baseline and its height along the
/// left-normal of a->b.
pub struct Tick {
    pub base: Point2,
    pub tip: Point2,
    pub inch_mark: bool,
}

/// Full tick pattern for a ruler from a to b. Procedural: longer rulers
/// simply produce more ticks. The k=0 tick is the a endpoint's inch mark.
pub fn ticks(a: Point2, b: Point2) -> Vec<Tick> {
    let Some((ux, uy, nx, ny)) = frame(a, b) else {
        return Vec::new();
    };
    // Canonical side: ticks point DOWN on screen, matching the label
    // side — reversed rulers render identically to forward ones.
    let (nx, ny) = if ny < 0. { (-nx, -ny) } else { (nx, ny) };
    let len = length(a, b);
    let steps = (len / SIXTEENTH).floor() as usize;
    let mut out = Vec::with_capacity(steps + 1);
    for k in 0..=steps {
        let m = k % 16;
        let h = match m {
            0 => H_INCH,
            8 => H_HALF,
            4 | 12 => H_QUARTER,
            _ => H_SMALL,
        };
        let d = SIXTEENTH * k as f64;
        let base = Point2::new(a.x + ux * d, a.y + uy * d);
        out.push(Tick {
            base,
            tip: Point2::new(base.x + nx * h, base.y + ny * h),
            inch_mark: m == 0,
        });
    }
    out
}

/// One label per FULL inch along the segment, anchored just beyond the
/// tick tips on the canonical (down-screen) side. Returns (anchor,
/// pixel count, inch count).
pub fn labels(a: Point2, b: Point2) -> Vec<(Point2, i64, usize)> {
    let Some((ux, uy, nx, ny)) = frame(a, b) else {
        return Vec::new();
    };
    let (nx, ny) = if ny < 0. { (-nx, -ny) } else { (nx, ny) };
    let len = length(a, b);
    let inches = (len / INCH).floor() as usize;
    let mut out = Vec::with_capacity(inches);
    for k in 1..=inches {
        let d = INCH * k as f64;
        out.push((
            Point2::new(
                a.x + ux * d + nx * (H_INCH + LABEL_OFFSET),
                a.y + uy * d + ny * (H_INCH + LABEL_OFFSET),
            ),
            INCH as i64 * k as i64,
            k,
        ));
    }
    out
}

/// True when this segment renders as a ruler component.
pub fn is_ruler(doc: &Document, sid: SegmentId) -> bool {
    doc.segment(sid).is_some_and(|s| s.kind == SegmentKind::Ruler)
}

fn length(a: Point2, b: Point2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

// Unit direction and left normal; None for zero-length.
fn frame(a: Point2, b: Point2) -> Option<(f64, f64, f64, f64)> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return None;
    }
    let ux = dx / len;
    let uy = dy / len;
    Some((ux, uy, -uy, ux))
}

/// Screen-space conversion helper shared by paint layers.
pub fn to_screen(cam: &Camera, p: Point2) -> (f32, f32) {
    let s = cam.unit_to_screen(p);
    (s.x as f32, s.y as f32)
}
