use gpui::{
    App, IntoElement, MouseButton, RenderOnce, SharedString, Window, div, prelude::*, px, rgb,
    rgba, svg,
};

use crate::editor::InteractionMode;
use crate::theme::{Theme, fade_in, lerp_rgb};
use crate::ui::inspector::INSPECTOR_WIDTH;

// Second topbar (Clay Phase 0): mode dropdown + redo region + conflict slot.
// In-flow row under the TitleBar; content stops at the inspector dock
// (absolute right overlay) via right padding. Canvas-width only, never under
// the inspector.

pub const MODEBAR_HEIGHT: f32 = 32.0;

const CHEVRON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m6 9l6 6l6-6" /></svg>"#;

// Layers-panel toggle: the ONLY sidebar control in the app (the panel
// itself carries no close button).
const ICON_SIDEBAR: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256">
	<path d="M0 0h256v256H0z" fill="none" />
	<path fill="currentColor" d="M216 42H40a14 14 0 0 0-14 14v144a14 14 0 0 0 14 14h176a14 14 0 0 0 14-14V56a14 14 0 0 0-14-14M38 200V56a2 2 0 0 1 2-2h42v148H40a2 2 0 0 1-2-2m180 0a2 2 0 0 1-2 2H94V54h122a2 2 0 0 1 2 2Z" />
</svg>
"#;

pub(crate) const ICON_OBJECT: &[u8] = br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
<g clip-path="url(#clip0_3_2)">
<path d="M9.25 4C9.25 4.67188 9.25 5.32812 9.25 6.07812C9.25 6.82812 9.25 7.42188 9.25 8M9.25 4C9.0175 4 8.9015 4 8.805 3.981C8.61092 3.94245 8.43264 3.84719 8.29273 3.70727C8.15281 3.56736 8.05755 3.38908 8.019 3.195C8 3.0985 8 2.9825 8 2.75M9.25 4C9.4825 4 9.5985 4 9.695 3.981C9.88908 3.94245 10.0674 3.84719 10.2073 3.70727C10.3472 3.56736 10.4424 3.38908 10.481 3.195C10.5 3.0985 10.5 2.9825 10.5 2.75C10.5 2.5175 10.5 2.4015 10.481 2.305C10.4424 2.11092 10.3472 1.93264 10.2073 1.79273C10.0674 1.65281 9.88908 1.55755 9.695 1.519C9.5985 1.5 9.4825 1.5 9.25 1.5C9.0175 1.5 8.9015 1.5 8.805 1.519C8.61092 1.55755 8.43264 1.65281 8.29273 1.79273C8.15281 1.93264 8.05755 2.11092 8.019 2.305C8 2.4015 8 2.5175 8 2.75M9.25 8C9.4825 8 9.5985 8 9.695 8.019C9.88908 8.05755 10.0674 8.15281 10.2073 8.29273C10.3472 8.43264 10.4424 8.61092 10.481 8.805C10.5 8.9015 10.5 9.0175 10.5 9.25C10.5 9.4825 10.5 9.5985 10.481 9.695C10.4424 9.88908 10.3472 10.0674 10.2073 10.2073C10.0674 10.3472 9.88908 10.4424 9.695 10.481C9.5985 10.5 9.4825 10.5 9.25 10.5C9.0175 10.5 8.9015 10.5 8.805 10.481C8.61092 10.4424 8.43264 10.3472 8.29273 10.2073C8.15281 10.0674 8.05755 9.88908 8.019 9.695C8 9.5985 8 9.4825 8 9.25M9.25 8C9.0175 8 8.9015 8 8.805 8.019C8.61092 8.05755 8.43264 8.15281 8.29273 8.29273C8.15281 8.43264 8.05755 8.61092 8.019 8.805C8 8.9015 8 9.0175 8 9.25M2.75 4C2.75 4.625 2.75 5.25 2.75 6C2.75 6.75 2.75 7.5 2.75 8M2.75 4C2.5175 4 2.4015 4 2.305 3.981C2.11092 3.94245 1.93264 3.84719 1.79273 3.70727C1.65281 3.56736 1.55755 3.38908 1.519 3.195C1.5 3.0985 1.5 2.9825 1.5 2.75C1.5 2.5175 1.5 2.4015 1.519 2.305C1.55755 2.11092 1.65281 1.93264 1.79273 1.79273C1.93264 1.65281 2.11092 1.55755 2.305 1.519C2.4015 1.5 2.5175 1.5 2.75 1.5C2.9825 1.5 3.0985 1.5 3.195 1.519C3.38908 1.55755 3.56736 1.65281 3.70727 1.79273C3.84719 1.93264 3.94245 2.11092 3.981 2.305C4 2.4015 4 2.5175 4 2.75M2.75 4C2.9825 4 3.0985 4 3.195 3.981C3.38908 3.94245 3.56736 3.84719 3.70727 3.70727C3.84719 3.56736 3.94245 3.38908 3.981 3.195C4 3.0985 4 2.9825 4 2.75M2.75 8C2.9825 8 3.0985 8 3.195 8.019C3.38908 8.05755 3.56736 8.15281 3.70727 8.29273C3.84719 8.43264 3.94245 8.61092 3.981 8.805C4 8.9015 4 9.0175 4 9.25M2.75 8C2.5175 8 2.4015 8 2.305 8.019C2.11092 8.05755 1.93264 8.15281 1.79273 8.29273C1.65281 8.43264 1.55755 8.61092 1.519 8.805C1.5 8.9015 1.5 9.0175 1.5 9.25C1.5 9.4825 1.5 9.5985 1.519 9.695C1.55755 9.88908 1.65281 10.0674 1.79273 10.2073C1.93264 10.3472 2.11092 10.4424 2.305 10.481C2.4015 10.5 2.5175 10.5 2.75 10.5C2.9825 10.5 3.0985 10.5 3.195 10.481C3.38908 10.4424 3.56736 10.3472 3.70727 10.2073C3.84719 10.0674 3.94245 9.88908 3.981 9.695C4 9.5985 4 9.4825 4 9.25M4 2.75C4.5 2.75 5.25 2.75 6 2.75C6.75 2.75 7.5 2.75 8 2.75M4 9.25C4.65625 9.25 5.25 9.25 6 9.25C6.75 9.25 7.40625 9.25 8 9.25" stroke="black" stroke-width="0.75" stroke-linecap="round" stroke-linejoin="round"/>
</g>
<defs>
<clipPath id="clip0_3_2">
<rect width="12" height="12" fill="white"/>
</clipPath>
</defs>
</svg>"#;

