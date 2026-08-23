use gpui::{
    App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, rgba, svg,
};

use crate::editor::Tool;
use crate::theme::Theme;

// Bottom toolbar: one centered row â€” mode tools (Move / Pan), a divider,
// then shape tools (Rectangle).

const ICON_MOVE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M11 21L4 4l17 7l-6.265 2.685a2 2 0 0 0-1.05 1.05z" />
</svg>
"#;

const ICON_PAN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 16 16">
	<path d="M0 0h16v16H0z" fill="none" />
	<path fill="currentColor" d="M13.5 2.4c-.4-.4-1-.5-1.5-.3c0-.3-.1-.6-.4-.9c-.2-.2-.6-.4-1.1-.4c-.3 0-.5.1-.7.1c0-.2-.1-.3-.2-.5c-.5-.6-1.5-.6-2 0c-.2.2-.4.4-.4.6C7 1 6.8.9 6.6.9c-.5 0-.8.2-1.1.5C5 1.9 5 2.7 5 2.7v3.8c-.3-.3-.8-.8-1.5-.8c-.2 0-.5.1-.7.2c-.4.2-.6.5-.7.9c-.3 1 .6 2.4.6 2.5c.1.1 1.2 2.7 2.2 3.8C5.9 14.3 7 15 9.8 15c2.9 0 4.2-1.6 4.2-5.1V4.4c0-.1.1-1.3-.5-2M8 2c0-.3-.1-1 .5-1c.5 0 .5.5.5 1v4c0 .3.2.5.5.5s.5-.2.5-.5V2.2s0-.4.5-.4c.6 0 .5.9.5.9V6c0 .3.2.5.5.5s.5-.2.5-.5V3.6c0-.1 0-.6.5-.6s.5 1 .5 1v5.9c0 3.4-1.3 4.1-3.2 4.1c-2.4 0-3.3-.5-4.1-1.6c-.9-1-2.1-3.6-2.1-3.7c-.3-.3-.7-1.2-.6-1.6c0-.1.1-.2.2-.3c.1 0 .2-.1.2-.1c.4 0 .8.5.9.7l.6.9c.1.2.4.3.6.2c.4 0 .5-.2.5-.4V2.9c0-.4 0-1 .5-1c.4 0 .5.3.5.8V6c0 .3.2.5.5.5S8 6.3 8 6z" />
</svg>
"#;

const ICON_RECTANGLE: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="currentColor" d="M3 19V5h18v14zm1-1h16V6H4zm0 0V6z" />
</svg>
"#;

#[derive(IntoElement)]
pub struct Toolbar {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for Toolbar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let active_tool = self
            .editor
            .upgrade()
            .map(|e| e.read(cx).tool)
            .unwrap_or(Tool::Move);

        div()
            // Single column, flush against the far-left edge, full height
            // of the canvas area.
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .flex()
            .flex_col()
            .items_center()
            .py(px(6.))
            .px(px(4.))
            .gap(px(2.))
            .bg(rgb(t.bg_primary))
            // Right-edge border separates the rail from the canvas.
            .border_r_1()
            .border_color(rgb(t.component_border_color))
            .child(self.tool_button(Tool::Move, ICON_MOVE, active_tool, t, cx))
            .child(self.tool_button(Tool::Pan, ICON_PAN, active_tool, t, cx))
            .child(divider(t))
            .child(self.tool_button(Tool::Rectangle, ICON_RECTANGLE, active_tool, t, cx))
    }
}

impl Toolbar {
    fn tool_button(
        &self,
        tool: Tool,
        icon: &'static [u8],
        active_tool: Tool,
        t: Theme,
        cx: &gpui::App,
    ) -> impl IntoElement {
        use crate::theme::{fade_in, lerp_rgb};

        let editor = self.editor.clone();
        let is_active = active_tool == tool;
        let key = format!("tb-{}", tool_debug_name(tool));
        let k = if is_active {
            1.0
        } else {
            self.shell
                .upgrade()
                .map(|s| s.read(cx).fade(&key))
                .unwrap_or(0.0)
        };

        // Hover: plain bg fade to bg_secondary. Active: identical to the
        // home button — bg_tertiary + border + shadow_sm. Invisible border
        // when idle so nothing shifts.
        let bg = lerp_rgb(t.bg_primary, t.bg_secondary, k);
        let active_bg = t.bg_tertiary;
        let bg = if is_active { active_bg } else { bg };
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        // Shadow belongs to the active state only (home-button contract).
        let mut shadow = t.shadow_sm();
        if !is_active {
            shadow.color = gpui::rgba(0x00000000).into();
        }
        let icon_color = lerp_rgb(
            t.text_secondary,
            t.text_primary,
            k.max(if is_active { 1.0 } else { 0.0 }),
        );

        let shell_hover = self.shell.clone();
        div()
            .id(tool_debug_name(tool))
            .w(px(30.))
            .h(px(30.))
            .rounded(px(6.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            // Constant geometry; only colors tween.
            .border_1()
            .border_color(rgba(border))
            .bg(rgb(bg))
            .shadow(vec![shadow])
            .on_hover(move |hovered, _, cx| {
                let _ = shell_hover.update(cx, |shell, cx| {
                    shell.animate_fade(&key, if *hovered { 1.0 } else { 0.0 }, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_: &gpui::MouseDownEvent, _, cx| {
                // Don't let tool clicks leak into the canvas beneath the
                // rail (they'd register as canvas clicks).
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| {
                    if ed.set_tool(tool) {
                        cx.notify();
                    }
                });
            })
            .child(
                svg()
                    .data(icon)
                    .w(px(17.))
                    .h(px(17.))
                    .text_color(rgb(icon_color)),
            )
    }
}

fn divider(t: Theme) -> impl IntoElement {
    // Horizontal divider for the vertical rail.
    div()
        .h(px(2.))
        .w(px(18.))
        .my(px(3.))
        .bg(rgb(t.border_color))
}

fn tool_debug_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Move => "tool-move",
        Tool::Pan => "tool-pan",
        Tool::Rectangle => "tool-rectangle",
    }
}
