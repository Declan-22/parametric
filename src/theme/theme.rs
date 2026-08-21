use super::colors::*;
use gpui::{BoxShadow, Point, px, rgba};

pub const FONT_UI: &str = "Geist";
pub const FONT_MONO: &str = "Departure Mono";

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
        }
    }
}
