use gpui::{
    AnyElement, App, MouseButton, WeakEntity, Window, WindowControlArea, div, prelude::*, px, rgb,
    rgba, svg,
};

use crate::theme::{Theme, fade_in, lerp_rgb};
use crate::ui::shell::Shell;

pub const TITLE_BAR_HEIGHT: f32 = 36.0;

const MENU_ICON: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5">
		<path d="M20 7L4 7" />
		<path d="M20 12L4 12" />
		<path d="M20 17L4 17" />
	</g>
</svg>
"#;

const HOME_ICON: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-width="1.5">
		<path d="M2 12.2039C2 9.91549 2 8.77128 2.5192 7.82274C3.0384 6.87421 3.98695 6.28551 5.88403 5.10813L7.88403 3.86687C9.88939 2.62229 10.8921 2 12 2C13.1079 2 14.1106 2.62229 16.116 3.86687L18.116 5.10812C20.0131 6.28551 20.9616 6.87421 21.4808 7.82274C22 8.77128 22 9.91549 22 12.2039V13.725C22 17.6258 22 19.5763 20.8284 20.7881C19.6569 22 17.7712 22 14 22H10C6.22876 22 4.34315 22 3.17157 20.7881C2 19.5763 2 17.6258 2 13.725V12.2039Z" />
		<path stroke-linecap="round" d="M15 18H9" />
	</g>
</svg>
"#;

#[derive(IntoElement)]
pub struct TitleBar {
    pub menu_open: bool,
    pub icon_animation: f32,
    pub home_active: bool,
    pub shell: WeakEntity<Shell>,
}

impl RenderOnce for TitleBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);

        div()
            .id("title-bar")
            .relative()
            .w_full()
            .h(px(TITLE_BAR_HEIGHT))
            .flex()
            .flex_row()
            .items_stretch()
            .bg(rgb(t.bg_primary))
            .border_b_1()
            .border_color(rgb(t.component_border_color))
            // Clicking anywhere else on the title bar dismisses the menu.
            .on_mouse_down(MouseButton::Left, {
                let shell = self.shell.clone();
                move |_, _, cx| {
                    let _ = shell.update(cx, |shell, cx| shell.close_menu(cx));
                }
            })
            .child(
                // Buttons pinned exactly centered: (36 - 24) / 2 = 6px on
                // both sides, immune to the bottom border's layout box.
                div()
                    .absolute()
                    .top(px((TITLE_BAR_HEIGHT - 24.) / 2.))
                    .left(px(3.))
                    .flex()
                    .flex_row()
                    .child(render_menu_button(
                        self.menu_open,
                        self.icon_animation,
                        t,
                        self.shell.clone(),
                        cx,
                    ))
                    // Home button sits directly right of the menu button;
                    // active while the user is on the home screen.
                    .child(render_home_button(
                        self.home_active,
                        t,
                        self.shell.clone(),
                        cx,
                    )),
            )
            // Spacer — only this region is draggable so it doesn't
            // swallow the close / min / max hitboxes.
            .child(div().flex_1().window_control_area(WindowControlArea::Drag))
            .child(render_window_controls(t, self.shell.clone(), cx))
    }
}

fn render_home_button(active: bool, t: Theme, shell: WeakEntity<Shell>, cx: &App) -> AnyElement {
    // Hover fade: bg + border + shadow tween in together.
    let mut k = shell_fade(&shell, cx, "tb-home");
    if active {
        k = 1.0;
    }
    let idle_border = 0x00000000;
    let hover_bg = lerp_rgb(t.bg_primary, t.bg_tertiary, k);
    let active_bg = t.bg_tertiary;
    let bg = lerp_rgb(hover_bg, active_bg, if active { 1.0 } else { 0.0 });
    // Alpha-only fade: lerping RGB from black causes a dark flash.
    let border = fade_in((t.border_color << 8) | 0xFF, k);
    let mut shadow = t.shadow_sm();
    shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();
    // Icon + label brighten to text_primary on hover/active.
    let fg = lerp_rgb(t.text_secondary, t.text_primary, k);

    div()
        .id("home-button")
        .flex()
        .items_center()
        .gap(px(4.))
        .ml(px(2.))
        .h(px(24.))
        .px(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        // Constant geometry; only colors tween.
        .border_1()
        .border_color(rgba(border))
        .bg(rgb(bg))
        .shadow(vec![shadow])
        .on_hover({
            let shell = shell.clone();
            move |hovered, _, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.animate_fade("tb-home", if *hovered { 1.0 } else { 0.0 }, cx);
                });
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            let _ = shell.update(cx, |shell, cx| shell.go_home(cx));
        })
        .child(
            svg()
                .data(HOME_ICON)
                .w(px(14.))
                .h(px(14.))
                .text_color(rgb(fg)),
        )
        .child(div().text_xs().text_color(rgb(fg)).child("Home"))
        .into_any_element()
}