pub(crate) const ICON_EDIT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linejoin="round" stroke-width="1.5">
		<path d="m12.791 8.768l4.693 1.836c2.706 1.059 4.06 1.589 4.015 2.429s-1.457 1.225-4.282 1.995c-.841.23-1.262.344-1.553.636c-.292.291-.406.712-.636 1.553c-.77 2.825-1.155 4.237-1.995 4.282s-1.37-1.308-2.43-4.015L8.769 12.79c-1.11-2.834-1.664-4.25-.946-4.969c.718-.718 2.135-.163 4.969.946Z" />
		<path stroke-linecap="round" d="M4.5 2.5h-1a1 1 0 0 0-1 1v1a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-1a1 1 0 0 0-1-1m0 11h-1a1 1 0 0 0-1 1v1a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-1a1 1 0 0 0-1-1m11-11h-1a1 1 0 0 0-1 1v1a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-1a1 1 0 0 0-1-1m-2 1.5h-8M4 5.5v8" />
	</g>
</svg>
"#;

#[derive(IntoElement)]
pub struct ModeBar {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for ModeBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let mode = self
            .editor
            .upgrade()
            .map(|e| e.read(cx).interaction_mode)
            .unwrap_or(InteractionMode::Object);

        div()
            .id("mode-bar")
            .w_full()
            .h(px(MODEBAR_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            // Inspector is an absolute right dock: keep bar content clear.
            .pr(px(INSPECTOR_WIDTH + 8.))
            .bg(rgb(t.bg_primary))
            .border_b_1()
            .border_color(rgb(t.component_border_color))
            .child(self.sidebar_toggle(t, cx))
            .child(self.mode_dropdown(mode, t, cx))
            // Redo region (flex): last-action readout (label + key hints)
            // until Phase 3 puts live op params here. Collapses when empty.
            .child(self.redo_region(t, cx))
        // Conflict chip slot (Phase 4; hidden at zero).
    }
}

impl ModeBar {
    // Sidebar toggle: titlebar menu-button contract (constant 24px
    // geometry; bg + border + shadow tween on hover). Reads ACTIVE while
    // the panel is open so the button stays lit.
    fn sidebar_toggle(&self, t: Theme, cx: &App) -> impl IntoElement {
        let shell = self.shell.clone();
        let shell_toggle = self.shell.clone();
        let open = shell
            .upgrade()
            .map(|s| s.read(cx).layers_panel_open)
            .unwrap_or(false);
        let hov = shell
            .upgrade()
            .map(|s| s.read(cx).fade("modebar-layers"))
            .unwrap_or(0.0);
        let k = if open { 1.0 } else { hov };
        let bg = lerp_rgb(t.bg_primary, t.bg_tertiary, k);
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        let mut shadow = t.shadow_sm();
        shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();
        let fg = lerp_rgb(t.text_secondary, t.text_primary, k);

        div()
            .id("sidebar-toggle")
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.))
            .h(px(24.))
            .rounded(px(6.))
            .cursor_pointer()
            .border_1()
            .border_color(rgba(border))
            .bg(rgb(bg))
            .shadow(vec![shadow])
            .on_hover(move |hovered, _, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.animate_fade(
                        "modebar-layers",
                        if *hovered { 1.0 } else { 0.0 },
                        cx,
                    );
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = shell_toggle.update(cx, |shell, cx| shell.toggle_layers_panel(cx));
            })
            .child(
                svg()
                    .data(ICON_SIDEBAR)
                    .w(px(15.))
                    .h(px(15.))
                    .text_color(rgb(fg)),
            )
    }

    fn redo_region(&self, t: Theme, cx: &App) -> impl IntoElement {
        let status = self
            .shell
            .upgrade()
            .and_then(|s| {
                s.read(cx)
                    .editor
                    .as_ref()
                    .map(|ed| ed.read(cx).clay_status.clone())
            })
            .flatten();
        match status {
            Some((label, hint)) => div()
                .flex_1()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .overflow_hidden()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(t.text_secondary))
                        .child(hint),
                )
                .into_any_element(),
            None => div().flex_1().into_any_element(),
        }
    }
}

