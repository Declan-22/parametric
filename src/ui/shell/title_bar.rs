use gpui::{
    AnyElement, App, MouseButton, WeakEntity, Window, WindowControlArea, div, prelude::*, px, rgb, rgba,
    svg,
};

use crate::theme::Theme;
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

#[derive(IntoElement)]
pub struct TitleBar {
    pub menu_open: bool,
    pub icon_animation: f32,
    pub shell: WeakEntity<Shell>,
}

impl RenderOnce for TitleBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);

        div()
            .id("title-bar")
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
                // Vertically centers the menu button within the title bar.
                div()
                    .flex()
                    .items_center()
                    .pl(px(3.))
                    .child(render_menu_button(
                        self.menu_open,
                        self.icon_animation,
                        t,
                        self.shell.clone(),
                    )),
            )
            // Spacer — only this region is draggable so it doesn't
            // swallow the close / min / max hitboxes.
            .child(div().flex_1().window_control_area(WindowControlArea::Drag))
            .child(render_window_controls(t))
    }
}

fn render_menu_button(menu_open: bool, animation: f32, t: Theme, shell: WeakEntity<Shell>) -> AnyElement {
    // Fade between idle and active styling as the menu opens / closes.
    let k = animation.clamp(0., 1.);
    let bg = lerp_color(t.bg_primary, t.bg_secondary, k);
    let icon_color = lerp_color(t.empty_text_primary, t.text_secondary, k);
    // Shadow fades in alongside the rest of the active styling.
    let mut shadow = t.shadow_sm();
    shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();

    div()
        .id("app-menu-button")
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.))
        .h(px(28.))
        .rounded(px(6.))
        .cursor_pointer()
        .bg(rgb(bg))
        .shadow(vec![shadow])
        .hover(move |s| {
            s.bg(rgb(t.bg_secondary)).shadow(vec![t.shadow_sm()])
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
                .w(px(20.))
                .h(px(20.))
                .text_color(rgb(icon_color)),
        )
        .into_any_element()
}

// Fades an RGBA color in from fully transparent, preserving its target alpha.
// GPUI colors are 0xRRGGBBAA — alpha lives in the low byte.
fn fade_in(color: u32, k: f32) -> u32 {
    let a = ((color & 0xFF) as f32 * k).round() as u32;
    (color & 0xFFFFFF00) | a
}

fn lerp_color(from: u32, to: u32, k: f32) -> u32 {
    let ch = |c: u32, shift: u32| ((c >> shift) & 0xFF) as f32;
    let mix = |shift: u32| (ch(from, shift) + (ch(to, shift) - ch(from, shift)) * k).round() as u32;
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
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
