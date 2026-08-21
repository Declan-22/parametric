use super::colors::*;

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

    pub text_primary: u32,
    pub text_secondary: u32,

    pub empty_text_primary: u32,
    pub empty_text_secondary: u32,
}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg_primary: LIGHT_BG_PRIMARY,
            bg_secondary: LIGHT_BG_SECONDARY,
            bg_tertiary: LIGHT_BG_TERTIARY,
            bg_darker: LIGHT_BG_DARKER,

            border_color: LIGHT_BORDER_COLOR,

            text_primary: LIGHT_TEXT_PRIMARY,
            text_secondary: LIGHT_TEXT_SECONDARY,

            empty_text_primary: LIGHT_EMPTY_TEXT_PRIMARY,
            empty_text_secondary: LIGHT_EMPTY_TEXT_SECONDARY,
        }
    }

    pub fn dark() -> Self {
        Self {
            bg_primary: DARK_BG_PRIMARY,
            bg_secondary: DARK_BG_SECONDARY,
            bg_tertiary: DARK_BG_TERTIARY,
            bg_darker: DARK_BG_DARKER,

            border_color: DARK_BORDER_COLOR,

            text_primary: DARK_TEXT_PRIMARY,
            text_secondary: DARK_TEXT_SECONDARY,

            empty_text_primary: DARK_EMPTY_TEXT_PRIMARY,
            empty_text_secondary: DARK_EMPTY_TEXT_SECONDARY,
        }
    }
}
