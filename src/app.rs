use std::borrow::Cow;

use gpui::{App, AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use gpui_platform::application;

use crate::persistence::registry::Registry;
use crate::theme::{self, ThemeMode};
use crate::ui::actions::*;
use crate::ui::shell::Shell;

const PREF_THEME: &str = "theme";

pub fn run() {
    application().run(|cx: &mut App| {
        load_fonts(cx);
        init_registry(cx);
        theme::init(cx, saved_theme_mode(cx));
        register_action_handlers(cx);
        bind_keys(cx);

        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(400.), px(200.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Parametric".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_, cx| cx.new(Shell::new),
        )
        .unwrap();

        cx.activate(true);
    });
}

fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-VariableFont_wght.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/DepartureMono-Regular.otf")),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .expect("failed to load bundled fonts");
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-x", Cut, None),
        KeyBinding::new("ctrl-c", Copy, None),
        KeyBinding::new("ctrl-v", Paste, None),
        KeyBinding::new("delete", DeleteSelection, None),
        KeyBinding::new("ctrl-z", Undo, None),
        KeyBinding::new("ctrl-shift-z", Redo, None),
        KeyBinding::new("=", ZoomIn, None),
        KeyBinding::new("-", ZoomOut, None),
        KeyBinding::new("shift-1", ZoomToFit, None),
        KeyBinding::new("shift-2", ZoomToSelection, None),
        // Snap-bond choice menu quick keys.
        KeyBinding::new("1", BondCoincident, None),
        KeyBinding::new("2", BondCombinePoints, None),
        KeyBinding::new("escape", BondDismiss, None),
        // Tool selection (canvas focused; also works globally when not renaming)
        KeyBinding::new("v", ToolMove, None),
        KeyBinding::new("space", ToolPan, None),
        KeyBinding::new("m", ToolRuler, None),
        KeyBinding::new("l", ToolLine, None),
        KeyBinding::new("r", ToolRectangle, None),
        KeyBinding::new("a", ToolCircle, None),
    ]);
}

fn register_action_handlers(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleTheme, cx| {
        theme::toggle(cx);
        // Persist the choice so it survives restarts.
        let mode = match theme::mode(cx) {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        if let Some(reg) = cx.try_global::<Registry>() {
            reg.pref_set(PREF_THEME, mode);
        }
    });
}

fn init_registry(cx: &mut App) {
    match Registry::open_default() {
        Ok(reg) => {
            cx.set_global(reg);
        }
        Err(e) => eprintln!("failed to open app registry: {e}"),
    }
}

fn saved_theme_mode(cx: &App) -> Option<ThemeMode> {
    cx.try_global::<Registry>()
        .and_then(|reg| reg.pref_get(PREF_THEME))
        .map(|v| match v.as_str() {
            "light" => ThemeMode::Light,
            _ => ThemeMode::Dark,
        })
}
