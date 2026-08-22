use gpui::{App, BorrowAppContext, Global, WindowAppearance};

use super::theme::{Theme, ThemeMode};

pub struct ThemeState {
    pub mode: ThemeMode,
    pub theme: Theme,
}

impl Global for ThemeState {}

pub fn init(cx: &mut App) {
    let mode = match cx.window_appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
    };
    cx.set_global(ThemeState {
        mode,
        theme: match mode {
            ThemeMode::Light => Theme::light(),
            ThemeMode::Dark => Theme::dark(),
        },
    });
}

pub fn active(cx: &App) -> &Theme {
    &cx.global::<ThemeState>().theme
}

pub fn mode(cx: &App) -> ThemeMode {
    cx.global::<ThemeState>().mode
}

pub fn set_mode(new_mode: ThemeMode, cx: &mut App) {
    let theme = match new_mode {
        ThemeMode::Light => Theme::light(),
        ThemeMode::Dark => Theme::dark(),
    };
    cx.update_global(|state: &mut ThemeState, _| {
        state.mode = new_mode;
        state.theme = theme;
    });
}

pub fn toggle(cx: &mut App) {
    let next = match mode(cx) {
        ThemeMode::Light => ThemeMode::Dark,
        ThemeMode::Dark => ThemeMode::Light,
    };
    set_mode(next, cx);
}
