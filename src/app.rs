use std::borrow::Cow;

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use gpui_platform::application;

use crate::theme;
use crate::ui::actions::*;
use crate::ui::shell::Shell;

pub fn run() {
    application().run(|cx: &mut App| {
        load_fonts(cx);
        theme::init(cx);
        register_action_handlers(cx);

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

fn register_action_handlers(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleTheme, cx| theme::toggle(cx));
}
