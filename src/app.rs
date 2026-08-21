use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use gpui_platform::application;

use crate::theme;
use crate::ui::actions::*;
use crate::ui::shell::Shell;

pub fn run() {
    application().run(|cx: &mut App| {
        theme::init(cx);
        register_action_handlers(cx);

        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(640.))),
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

fn register_action_handlers(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleTheme, cx| theme::toggle(cx));
}
