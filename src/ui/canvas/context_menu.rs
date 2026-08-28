use gpui::{
    App, AppContext, IntoElement, MouseButton, SharedString, WeakEntity, div, prelude::*, px, rgb,
    rgba, svg,
};

use crate::ui::shell::Shell;

// Canvas context menu: the one popup panel used for every in-canvas choice
// (snap-bond options now; right-click delete/copy/constraints later).
// Styled after the app dropdown panels. Entries carry a semantic action;
// the EDITOR decides what each action does — this module only renders.

pub const ENTRY_H: f32 = 26.;
pub const PANEL_W: f32 = 190.;
pub const ICON_SIZE: f32 = 15.;
const PANEL_PADDING_Y: f32 = 4.;
const PANEL_PADDING_X: f32 = 4.;
const ITEM_GAP: f32 = 2.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    // Snap-bond choices.
    BondCoincident,
    BondMerge,
}

#[derive(Clone)]
pub struct ContextMenuEntry {
    pub icon: &'static [u8],
    pub label: &'static str,
    pub shortcut: &'static str,
    pub action: ContextAction,
}

#[derive(Clone)]
pub struct ContextMenu {
    // Screen px anchor (canvas-local space).
    pub x: f32,
    pub y: f32,
    pub entries: Vec<ContextMenuEntry>,
}

impl ContextMenu {
    pub fn panel_height(&self) -> f32 {
        PANEL_PADDING_Y * 2.
            + self.entries.len() as f32 * ENTRY_H
            + self.entries.len().saturating_sub(1) as f32 * ITEM_GAP
    }

    /// Nudges the anchor so the whole panel stays on screen.
    pub fn clamp_to(&mut self, view_w: f32, view_h: f32) {
        const MARGIN: f32 = 8.;
        if self.x + PANEL_W > view_w - MARGIN {
            self.x = view_w - MARGIN - PANEL_W;
        }
        if self.y + self.panel_height() > view_h - MARGIN {
            self.y = view_h - MARGIN - self.panel_height();
        }
        self.x = self.x.max(MARGIN);
        self.y = self.y.max(MARGIN);
    }
}

// -- icons --

pub const ICON_COINCIDENT: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none">
  <circle cx="12" cy="5" r="2" fill="currentColor"/>
  <circle cx="12" cy="19" r="2" fill="currentColor"/>
  <path d="M12 7v10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
</svg>"#;

pub const ICON_MERGE_POINTS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256">
	<path d="M0 0h256v256H0z" fill="none" />
	<path fill="currentColor" d="M176 152h32a16 16 0 0 0 16-16v-32a16 16 0 0 0-16-16h-32a16 16 0 0 0-16 16v8H88V80h8a16 16 0 0 0 16-16V32a16 16 0 0 0-16-16H64a16 16 0 0 0-16 16v32a16 16 0 0 0 16 16h8v112a24 24 0 0 0 24 24h64v8a16 16 0 0 0 16 16h32a16 16 0 0 0 16-16v-32a16 16 0 0 0-16-16h-32a16 16 0 0 0-16 16v8H96a8 8 0 0 1-8-8v-64h72v8a16 16 0 0 0 16 16M64 32h32v32H64Zm112 160h32v32h-32Zm0-88h32v32h-32Z" />
</svg>"#;

/// Renders the open context menu, if any. Clicking an entry applies its
/// action through the editor; hover/pop fades ride the *Editor*'s tween
/// system (not Shell's) so reading them during Shell::render doesn't
/// re-entrantly borrow Shell.
pub fn draw(
    editor: WeakEntity<crate::editor::Editor>,
    _shell: WeakEntity<Shell>,
    cx: &mut App,
) -> Option<impl IntoElement> {
    use crate::theme::{fade_in, lerp_rgb};

    let t = *crate::theme::active(cx);
    let editor_up = editor.upgrade()?;
    let menu = editor_up.read(cx).context_menu.clone()?;
    let entries = menu.entries.clone();
    let pop = {
        let p = editor_up.read(cx).context_menu_pop;
        if p < 0.999 {
            let _ = editor.update(cx, |ed, cx| ed.animate_context_menu_pop(1.0, cx));
        }
        p.max(0.01)
    };

    let mut panel = div()
        .occlude()
        .absolute()
        .left(px(menu.x))
        .top(px(menu.y - 8. * (1. - pop)))
        .opacity(pop)
        .w(px(PANEL_W))
        .flex()
        .flex_col()
        .px(px(PANEL_PADDING_X))
        .py(px(PANEL_PADDING_Y))
        .gap_y(px(ITEM_GAP))
        .bg(rgb(t.bg_darker))
        .border_1()
        .border_color(rgb(t.menu_border_color))
        .rounded(px(8.))
        .shadow(vec![t.shadow_sm()])
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

    for (i, entry) in entries.into_iter().enumerate() {
        let k = editor_up
            .read(cx)
            .context_menu_fade(&format!("ctx-entry-{i}"));
        let bg = lerp_rgb(t.bg_darker, t.bg_tertiary, k);
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        let ed_click = editor.clone();
        let ed_hover = editor.clone();
        let hover_key = format!("ctx-entry-{i}");

        panel = panel.child(
            div()
                .id(SharedString::from(format!("ctx-entry-{i}")))
                .flex()
                .items_center()
                .justify_between()
                .h(px(ENTRY_H))
                .px(px(4.))
                .rounded(px(6.))
                .text_sm()
                .text_color(rgb(t.text_primary))
                .cursor_pointer()
                .bg(rgb(bg))
                .border_1()
                .border_color(rgba(border))
                .on_hover(move |hovered, _, cx| {
                    if let Some(ed) = ed_hover.upgrade() {
                        let _ = ed.update(cx, |ed, cx| {
                            ed.animate_context_menu_fade(
                                &hover_key,
                                if *hovered { 1.0 } else { 0.0 },
                                cx,
                            )
                        });
                    }
                })
                .on_mouse_down(MouseButton::Left, move |_: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let _ = ed_click.update(cx, |ed, _| ed.apply_context_action(entry.action));
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div().flex().items_center().mt(px(-1.)).gap(px(2.)).child(
                                svg()
                                    .data(entry.icon)
                                    .w(px(ICON_SIZE))
                                    .h(px(ICON_SIZE))
                                    .text_color(rgb(t.text_primary)),
                            ),
                        )
                        .child(entry.label),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family(crate::theme::FONT_UI)
                        .text_color(rgb(t.empty_text_primary))
                        .child(entry.shortcut),
                ),
        );
    }
    Some(panel)
}
