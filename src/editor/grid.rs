// The document grid: one fixed base cell, subdivided 5x per level as you
// zoom in (and coarsened 5x as you zoom out) so screen spacing stays in a
// readable band. The grid SIZE is not a setting — everything derives from
// GRID_BASE and the camera zoom, and the drawn lattice IS the snap lattice:
// paint (push_grid) and snapping (snap_step) share this module so they can
// never drift apart.

/// Fixed base cell size in document units.
pub const GRID_BASE: f64 = 20.0;

// Screen-space spacing band the minor level is kept inside. Same constants
// as the original push_grid LOD logic.
const MIN_SCREEN_PX: f64 = 16.0;
const MAX_SCREEN_PX: f64 = 80.0;

/// Which lattice levels are drawn at this zoom, and their doc-unit steps.
/// `minor` is the base-band level; major = minor*5 and finer = minor/5 are
/// its 5x neighbors. `major_visible` is reported for completeness — major
/// intersections always lie on the minor/finer lattice, so snapping only
/// ever needs the finest visible level (see `snap_step`).
pub struct GridLevels {
    pub minor: f64,
    pub minor_visible: bool,
    pub finer_visible: bool,
    pub major_visible: bool,
}

pub fn levels(zoom: f64) -> GridLevels {
    let mut minor = GRID_BASE;
    let mut minor_screen = minor * zoom;
    // Bring the minor level into the readable band by 5x steps. (Stable,
    // not 2x — matches the drawn grid's subdivision rhythm.)
    for _ in 0..16 {
        if minor_screen >= MIN_SCREEN_PX && minor_screen <= MAX_SCREEN_PX {
            break;
        }
        if minor_screen < MIN_SCREEN_PX {
            minor *= 5.0;
            minor_screen *= 5.0;
        } else if minor_screen > MAX_SCREEN_PX {
            minor /= 5.0;
            minor_screen /= 5.0;
        }
        if minor > 1e7 || minor < 1e-6 {
            return GridLevels {
                minor: GRID_BASE,
                minor_visible: false,
                finer_visible: false,
                major_visible: false,
            };
        }
    }
    // Clamp once more if still out of band (extreme zoom) — falls back to
    // the base level, mirroring push_grid.
    if minor_screen < MIN_SCREEN_PX * 0.5 || minor_screen > MAX_SCREEN_PX * 2.0 {
        minor = GRID_BASE;
        minor_screen = minor * zoom;
    }

    let major_screen = minor_screen * 5.0;
    let finer_screen = minor_screen / 5.0;

    let finer_visible = finer_screen >= 8.0 && finer_screen < MIN_SCREEN_PX * 2.0;
    let minor_visible = minor_screen >= MIN_SCREEN_PX && minor_screen <= MAX_SCREEN_PX * 2.0;
    let major_visible = major_screen >= MIN_SCREEN_PX && major_screen <= 600.0;

    GridLevels {
        minor,
        minor_visible,
        finer_visible,
        major_visible,
    }
}

/// The finest DRAWN lattice step at this zoom, in document units. Snapping
/// targets are exactly the intersections of the drawn grid: when zooming in
/// makes a new 5x subdivision appear, its intersections become snappable the
/// moment they're visible; zooming out coarsens the lattice the same way.
pub fn snap_step(zoom: f64) -> f64 {
    if zoom < 1e-9 {
        return GRID_BASE;
    }
    let l = levels(zoom);
    if l.finer_visible {
        l.minor / 5.0
    } else if l.minor_visible {
        l.minor
    } else if l.major_visible {
        l.minor * 5.0
    } else {
        // Extreme zoom: push_grid's fallback draws the base level.
        GRID_BASE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_zoom_snaps_to_base_grid() {
        // At zoom 1 the 20-unit cells are 20px on screen — in band, and the
        // finer 4-unit level is only 4px (not drawn).
        assert_eq!(snap_step(1.0), 20.0);
    }

    #[test]
    fn zoom_in_snaps_to_subdivisions() {
        // Zoom in enough that the finer level (base/5) becomes visible.
        // base*zoom = 80+ -> minor drops to base/5 = 4 units, screen 16px+;
        // finer = 0.8 units. Any way you slice it, the step must be a
        // division of the base by 5^k.
        let step = snap_step(10.0);
        let k = (GRID_BASE / step).round();
        assert!((GRID_BASE / k - step).abs() < 1e-9);
        assert!(k >= 5.0, "zoomed in should subdivide, got {step}");
        // And the step must be what's actually drawn: its screen size is
        // inside the visible band for subdivisions or the minor band.
        let screen = step * 10.0;
        assert!(screen >= 8.0, "snapping to an invisible lattice, {screen}");
    }

    #[test]
    fn zoom_out_coarsens_the_lattice() {
        let step = snap_step(0.1);
        assert!(step > GRID_BASE, "zoomed out should coarsen, got {step}");
    }

    #[test]
    fn extreme_zoom_falls_back_to_base() {
        assert_eq!(snap_step(0.0), GRID_BASE);
        assert_eq!(snap_step(1e12), GRID_BASE);
    }
}
