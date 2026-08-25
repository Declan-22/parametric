use gpui::{
    App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, rgba, svg,
};

use crate::editor::Tool;
use crate::theme::Theme;

// Bottom toolbar: one centered row â€” mode tools (Move / Pan), a divider,
// then shape tools (Rectangle).

const ICON_MOVE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="none" stroke="currentColor" stroke-linejoin="round" stroke-width="1.5" d="m9.803 4.63l6.033 2.36c3.48 1.362 5.22 2.043 5.163 3.123c-.058 1.08-1.874 1.576-5.506 2.566c-1.081.295-1.622.442-1.997.817s-.522.916-.817 1.997c-.99 3.632-1.486 5.448-2.566 5.506s-1.76-1.683-3.122-5.163L4.63 9.803C3.204 6.159 2.49 4.338 3.414 3.414c.924-.923 2.745-.21 6.389 1.216Z" />
</svg>


"#;

const ICON_PAN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5">
		<path d="M4 10.059v3.424c0 1.853 0 2.78.221 3.536c.527 1.8 1.935 3.216 3.735 3.846c.6.176 1.196.363 2.344.532a5.8 5.8 0 0 0 2.014-.066c.303-.062.55-.115.758-.16c.49-.106.98-.233 1.43-.454c.508-.248.903-.506 1.475-.933c.342-.255.655-.566 1.28-1.188l3.247-3.23a1.68 1.68 0 0 0 0-2.384a1.7 1.7 0 0 0-2.396 0l-2.25 2.239v-5.162" />
		<path d="M12.893 7.852V5.95c0-.815.664-1.475 1.483-1.475c.818 0 1.482.66 1.482 1.475v4.424m-5.929-.319V3.95c0-.815.664-1.475 1.482-1.475c.819 0 1.482.66 1.482 1.475v6.109M6.964 7.32v2.739v-5.104a1.483 1.483 0 0 1 2.965 0v5.104M6.964 8.854V7.95c0-.815-.663-1.475-1.482-1.475C4.664 6.475 4 7.135 4 7.95v2.738" />
	</g>
</svg>



"#;

const ICON_LINE: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256">
	<path d="M0 0h256v256H0z" fill="none" />
	<path fill="currentColor" d="M213.23 42.77A30 30 0 0 0 167 80.54L80.54 167a30.07 30.07 0 0 0-37.77 3.81A30 30 0 1 0 89 175.46L175.46 89a30 30 0 0 0 37.77-46.25Zm-136.51 162a18 18 0 1 1 0-25.46a18 18 0 0 1 0 25.43Zm128-128a18 18 0 0 1-25.46 0a18 18 0 1 1 25.46 0" />
</svg>


"#;

const ICON_RECTANGLE: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M5.061 20.045C6.375 21 8.251 21 12 21s5.625 0 6.939-.955a5 5 0 0 0 1.106-1.106C21 17.625 21 15.749 21 12s0-5.625-.955-6.939a5 5 0 0 0-1.106-1.106C17.625 3 15.749 3 12 3s-5.625 0-6.939.955A5 5 0 0 0 3.955 5.06C3 6.375 3 8.251 3 12s0 5.625.955 6.939a5 5 0 0 0 1.106 1.106M17 18l1-1m-5 1l5-5m-9 5l9-9" />
    </svg>


"#;

const ICON_RULER: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5">
		<path d="m17.5 10.5l2 2M14 14l2 2m-5.5 1.5l2 2" />
		<path stroke-linejoin="round" d="M10.536 4.678c1.364-1.365 2.047-2.047 2.808-2.363a4.14 4.14 0 0 1 3.17 0c.761.316 1.444.998 2.808 2.363c1.365 1.364 2.047 2.047 2.363 2.808a4.14 4.14 0 0 1 0 3.17c-.316.761-.998 1.444-2.363 2.808l-5.857 5.858c-1.365 1.365-2.048 2.047-2.809 2.363a4.14 4.14 0 0 1-3.17 0c-.761-.316-1.444-.998-2.808-2.363c-1.365-1.364-2.047-2.047-2.363-2.808a4.14 4.14 0 0 1 0-3.17c.316-.761.998-1.444 2.363-2.808z" />
	</g>
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
            .py(px(4.))
            .px(px(4.))
            .gap(px(2.))
            .bg(rgb(t.bg_primary))
            // Right-edge border separates the rail from the canvas.
            .border_r_1()
            .border_color(rgb(t.component_border_color))
            .child(self.tool_button(Tool::Move, ICON_MOVE, active_tool, t, cx))
            .child(self.tool_button(Tool::Pan, ICON_PAN, active_tool, t, cx))
            .child(divider(t))
            .child(self.tool_button(Tool::Ruler, ICON_RULER, active_tool, t, cx))
            .child(divider(t))
            .child(self.tool_button(Tool::Line, ICON_LINE, active_tool, t, cx))
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
            .w(px(34.))
            .h(px(34.))
            .rounded(px(8.))
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
                    .w(px(21.))
                    .h(px(21.))
                    .text_color(rgb(icon_color)),
            )
    }
}

fn divider(t: Theme) -> impl IntoElement {
    // Horizontal divider for the vertical rail.
    div()
        .h(px(2.))
        .w(px(20.))
        .my(px(2.))
        .bg(rgb(t.border_color))
}

fn tool_debug_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Move => "tool-move",
        Tool::Pan => "tool-pan",
        Tool::Line => "tool-line",
        Tool::Rectangle => "tool-rectangle",
        Tool::Ruler => "tool-ruler",
    }
}
