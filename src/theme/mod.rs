pub mod colors;
pub mod state;
pub mod theme;

pub use state::{ThemeState, active, init, mode, toggle};
pub use theme::{ACCENT, FONT_UI, Theme, ThemeMode, fade_in, lerp_rgb, lerp_rgba};