impl ModeBar {
    fn mode_dropdown(&self, mode: InteractionMode, t: Theme, cx: &App) -> impl IntoElement {
        let shell = self.shell.clone();
        let shell_toggle = self.shell.clone();

        // Mode icon per current mode (no dots).
        let (icon, label) = match mode {
            InteractionMode::Object => (ICON_OBJECT, "Object Mode"),
            // Active-object name lands with object identity (Phase 1+):
            // `Edit Mode · Path 12`.
            InteractionMode::Edit => (ICON_EDIT, "Edit Mode"),
        };

        // Home-button contract (title_bar.rs): bg + shadow tween on hover;
        // the border stays put 24/7 (no fade, no shift). Hover lands on
        // bg_tertiary (matches the canvas Edit pill).
        let k = shell
            .upgrade()
            .map(|s| s.read(cx).fade("modebar-dropdown"))
            .unwrap_or(0.0);
        let bg = lerp_rgb(t.bg_primary, t.bg_tertiary, k);
        let mut shadow = t.shadow_sm();
        shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();
        let fg = lerp_rgb(t.text_secondary, t.text_primary, k);

        div().relative().child(
            div()
                .id("mode-dropdown")
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(24.))
                .px(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                // Constant geometry AND border; only bg + fg tween.
                .border_1()
                .border_color(rgb(t.border_color))
                .bg(rgb(bg))
                .text_sm()
                .text_color(rgb(fg))
                .on_hover(move |hovered, _, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.animate_fade(
                            "modebar-dropdown",
                            if *hovered { 1.0 } else { 0.0 },
                            cx,
                        );
                    });
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = shell_toggle.update(cx, |shell, cx| {
                        shell.mode_dropdown_open = !shell.mode_dropdown_open;
                        cx.notify();
                    });
                })
                .child(svg().data(icon).w(px(14.)).h(px(14.)).text_color(rgb(fg)))
                .child(label)
                .child(
                    svg()
                        .data(CHEVRON)
                        .w(px(12.))
                        .h(px(12.))
                        .text_color(rgb(fg)),
                ),
        )
    }
}

