use super::colors::*;use gpui::{BoxShadow, Point, px, rgba};

pub const FONT_UI: &str = "Geist";
pub const FONT_MONO: &str = "Departure Mono";

// Accent color for interactive indicators (selection outlines, handles).
pub const ACCENT: u32 = 0x4C8DFF;

// Snap indicator color (guides + snap marker) â€” distinct from accent.
pub const SNAP: u32 = 0xFF9500;

// Fades an RGBA color's alpha byte in from transparent (0xRRGGBBAA colors).
pub fn fade_in(color: u32, k: f32) -> u32 {
    let a = ((color & 0xFF) as f32 * k).round() as u32;
    (color & 0xFFFFFF00) | a
}

// Linear blend of two 0xRRGGBB colors (alpha untouched).
pub fn lerp_rgb(from: u32, to: u32, k: f32) -> u32 {
    let ch = |c: u32, shift: u32| ((c >> shift) & 0xFF) as f32;
    let mix = |shift: u32| (ch(from, shift) + (ch(to, shift) - ch(from, shift)) * k).round() as u32;
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

// Linear blend of two 0xRRGGBBAA colors including the alpha byte.
pub fn lerp_rgba(from: u32, to: u32, k: f32) -> u32 {
    let ch = |c: u32, shift: u32| ((c >> shift) & 0xFF) as f32;
    let mix = |shift: u32| (ch(from, shift) + (ch(to, shift) - ch(from, shift)) * k).round() as u32;
    (mix(24) << 24) | lerp_rgb(from & 0xFFFFFF, to & 0xFFFFFF, k)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg_primary: u32,
    pub bg_secondary: u32,
    pub bg_tertiary: u32,
    pub bg_darker: u32,

    pub border_color: u32,
    pub component_border_color: u32,

    pub text_primary: u32,
    pub text_secondary: u32,

    pub empty_text_primary: u32,
    pub empty_text_secondary: u32,

    pub shadow_color: u32,
    pub item_shadow_color: u32,

    pub button_background: u32,
    pub button_text: u32,
    pub button_border_color: u32,

    pub accent: u32,
    // Accent-adjacent border for emphasized accent surfaces (chips):
    // darker than accent in light mode, brighter in dark mode.
    pub accent_border: u32,
}

impl Theme {
    // Super thin, subtle shadow for popups and floating surfaces.
    pub fn shadow_md(&self) -> BoxShadow {
        BoxShadow {
            color: rgba(self.shadow_color).into(),
            offset: Point {
                x: px(0.),
                y: px(0.75),
            },
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        }
    }

    // Barely-there shadow for hovered / active items.
    pub fn shadow_sm(&self) -> BoxShadow {
        BoxShadow {
            color: rgba(self.item_shadow_color).into(),
            offset: Point {
                x: px(0.),
                y: px(0.75),
            },
            blur_radius: px(1.5),
            spread_radius: px(0.),
            inset: false,
        }
    }

    pub fn light() -> Self {
        Self {
            bg_primary: LIGHT_BG_PRIMARY,
            bg_secondary: LIGHT_BG_SECONDARY,
            bg_tertiary: LIGHT_BG_TERTIARY,
            bg_darker: LIGHT_BG_DARKER,

            border_color: LIGHT_BORDER_COLOR,
            component_border_color: LIGHT_COMPONENT_BORDER_COLOR,

            text_primary: LIGHT_TEXT_PRIMARY,
            text_secondary: LIGHT_TEXT_SECONDARY,

            empty_text_primary: LIGHT_EMPTY_TEXT_PRIMARY,
            empty_text_secondary: LIGHT_EMPTY_TEXT_SECONDARY,

            shadow_color: LIGHT_SHADOW_COLOR,
            item_shadow_color: LIGHT_ITEM_SHADOW_COLOR,

            button_background: LIGHT_BUTTON_BACKGROUND,
            button_text: LIGHT_BUTTON_TEXT,
            button_border_color: LIGHT_BUTTON_BORDER_COLOR,

            accent: LIGHT_ACCENT,
            accent_border: LIGHT_ACCENT_BORDER,
        }
    }

    pub fn dark() -> Self {
        Self {
            bg_primary: DARK_BG_PRIMARY,
            bg_secondary: DARK_BG_SECONDARY,
            bg_tertiary: DARK_BG_TERTIARY,
            bg_darker: DARK_BG_DARKER,

            border_color: DARK_BORDER_COLOR,
            component_border_color: DARK_COMPONENT_BORDER_COLOR,

            text_primary: DARK_TEXT_PRIMARY,
            text_secondary: DARK_TEXT_SECONDARY,

            empty_text_primary: DARK_EMPTY_TEXT_PRIMARY,
            empty_text_secondary: DARK_EMPTY_TEXT_SECONDARY,

            shadow_color: DARK_SHADOW_COLOR,
            item_shadow_color: DARK_ITEM_SHADOW_COLOR,

            button_background: DARK_BUTTON_BACKGROUND,
            button_text: DARK_BUTTON_TEXT,
            button_border_color: DARK_BUTTON_BORDER_COLOR,

            accent: DARK_ACCENT,
            accent_border: DARK_ACCENT_BORDER,
        }
    }
}
