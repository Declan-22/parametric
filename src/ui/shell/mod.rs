use std::time::{Duration, Instant};

use gpui::{Context, MouseButton, Point, Window, div, prelude::*, px, rgb};

use crate::theme::{self, ThemeState};
pub mod title_bar;

use crate::ui::menu::dropdown::AppMenu;
use crate::ui::shell::title_bar::{TITLE_BAR_HEIGHT, TitleBar};

pub struct Shell {
    pub(crate) menu_open: bool,
    pub(crate) active_menu: Option<usize>,
    pub(crate) menu_animation: f32,
    pub(crate) icon_animation: f32,
    pub(crate) cursor_trail: Vec<(Point<gpui::Pixels>, Instant)>,
    pub(crate) hovered_entry: Option<usize>,
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<ThemeState>(|_, cx| cx.notify())
            .detach();
        Self {
            menu_open: false,
            active_menu: None,
            menu_animation: 0.0,
            icon_animation: 0.0,
            cursor_trail: Vec::new(),
            hovered_entry: None,
        }
    }

    pub(crate) fn record_cursor(&mut self, pos: Point<gpui::Pixels>) {
        let now = Instant::now();
        self.cursor_trail.push((pos, now));
        self.cursor_trail
            .retain(|(_, t)| now.duration_since(*t).as_millis() <= 120);
    }

    // Velocity in px/ms over the recent trail.
    pub(crate) fn cursor_velocity(&self) -> (f32, f32) {
        let Some((first, t0)) = self.cursor_trail.first() else {
            return (0., 0.);
        };
        let Some((last, t1)) = self.cursor_trail.last() else {
            return (0., 0.);
        };
        let dt = t1.duration_since(*t0).as_secs_f32();
        if dt < f32::EPSILON {
            return (0., 0.);
        }
        (
            (last.x - first.x).as_f32() / dt,
            (last.y - first.y).as_f32() / dt,
        )
    }

    pub(crate) fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        self.active_menu = None;
        self.menu_animation = 0.0;
        self.animate_icon(self.menu_open, cx);
        if self.menu_open {
            self.start_menu_animation(cx);
        }
    }

    // Fades the menu icon between its idle and active styling.
    pub(crate) fn animate_icon(&mut self, opening: bool, cx: &mut Context<Self>) {
        let start = self.icon_animation;
        let end = if opening { 1.0 } else { 0.0 };
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let steps = 6;
            for i in 1..=steps {
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    shell.icon_animation = start + (end - start) * (i as f32 / steps as f32);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if !self.menu_open {
            return;
        }
        self.menu_open = false;
        self.active_menu = None;
        self.menu_animation = 0.0;
        self.animate_icon(false, cx);
        cx.notify();
    }

    pub(crate) fn set_active_menu(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.active_menu == index {
            return;
        }
        self.active_menu = index;
        cx.notify();
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
            .bg(rgb(t.bg_primary))
            .text_color(rgb(t.text_primary))
            .font_family(theme::FONT_UI)
            .child(TitleBar {
                menu_open: self.menu_open,
                icon_animation: self.icon_animation,
                shell: cx.entity().downgrade(),
            })
            .child(div().flex_1())
            .when(self.menu_open, |d| {
                let shell = cx.entity().downgrade();
                d.child(
                    // Click-away catcher below the title bar.
                    div()
                        .absolute()
                        .inset_0()
                        .top(px(TITLE_BAR_HEIGHT))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            let _ = shell.update(cx, |shell, cx| shell.close_menu(cx));
                        }),
                )
                .child(AppMenu {
                    shell: cx.entity().downgrade(),
                    active: self.active_menu,
                    animation: self.menu_animation,
                })
            })
    }
}