// Mode popup as a top-of-tree overlay (rendered last, beside Toasts): tree
// order = paint order, so anything inside the ModeBar row would paint UNDER
// the toolbar rail and canvas siblings. Driven by the same shell state.
#[derive(IntoElement)]
pub struct ModeBarPopup {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for ModeBarPopup {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let open = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).mode_dropdown_open)
            .unwrap_or(false);
        if !open {
            return div().into_any_element();
        }
        let mode = self
            .editor
            .upgrade()
            .map(|e| e.read(cx).interaction_mode)
            .unwrap_or(InteractionMode::Object);
        div()
            .absolute()
            .top(px(MODEBAR_HEIGHT - 2.))
            .left(px(8.))
            .child(self.dropdown_popup(mode, t, cx))
            .into_any_element()
    }
}

impl ModeBarPopup {
    fn dropdown_popup(&self, mode: InteractionMode, t: Theme, cx: &App) -> impl IntoElement {
        div()
            .occlude()
            .w(px(160.))
            .flex()
            .flex_col()
            .p(px(4.))
            .gap_y(px(2.))
            .bg(rgb(t.bg_darker))
            .border_1()
            .border_color(rgb(t.menu_border_color))
            .rounded(px(12.))
            .shadow(vec![t.shadow_sm()])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.dropdown_option(InteractionMode::Object, "Object Mode", mode, t, cx))
            .child(self.dropdown_option(InteractionMode::Edit, "Edit Mode", mode, t, cx))
    }

    fn dropdown_option(
        &self,
        option: InteractionMode,
        label: &'static str,
        current: InteractionMode,
        t: Theme,
        cx: &App,
    ) -> impl IntoElement {
        let editor = self.editor.clone();
        let shell = self.shell.clone();
        let shell_click = self.shell.clone();
        let fade_key: &'static str = match option {
            InteractionMode::Object => "mode-option-object",
            InteractionMode::Edit => "mode-option-edit",
        };
        let is_current = option == current;
        let icon = match option {
            InteractionMode::Object => ICON_OBJECT,
            InteractionMode::Edit => ICON_EDIT,
        };

        // Submenu-entry contract (menu/dropdown.rs): hover tween blended
        // over the current-mode state.
        let hov = shell
            .upgrade()
            .map(|s| s.read(cx).fade(fade_key))
            .unwrap_or(0.0);
        let k = hov.max(if is_current { 1.0 } else { 0.0 });
        let bg = lerp_rgb(t.bg_darker, t.bg_secondary, k);
        // Alpha-only fade: lerping RGB from black causes a dark flash.
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        let mut shadow = t.shadow_sm();
        shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();

        div()
            .id(SharedString::from(format!("mode-option-{label}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .pl(px(8.))
            .pr(px(6.))
            .rounded(px(10.))
            .text_sm()
            .text_color(rgb(t.text_primary))
            .cursor_pointer()
            .bg(rgb(bg))
            .border_1()
            .border_color(rgba(border))
            .shadow(vec![shadow])
            .on_hover(move |hovered, _, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.animate_fade(fade_key, if *hovered { 1.0 } else { 0.0 }, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| {
                    if ed.set_interaction_mode(option) {
                        cx.notify();
                    }
                });
                let _ = shell_click.update(cx, |shell, cx| {
                    shell.mode_dropdown_open = false;
                    cx.notify();
                });
            })
            .child(
                svg()
                    .data(icon)
                    .w(px(14.))
                    .h(px(14.))
                    .text_color(rgb(t.text_primary)),
            )
            .child(label)
    }
}
