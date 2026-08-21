use gpui::{
    AnyElement, App, MouseButton, SharedString, WeakEntity, Window, WindowControlArea, anchored,
    deferred, div, prelude::*, px, rgb, svg,
};

use crate::theme::Theme;
use crate::ui::menu::default_menus;
use crate::ui::shell::Shell;

pub const TITLE_BAR_HEIGHT: f32 = 36.0;

const HOME_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none"/><path fill="currentColor" d="M21 14.08h-6.92V5.77H5.77v8.31h8.31V21H3V5.77h2.77V3H21z"/></svg>"#;

#[derive(IntoElement)]
pub struct TitleBar {
    pub open_menu: Option<usize>,
    pub shell: WeakEntity<Shell>,
    pub animation: f32,
}

impl RenderOnce for TitleBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let menus = default_menus();

        div()
            .w_full()
            .h(px(TITLE_BAR_HEIGHT))
            .flex()
            .flex_row()
            .items_stretch()
            .bg(rgb(t.bg_primary))
            .border_b_1()
            .border_color(rgb(t.border_color))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .pl(px(6.))
                    .pr(px(4.))
                    .gap(px(2.))
                    .child(
                        div()
                            .id("home-button")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(32.))
                            .h(px(28.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover({
                                let t = t;
                                move |s| s.bg(rgb(t.bg_secondary))
                            })
                            .child(
                                svg()
                                    .data(HOME_ICON)
                                    .w(px(18.))
                                    .h(px(18.))
                                    .text_color(rgb(t.text_primary)),
                            ),
                    )
                    .children(
                        menus
                            .iter()
                            .enumerate()
                            .map(|(i, menu)| self.render_menu_button(i, menu, t)),
                    ),
            )
            // Spacer — only this region is draggable so it doesn't
            // swallow the close / min / max hitboxes.
            .child(div().flex_1().window_control_area(WindowControlArea::Drag))
            .child(render_window_controls(t))
    }
}

impl TitleBar {
    fn render_menu_button(
        &self,
        index: usize,
        menu: &crate::ui::menu::Menu,
        t: Theme,
    ) -> AnyElement {
        let is_open = self.open_menu == Some(index);
        let switch_on_hover = self.open_menu.is_some() && !is_open;

        let mut button = div()
            .id(SharedString::from(format!("menu-button-{index}")))
            .flex()
            .items_center()
            .px(px(10.))
            .h(px(26.))
            .rounded(px(4.))
            .text_sm()
            .text_color(rgb(t.text_primary))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(t.bg_secondary)))
            .on_mouse_down(MouseButton::Left, {
                let shell = self.shell.clone();
                move |_, _window, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.open_menu = if shell.open_menu == Some(index) {
                            None
                        } else {
                            Some(index)
                        };
                        shell.start_menu_animation(cx);
                        cx.notify();
                    });
                }
            })
            .on_hover({
                let shell = self.shell.clone();
                move |hovered, _, cx| {
                    if *hovered && switch_on_hover {
                        let _ = shell.update(cx, |shell, cx| {
                            shell.open_menu = Some(index);
                            shell.start_menu_animation(cx);
                            cx.notify();
                        });
                    }
                }
            })
            .child(menu.label.clone());

        if is_open {
            button = button.bg(rgb(t.bg_tertiary));
            button = button.child(render_popup(menu, t, self.animation));
        }

        button.into_any_element()
    }
}

fn render_popup(menu: &crate::ui::menu::Menu, t: Theme, animation: f32) -> AnyElement {
    let opacity = animation.min(1.0);

    deferred(
        anchored().snap_to_window().child(
            div()
                .occlude()
                .mt(px(36.))
                .w(px(232.))
                .flex()
                .flex_col()
                .py(px(4.))
                .bg(rgb(t.bg_secondary))
                .border_1()
                .border_color(rgb(t.border_color))
                .rounded(px(8.))
                .shadow_md()
                .opacity(opacity)
                .children(
                    menu.items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| render_item(item, i, t)),
                ),
        ),
    )
    .with_priority(1)
    .into_any_element()
}

fn render_item(item: &crate::ui::menu::MenuItem, index: usize, t: Theme) -> AnyElement {
    match item {
        crate::ui::menu::MenuItem::Separator => div()
            .h(px(1.))
            .mx(px(10.))
            .my(px(5.))
            .bg(rgb(t.border_color))
            .into_any_element(),
        crate::ui::menu::MenuItem::Entry(entry) => {
            let action = entry.action.boxed_clone();
            div()
                .id(SharedString::from(format!("menu-item-{index}")))
                .mx(px(6.))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(10.))
                .py(px(5.))
                .rounded(px(4.))
                .text_sm()
                .text_color(rgb(t.text_primary))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(t.bg_tertiary)))
                .on_click(move |_, window, cx| {
                    window.dispatch_action(action.boxed_clone(), cx);
                })
                .child(entry.label.clone())
                .children(entry.shortcut.clone().map(|shortcut| {
                    div()
                        .pl(px(24.))
                        .text_xs()
                        .text_color(rgb(t.text_secondary))
                        .child(shortcut)
                }))
                .into_any_element()
        }
    }
}

fn render_window_controls(t: Theme) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_stretch()
        .child(
            div()
                .id("window-minimize")
                .w(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(WindowControlArea::Min)
                .hover(move |s| s.bg(rgb(t.bg_secondary)))
                .child(div().w(px(10.)).h(px(1.)).bg(rgb(t.text_secondary))),
        )
        .child(
            div()
                .id("window-maximize")
                .w(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(WindowControlArea::Max)
                .hover(move |s| s.bg(rgb(t.bg_secondary)))
                .child(
                    div()
                        .size(px(9.))
                        .border_1()
                        .border_color(rgb(t.text_secondary)),
                ),
        )
        .child(
            div()
                .id("window-close")
                .w(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(WindowControlArea::Close)
                .hover(move |s| s.bg(rgb(t.bg_tertiary)))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(t.text_primary))
                        .child("\u{2715}"),
                ),
        )
        .into_any_element()
}