fn render_menu_button(
    menu_open: bool,
    animation: f32,
    t: Theme,
    shell: WeakEntity<Shell>,
    cx: &App,
) -> AnyElement {
    // Active fade follows the open/close animation; hover fade is a tween.
    let act = animation.clamp(0., 1.);
    let hov = shell_fade(&shell, cx, "tb-menu");
    let k = act.max(hov);
    let bg = lerp_rgb(t.bg_primary, t.bg_tertiary, k);
    let icon_color = lerp_rgb(t.text_secondary, t.text_primary, act);
    let border = fade_in((t.border_color << 8) | 0xFF, k);
    let mut shadow = t.shadow_sm();
    shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();

    div()
        .id("app-menu-button")
        .flex()
        .items_center()
        .justify_center()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(6.))
        .cursor_pointer()
        // Constant geometry; only colors tween — same contract as home.
        .border_1()
        .border_color(rgba(border))
        .bg(rgb(bg))
        .shadow(vec![shadow])
        .on_hover({
            let shell = shell.clone();
            move |hovered, _, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.animate_fade("tb-menu", if *hovered { 1.0 } else { 0.0 }, cx);
                });
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            let _ = shell.update(cx, |shell, cx| {
                shell.toggle_menu(cx);
                cx.stop_propagation();
            });
        })
        .child(
            svg()
                .data(MENU_ICON)
                .w(px(16.))
                .h(px(16.))
                .text_color(rgb(icon_color)),
        )
        .into_any_element()
}

fn lerp_color(from: u32, to: u32, k: f32) -> u32 {
    let ch = |c: u32, shift: u32| ((c >> shift) & 0xFF) as f32;
    let mix = |shift: u32| (ch(from, shift) + (ch(to, shift) - ch(from, shift)) * k).round() as u32;
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn render_window_controls(t: Theme, shell: WeakEntity<Shell>, cx: &App) -> AnyElement {
    let control =
        |id: &'static str, hover_bg: u32, content: gpui::AnyElement| -> gpui::AnyElement {
            let k = shell_fade(&shell, cx, id);
            let bg = lerp_rgb(t.bg_primary, hover_bg, k);
            let shell_c = shell.clone();
            div()
                .id(id)
                .w(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(match id {
                    "window-minimize" => WindowControlArea::Min,
                    "window-maximize" => WindowControlArea::Max,
                    _ => WindowControlArea::Close,
                })
                .bg(rgb(bg))
                .on_hover(move |hovered, _, cx| {
                    let _ = shell_c.update(cx, |shell, cx| {
                        shell.animate_fade(id, if *hovered { 1.0 } else { 0.0 }, cx);
                    });
                })
                .child(content)
                .into_any_element()
        };

    div()
        .flex()
        .flex_row()
        .items_stretch()
        .child(control(
            "window-minimize",
            t.bg_secondary,
            div()
                .w(px(10.))
                .h(px(1.))
                .bg(rgb(t.text_secondary))
                .into_any_element(),
        ))
        .child(control(
            "window-maximize",
            t.bg_secondary,
            div()
                .size(px(9.))
                .border_1()
                .border_color(rgb(t.text_secondary))
                .into_any_element(),
        ))
        .child(control(
            "window-close",
            t.bg_tertiary,
            div()
                .text_sm()
                .text_color(rgb(t.text_primary))
                .child("\u{2715}")
                .into_any_element(),
        ))
        .into_any_element()
}

// Reads a fade value from the shell without needing a full upgrade.
fn shell_fade(shell: &WeakEntity<Shell>, cx: &App, key: &str) -> f32 {
    shell.upgrade().map(|s| s.read(cx).fade(key)).unwrap_or(0.0)
}
