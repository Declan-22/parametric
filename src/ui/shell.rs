use std::time::Duration;

use gpui::{
    prelude::*, Context, MouseButton, Window, div, px, rgb,
};

use crate::theme::{self, ThemeState};
use crate::ui::title_bar::{TITLE_BAR_HEIGHT, TitleBar};

pub struct Shell {
    pub(crate) open_menu: Option<usize>,
    pub(crate) menu_animation: f32,
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<ThemeState>(|_, cx| cx.notify())
            .detach();
        Self {
            open_menu: None,
            menu_animation: 0.0,
        }
    }

    pub(crate) fn start_menu_animation(&mut self, cx: &mut Context<Self>) {
        self.menu_animation = 0.0;
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let steps = 6;
            for i in 1..=steps {
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    shell.menu_animation = i as f32 / steps as f32;
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_darker))
            .text_color(rgb(t.text_primary))
            .child(TitleBar {
                open_menu: self.open_menu,
                shell: cx.entity().downgrade(),
                animation: self.menu_animation,
            })
            .child(div().flex_1())
            .when(self.open_menu.is_some(), |d| {
                let shell = cx.entity().downgrade();
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .top(px(TITLE_BAR_HEIGHT))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            let _ = shell.update(cx, |shell, cx| {
                                shell.open_menu = None;
                                shell.menu_animation = 0.0;
                                cx.notify();
                            });
                        }),
                )
            })
    }
}
