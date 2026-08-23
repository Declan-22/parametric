use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, rgba, svg};

use crate::editor::Tool;
use crate::theme::Theme;

// Bottom toolbar: one centered row — mode tools (Move / Pan), a divider,
// then shape tools (Rectangle / Ellipse).

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

const ICON_RECTANGLE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="currentColor" d="M3 19V5h18v14zm1-1h16V6H4zm0 0V6z" />
</svg>
"#;

const ICON_ELLIPSE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 2048 2048">
	<path d="M0 0h2048v2048H0z" fill="none" />
	<path fill="currentColor" d="M1024 256q131 0 268 27t264 85t233 144t175 206q41 71 62 147t22 159q0 82-21 158t-63 148q-68 119-174 206t-233 144t-264 84t-269 28q-131 0-268-27t-264-85t-233-144t-175-206q-41-71-62-147T0 1024q0-82 21-158t63-148q68-119 174-206t233-144t264-84t269-28m0 1408q84 0 169-11t167-36t159-60t146-87q54-40 101-88t81-105t53-120t20-133q0-70-19-133t-54-119t-81-105t-101-89q-68-50-145-86t-160-61t-167-35t-169-12q-84 0-169 11t-167 36t-159 60t-146 87q-54 40-101 88t-81 105t-53 120t-20 133q0 70 19 133t54 119t81 105t101 89q68 50 145 86t160 61t167 35t169 12" />
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
            // Centered row pinned near the bottom; width fits the buttons.
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(12.))
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .p(px(3.))
                    .rounded(px(8.))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.component_border_color))
                    .shadow(vec![t.shadow_sm()])
                    .child(self.tool_button(Tool::Move, ICON_MOVE, active_tool, t, cx))
                    .child(self.tool_button(Tool::Pan, ICON_PAN, active_tool, t, cx))
                    .child(divider(t))
                    .child(self.tool_button(Tool::Rectangle, ICON_RECTANGLE, active_tool, t, cx))
                    .child(self.tool_button(Tool::Ellipse, ICON_ELLIPSE, active_tool, t, cx)),
            )
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
        // Hover tween: idle bg_secondary -> hover bg_primary + shadow.
        let key = format!("tb-{}", tool_debug_name(tool));
        let k = if is_active {
            1.0
        } else {
            self.shell
                .upgrade()
                .map(|s| s.read(cx).fade(&key))
                .unwrap_or(0.0)
        };
        let bg = lerp_rgb(t.bg_secondary, t.bg_primary, k);
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        let mut shadow = t.shadow_sm();
        shadow.color = gpui::rgba(fade_in(t.item_shadow_color, k)).into();
        let icon_color = lerp_rgb(t.text_secondary, t.text_primary, k.max(if is_active { 1.0 } else { 0.0 }));

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
            .bg(rgb(bg))
            .border_1()
            .border_color(rgba(border))
            .shadow(vec![shadow])
            .on_hover(move |hovered, _, cx| {
                let _ = shell_hover.update(cx, |shell, cx| {
                    shell.animate_fade(&key, if *hovered { 1.0 } else { 0.0 }, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let _ = editor.update(cx, |ed, cx| {
                    if ed.set_tool(tool) {
                        cx.notify();
                    }
                });
            })
            .child(svg().data(icon).w(px(15.)).h(px(15.)).text_color(rgb(icon_color)))
    }
}

fn divider(t: Theme) -> impl IntoElement {
    div().w(px(1.)).h(px(18.)).mx(px(3.)).bg(rgb(t.component_border_color))
}

fn tool_debug_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Move => "tool-move",
        Tool::Pan => "tool-pan",
        Tool::Rectangle => "tool-rectangle",
        Tool::Ellipse => "tool-ellipse",
    }
}




