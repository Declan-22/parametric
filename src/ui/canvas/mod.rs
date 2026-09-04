use gpui::{
    App, Bounds, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, RenderOnce, ScrollDelta, ScrollWheelEvent, Size, Window, canvas,
    div, fill, prelude::*, px, rgb, rgba, svg,
};

use crate::editor::Editor;
use crate::ui::shell::title_bar::TITLE_BAR_HEIGHT;

pub mod context_menu;
pub mod paint;

// Canvas view: renders the document through the editor camera and owns the
// viewport interactions (pan, zoom). Stateless — all state lives on Editor.

#[derive(IntoElement)]
pub struct CanvasView {
    pub editor: gpui::WeakEntity<Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
    pub focus: gpui::FocusHandle,
}

impl RenderOnce for CanvasView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Dimension geometry: computed once here, consumed by both the
        // painted lines and the label overlays.
        if let Some(editor) = self.editor.upgrade() {
            let _ = editor.update(cx, |ed, _| ed.update_dim_geom());
        }
        let editor_mods = self.editor.clone();
        let editor_move = self.editor.clone();
        let editor_down_l = self.editor.clone();
        let editor_down_m = self.editor.clone();
        let editor_up_l = self.editor.clone();
        let editor_up_m = self.editor.clone();
        let editor_scroll = self.editor.clone();
        let editor_hover_out = self.editor.clone();
        let focus_l = self.focus.clone();
        let focus_m = self.focus.clone();

        div()
            .id("canvas")
            .relative()
            .flex()
            .flex_1()
            .w_full()
            .h_full()
            // Hold focus while interacting so modifier changes reach the
            // canvas even when the mouse is still.
            .track_focus(&self.focus)
            .on_modifiers_changed(move |e: &gpui::ModifiersChangedEvent, _, cx| {
                let shift = e.modifiers.shift;
                let _ = editor_mods.update(cx, |ed, cx| {
                    if ed.alt_down != e.modifiers.alt || ed.shift != shift {
                        ed.alt_down = e.modifiers.alt;
                        ed.shift = shift;
                        if let Some(c) = ed.last_cursor {
                            ed.canvas_drag(c, shift);
                            cx.notify();
                        }
                    }
                });
            })
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
                window.focus(&focus_l, cx);
                let _ = editor_down_l.update(cx, |ed, cx| {
                    if ed.canvas_down(
                        MouseButton::Left,
                        canvas_pos(e.position),
                        e.modifiers.shift,
                        e.click_count,
                    ) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_down(
                MouseButton::Middle,
                move |e: &MouseDownEvent, window, cx| {
                    window.focus(&focus_m, cx);
                    let _ = editor_down_m.update(cx, |ed, cx| {
                        if ed.canvas_down(MouseButton::Middle, canvas_pos(e.position), false, 1) {
                            cx.notify();
                        }
                    });
                },
            )
            .on_hover(move |hovered, _, cx| {
                if !*hovered {
                    let _ = editor_hover_out.update(cx, |ed, cx| {
                        let mut changed = ed.clear_arc_reveal();
                        if ed.hover.take().is_some() {
                            changed = true;
                        }
                        if !ed.snap_guides.is_empty() {
                            ed.snap_guides.clear();
                            changed = true;
                        }
                        if changed {
                            cx.notify();
                        }
                    });
                }
            })
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                let shift = e.modifiers.shift;
                let _ = editor_move.update(cx, |ed, cx| {
                    ed.alt_down = e.modifiers.alt;
                    ed.update_dim_geom();
                    let mut changed = false;
                    // While idle, track which resize handle is under the
                    // cursor (used for cursor styling).
                    if ed.is_idle() {
                        changed |= ed.canvas_hover(canvas_pos(e.position));
                    }
                    changed |= ed.canvas_drag(canvas_pos(e.position), shift);
                    // Arc center reveal follows the raw cursor even when
                    // nothing else changed — otherwise the dot sticks
                    // around after the mouse leaves the disk.
                    changed |= ed.update_arc_reveal(ed.cursor_doc(canvas_pos(e.position)));
                    if changed {
                        cx.notify();
                    }
                });
            })
            .on_scroll_wheel(move |e: &ScrollWheelEvent, _, cx| {
                let amount = match e.delta {
                    ScrollDelta::Pixels(p) => p.y.as_f32(),
                    ScrollDelta::Lines(l) => l.y * 16.,
                };
                let _ = editor_scroll.update(cx, |ed, cx| {
                    ed.zoom_at(canvas_pos(e.position), amount);
                    cx.notify();
                });
            })
            .on_mouse_up(MouseButton::Left, move |e: &MouseUpEvent, _, cx| {
                let _ = editor_up_l.update(cx, |ed, cx| {
                    if ed.canvas_up(MouseButton::Left, e.modifiers.shift) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Middle, move |_: &MouseUpEvent, _, cx| {
                let _ = editor_up_m.update(cx, |ed, cx| {
                    if ed.canvas_up(MouseButton::Middle, false) {
                        cx.notify();
                    }
                });
            })
            .child(self.paint_layer())
            .child(self.constraint_chip_layer(cx))
            .child(self.dimension_layer())
            .child(self.snap_cursor_layer(cx))
            .children(context_menu::draw(
                self.editor.clone(),
                self.shell.clone(),
                cx,
            ))
    }
}

pub(crate) const CHIP_SIZE: f32 = 18.;

// All editor-facing canvas coordinates are CANVAS-LOCAL: the origin is the
// canvas element's top-left (which sits below the title bar). gpui delivers
// mouse events in WINDOW coords — convert once here at the boundary, so the
// editor, the paint callbacks (which add bounds.origin) and the DOM overlay
// layers all share one space and painted geometry lines up with the cursor.
fn canvas_pos(p: gpui::Point<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
    gpui::point(p.x, p.y - px(TITLE_BAR_HEIGHT))
}

// Constraint chip glyphs (12x12 source SVGs, rendered at the full chip
// size so the glyph fills it). Stroke width is bumped from the source
// files' 0.75 to 1 to keep the glyph readable at 18px.
pub(crate) const ICON_CHIP_COINCIDENT: &[u8] =
    br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg"><g clip-path="url(#clip0_5_128)">
<path d="M2.75 1V7.06M5 9.25H11M2.305 10.481C2.4015 10.5 2.5175 10.5 2.75 10.5C2.9825 10.5 3.0985 10.5 3.195 10.481C3.38908 10.4424 3.56736 10.3472 3.70727 10.2073C3.84719 10.0674 3.94245 9.88908 3.981 9.695C4 9.5985 4 9.4825 4 9.25C4 9.0175 4 8.9015 3.981 8.805C3.94245 8.61092 3.84719 8.43264 3.70727 8.29273C3.56736 8.15281 3.38908 8.05755 3.195 8.019C3.0985 8 2.9825 8 2.75 8C2.5175 8 2.4015 8 2.305 8.019C2.11092 8.05755 1.93264 8.15281 1.79273 8.29273C1.65281 8.43264 1.55755 8.61092 1.519 8.805C1.5 8.9015 1.5 9.0175 1.5 9.25C1.5 9.4825 1.5 9.5985 1.519 9.695C1.55755 9.88908 1.65281 10.0674 1.79273 10.2073C1.93264 10.3472 2.11092 10.4424 2.305 10.481Z" stroke="black" stroke-width="0.75" stroke-linecap="round" stroke-linejoin="round"/>
</g>
<defs>
<clipPath id="clip0_5_128">
<rect width="12" height="12" fill="white"/>
</clipPath>
</defs>
</svg>"#;

pub(crate) const ICON_CHIP_HORIZONTAL: &[u8] =
    br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
<g clip-path="url(#clip0_5_124)">
<path d="M1.375 4.31H10.625M1.5 6.185H10.5M4 6.3921L2.70711 7.685M6.75 6.3921L5.45711 7.685M9.25 6.3921L7.95711 7.685" stroke="black" stroke-width="0.75" stroke-linecap="round"/>
</g>
<defs>
<clipPath id="clip0_5_124">
<rect width="12" height="12" fill="white"/>
</clipPath>
</defs>
</svg>"#;

pub(crate) const ICON_CHIP_VERTICAL: &[u8] =
    br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
<g clip-path="url(#clip0_5_111)">
<path d="M7.685 1.375V10.625M5.81 1.5V10.5M5.60289 4L4.31 2.70711M5.60289 6.75L4.31 5.45711M5.60289 9.25L4.31 7.95711" stroke="black" stroke-width="0.75" stroke-linecap="round"/>
</g>
<defs>
<clipPath id="clip0_5_111">
<rect width="12" height="12" fill="white"/>
</clipPath>
</defs>
</svg>"#;

pub(crate) const ICON_CHIP_TANGENT: &[u8] = br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
<g clip-path="url(#clip0_9_8)">
<path d="M9.0777 7.93289L8.70989 7.85983L8.70976 7.86045L9.0777 7.93289ZM9.0625 8.28889L8.6875 8.28889L9.0625 8.28889ZM9.0777 8.64489L8.70976 8.71734L8.70989 8.71796L9.0777 8.64489ZM9.7065 9.27369L9.63344 9.64151L9.63406 9.64163L9.7065 9.27369ZM10.4185 9.27369L10.4909 9.64163L10.4916 9.64151L10.4185 9.27369ZM10.8283 9.05471L10.5632 8.78955V8.78955L10.8283 9.05471ZM11.0473 8.64489L11.4151 8.71796L11.4152 8.71734L11.0473 8.64489ZM11.0625 8.28889L11.4375 8.28889L11.0625 8.28889ZM11.0473 7.93289L11.4152 7.86045L11.4151 7.85983L11.0473 7.93289ZM10.8283 7.52308L10.5632 7.78824L10.5632 7.78824L10.8283 7.52308ZM10.4185 7.30409L10.4916 6.93628L10.4909 6.93616L10.4185 7.30409ZM9.7065 7.30409L9.63406 6.93616L9.63344 6.93628L9.7065 7.30409ZM4.32832 2.55471L4.06315 2.28955V2.28955L4.32832 2.55471ZM2.5777 1.43289L2.20989 1.35983L2.20977 1.36045L2.5777 1.43289ZM2.5625 1.78889L2.1875 1.78889L2.5625 1.78889ZM2.5777 2.14489L2.20977 2.21734L2.20989 2.21796L2.5777 2.14489ZM3.2065 2.77369L3.13344 3.14151L3.13406 3.14163L3.2065 2.77369ZM3.9185 2.77369L3.99095 3.14163L3.99157 3.14151L3.9185 2.77369ZM4.5473 2.14489L4.91512 2.21796L4.91524 2.21734L4.5473 2.14489ZM4.5625 1.78889L4.9375 1.78889L4.5625 1.78889ZM4.5473 1.43289L4.91524 1.36045L4.91512 1.35983L4.5473 1.43289ZM4.32832 1.02308L4.06316 1.28824V1.28824L4.32832 1.02308ZM3.9185 0.804094L3.99157 0.436281L3.99095 0.436158L3.9185 0.804094ZM3.2065 0.804094L3.13406 0.436158L3.13344 0.436281L3.2065 0.804094ZM9.0777 7.93289L8.70976 7.86045C8.68572 7.98258 8.6875 8.12071 8.6875 8.28889L9.0625 8.28889L9.4375 8.28889C9.4375 8.08508 9.43928 8.03761 9.44564 8.00534L9.0777 7.93289ZM9.0625 8.28889L8.6875 8.28889C8.6875 8.45708 8.68572 8.59521 8.70976 8.71734L9.0777 8.64489L9.44564 8.57245C9.43928 8.54018 9.4375 8.49271 9.4375 8.28889L9.0625 8.28889ZM9.0777 8.64489L8.70989 8.71796C8.75518 8.946 8.86712 9.15548 9.03152 9.31988L9.29668 9.05471L9.56185 8.78955C9.50238 8.73008 9.4619 8.65431 9.44551 8.57183L9.0777 8.64489ZM9.29668 9.05471L9.03152 9.31988C9.19592 9.48428 9.40539 9.59621 9.63344 9.64151L9.7065 9.27369L9.77956 8.90588C9.69708 8.8895 9.62131 8.84901 9.56185 8.78955L9.29668 9.05471ZM9.7065 9.27369L9.63406 9.64163C9.75618 9.66568 9.89432 9.66389 10.0625 9.66389L10.0625 9.28889L10.0625 8.91389C9.85868 8.91389 9.81122 8.91211 9.77894 8.90576L9.7065 9.27369ZM10.0625 9.28889L10.0625 9.66389C10.2307 9.66389 10.3688 9.66568 10.4909 9.64163L10.4185 9.27369L10.3461 8.90576C10.3138 8.91211 10.2663 8.91389 10.0625 8.91389L10.0625 9.28889ZM10.4185 9.27369L10.4916 9.64151C10.7196 9.59621 10.9291 9.48428 11.0935 9.31988L10.8283 9.05471L10.5632 8.78955C10.5037 8.84901 10.4279 8.8895 10.3454 8.90588L10.4185 9.27369ZM10.8283 9.05471L11.0935 9.31988C11.2579 9.15548 11.3698 8.946 11.4151 8.71796L11.0473 8.64489L10.6795 8.57183C10.6631 8.65431 10.6226 8.73008 10.5632 8.78955L10.8283 9.05471ZM11.0473 8.64489L11.4152 8.71734C11.4393 8.59521 11.4375 8.45708 11.4375 8.28889L11.0625 8.28889L10.6875 8.28889C10.6875 8.49271 10.6857 8.54018 10.6794 8.57245L11.0473 8.64489ZM11.0625 8.28889L11.4375 8.28889C11.4375 8.12071 11.4393 7.98258 11.4152 7.86045L11.0473 7.93289L10.6794 8.00534C10.6857 8.03761 10.6875 8.08508 10.6875 8.28889L11.0625 8.28889ZM11.0473 7.93289L11.4151 7.85983C11.3698 7.63179 11.2579 7.42231 11.0935 7.25791L10.8283 7.52308L10.5632 7.78824C10.6226 7.84771 10.6631 7.92347 10.6795 8.00596L11.0473 7.93289ZM10.8283 7.52308L11.0935 7.25791C10.9291 7.09351 10.7196 6.98158 10.4916 6.93628L10.4185 7.30409L10.3454 7.67191C10.4279 7.68829 10.5037 7.72878 10.5632 7.78824L10.8283 7.52308ZM10.4185 7.30409L10.4909 6.93616C10.3688 6.91211 10.2307 6.91389 10.0625 6.91389L10.0625 7.28889L10.0625 7.66389C10.2663 7.66389 10.3138 7.66568 10.3461 7.67203L10.4185 7.30409ZM10.0625 7.28889L10.0625 6.91389C9.89432 6.91389 9.75618 6.91211 9.63406 6.93616L9.7065 7.30409L9.77894 7.67203C9.81122 7.66568 9.85868 7.66389 10.0625 7.66389L10.0625 7.28889ZM9.7065 7.30409L9.63344 6.93628C9.40539 6.98158 9.19592 7.09351 9.03152 7.25791L9.29668 7.52308L9.56185 7.78824C9.62131 7.72878 9.69708 7.68829 9.77956 7.67191L9.7065 7.30409ZM9.29668 7.52308L9.03152 7.25791C8.86712 7.42231 8.75518 7.63179 8.70989 7.85983L9.0777 7.93289L9.44551 8.00596C9.4619 7.92347 9.50238 7.84771 9.56185 7.78824L9.29668 7.52308ZM4.32832 2.55471L4.06315 2.81988L9.03152 7.78824L9.29668 7.52308L9.56185 7.25791L4.59349 2.28955L4.32832 2.55471ZM2.5777 1.43289L2.20977 1.36045C2.18572 1.48258 2.1875 1.62071 2.1875 1.78889L2.5625 1.78889L2.9375 1.78889C2.9375 1.58508 2.93928 1.53761 2.94564 1.50534L2.5777 1.43289ZM2.5625 1.78889L2.1875 1.78889C2.1875 1.95708 2.18572 2.09521 2.20977 2.21734L2.5777 2.14489L2.94564 2.07245C2.93928 2.04018 2.9375 1.99271 2.9375 1.78889L2.5625 1.78889ZM2.5777 2.14489L2.20989 2.21796C2.25519 2.446 2.36712 2.65548 2.53152 2.81988L2.79669 2.55471L3.06185 2.28955C3.00239 2.23008 2.9619 2.15431 2.94552 2.07183L2.5777 2.14489ZM2.79669 2.55471L2.53152 2.81988C2.69592 2.98428 2.9054 3.09621 3.13344 3.14151L3.2065 2.77369L3.27957 2.40588C3.19708 2.3895 3.12131 2.34901 3.06185 2.28955L2.79669 2.55471ZM3.2065 2.77369L3.13406 3.14163C3.25619 3.16568 3.39432 3.16389 3.5625 3.16389L3.5625 2.78889L3.5625 2.41389C3.35869 2.41389 3.31122 2.41211 3.27895 2.40576L3.2065 2.77369ZM3.5625 2.78889L3.5625 3.16389C3.73069 3.16389 3.86882 3.16568 3.99095 3.14163L3.9185 2.77369L3.84606 2.40576C3.81379 2.41211 3.76632 2.41389 3.5625 2.41389L3.5625 2.78889ZM3.9185 2.77369L3.99157 3.14151C4.21961 3.09621 4.42909 2.98428 4.59349 2.81988L4.32832 2.55471L4.06315 2.28955C4.00369 2.34901 3.92792 2.3895 3.84544 2.40588L3.9185 2.77369ZM4.32832 2.55471L4.59349 2.81988C4.75788 2.65548 4.86982 2.446 4.91512 2.21796L4.5473 2.14489L4.17949 2.07183C4.1631 2.15431 4.12262 2.23008 4.06315 2.28955L4.32832 2.55471ZM4.5473 2.14489L4.91524 2.21734C4.93928 2.09521 4.9375 1.95708 4.9375 1.78889L4.5625 1.78889L4.1875 1.78889C4.1875 1.99271 4.18572 2.04018 4.17937 2.07245L4.5473 2.14489ZM4.5625 1.78889L4.9375 1.78889C4.9375 1.62071 4.93928 1.48258 4.91524 1.36045L4.5473 1.43289L4.17937 1.50534C4.18572 1.53761 4.1875 1.58508 4.1875 1.78889L4.5625 1.78889ZM4.5473 1.43289L4.91512 1.35983C4.86982 1.13179 4.75789 0.922313 4.59348 0.757913L4.32832 1.02308L4.06316 1.28824C4.12262 1.34771 4.1631 1.42347 4.17949 1.50596L4.5473 1.43289ZM4.32832 1.02308L4.59348 0.757913C4.42908 0.593512 4.21961 0.48158 3.99157 0.436281L3.9185 0.804094L3.84544 1.17191C3.92792 1.18829 4.00369 1.22878 4.06316 1.28824L4.32832 1.02308ZM3.9185 0.804094L3.99095 0.436158C3.86882 0.412113 3.73069 0.413894 3.5625 0.413894L3.5625 0.788894L3.5625 1.16389C3.76632 1.16389 3.81379 1.16568 3.84606 1.17203L3.9185 0.804094ZM3.5625 0.788894L3.5625 0.413894C3.39432 0.413894 3.25619 0.412113 3.13406 0.436158L3.2065 0.804094L3.27895 1.17203C3.31122 1.16568 3.35869 1.16389 3.5625 1.16389L3.5625 0.788894ZM3.2065 0.804094L3.13344 0.436281C2.9054 0.48158 2.69592 0.593512 2.53152 0.757913L2.79669 1.02308L3.06185 1.28824C3.12131 1.22878 3.19708 1.18829 3.27957 1.17191L3.2065 0.804094ZM2.79669 1.02308L2.53152 0.757913C2.36712 0.922312 2.25519 1.13179 2.20989 1.35983L2.5777 1.43289L2.94552 1.50596C2.9619 1.42348 3.00239 1.34771 3.06185 1.28824L2.79669 1.02308Z" fill="black"/>
<path d="M0.5625 4.86669C1.34874 4.19469 2.36933 3.78889 3.4847 3.78889C5.96999 3.78889 7.9847 5.80361 7.9847 8.28889C7.9847 9.40427 7.57891 10.4249 6.90691 11.2111" stroke="black" stroke-width="0.75" stroke-linecap="round"/>
</g>
<defs>
<clipPath id="clip0_9_8">
<rect width="12" height="12" fill="white"/>
</clipPath>
</defs>
</svg>"#;
pub(crate) const ICON_CHIP_PARALLEL: &[u8] = br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M3.20312 6.72656L6.79688 3.13281M5.27344 8.79688L8.86719 5.20312" stroke="black" stroke-width="0.75" stroke-linecap="round"/><circle cx="2.29" cy="7.64" r="1.04" stroke="black" stroke-width="0.75"/><circle cx="7.64" cy="2.29" r="1.04" stroke="black" stroke-width="0.75"/><circle cx="4.36" cy="9.71" r="1.04" stroke="black" stroke-width="0.75"/><circle cx="9.71" cy="4.36" r="1.04" stroke="black" stroke-width="0.75"/></svg>"#;

impl CanvasView {
    // Dimension labels painted directly into the canvas pass — same
    // coordinates as the lines, so they are always anchored together.
    fn dimension_layer(&self) -> impl IntoElement {
        let editor = self.editor.clone();
        let editor_hit = self.editor.clone();

        let prepaint = move |_: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            let Some(editor) = editor.upgrade() else {
                return Vec::new();
            };
            let ed = editor.read(cx);
            let t = *crate::theme::active(cx);

            // Dimension labels: transient previews in accent, tool-created
            // constraint dims in the muted constraint ink. Hover lifts a
            // constraint dim to text_secondary. The EDITING dim keeps its
            // normal container — the selection highlight goes on the TEXT.
            let ink = |d_constraint: bool, d_hovered: bool, d_editing: bool| -> gpui::Hsla {
                if d_editing {
                    rgb(t.bg_primary).into()
                } else if d_constraint && d_hovered {
                    rgb(t.text_secondary).into()
                } else if d_constraint {
                    rgb(t.empty_text_secondary).into()
                } else {
                    rgb(t.accent).into()
                }
            };
            let border = |d_constraint: bool, d_hovered: bool, d_editing: bool| -> u32 {
                if d_constraint && d_hovered {
                    t.text_secondary
                } else if d_constraint {
                    t.empty_text_secondary
                } else {
                    t.accent
                }
            };
            let mut labels: Vec<LabelPrim> = ed
                .dim_renders
                .iter()
                .map(|d| {
                    make_label(
                        window,
                        d.text.clone(),
                        d.label_cx,
                        d.label_cy,
                        rgb(t.bg_primary).into(),
                        ink(d.constraint, d.hovered, d.editing),
                        border(d.constraint, d.hovered, d.editing),
                        d.dim_index,
                        d.editing,
                    )
                })
                .collect();
            labels.extend(ed.angle_dim_renders.iter().map(|a| {
                make_label(
                    window,
                    a.text.clone(),
                    a.label_cx,
                    a.label_cy,
                    rgb(t.bg_primary).into(),
                    ink(a.constraint, a.hovered, a.editing),
                    border(a.constraint, a.hovered, a.editing),
                    a.dim_index,
                    a.editing,
                )
            }));
            labels
        };

        let paint_labels = move |bounds: Bounds<Pixels>,
                                 labels: Vec<LabelPrim>,
                                 window: &mut Window,
                                 cx: &mut App| {
            // Convert canvas-local prim coords to window space.
            let (ox, oy) = (bounds.origin.x, bounds.origin.y);
            // Hitboxes (canvas-local) for placed dims — consumed by the
            // editor's hover/double-click/drag hit-testing.
            let mut hitboxes: Vec<(usize, [f32; 4])> = Vec::new();
            for l in &labels {
                const PAD_X: f32 = 6.;
                const BORDER: f32 = 4.;
                const BOX_H: f32 = 22.;
                if let Some(idx) = l.dim_index {
                    let box_w = l.line.width.as_f32() + PAD_X * 2. + BORDER;
                    hitboxes.push((
                        idx,
                        [
                            l.center_x - box_w / 2.,
                            l.center_y - BOX_H / 2.,
                            box_w,
                            BOX_H,
                        ],
                    ));
                }
            }
            if let Some(editor) = editor_hit.upgrade() {
                editor.update(cx, |ed, _| ed.dim_hitboxes = hitboxes);
            }
            for l in labels {
                const PAD_X: f32 = 6.;
                const BORDER: f32 = 4.;
                const BOX_H: f32 = 22.;
                let line_h = font_size_px() * 1.4;
                let box_w = l.line.width.as_f32() + PAD_X * 2. + BORDER;
                // Optically center: nudge down by the descent share of
                // the line box (glyphs sit above the box center).
                const OPTICAL_NUDGE: f32 = 2.;
                // Border stroke eats 2px of each padding side; offset by
                // half the border width so left/right gaps are equal.
                let origin = Point {
                    // `TextAlign::Center` centers within the supplied line
                    // width. Its origin therefore starts at the text's own
                    // left edge, not at the container's padded left edge.
                    // Including PAD_X here double-centered the text and put
                    // every value visibly to the right of its container.
                    x: px(l.center_x - l.line.width.as_f32() / 2.) + ox,
                    y: px(l.center_y - line_h.as_f32() / 2. + OPTICAL_NUDGE) + oy,
                };
                // Container background + border: sized to the measured
                // text (adaptable width) via padding, not fixed. Editing
                // keeps the container normal — the SELECTION goes on the
                // text itself, like a real text input.
                let t = *crate::theme::active(cx);
                window.paint_quad(gpui::quad(
                    Bounds {
                        origin: Point {
                            x: px(l.center_x - box_w / 2.) + ox,
                            y: px(l.center_y - BOX_H / 2.) + oy,
                        },
                        size: Size {
                            width: px(box_w),
                            height: px(BOX_H),
                        },
                    },
                    px(6.),
                    l.bg,
                    gpui::Edges::all(px(2.)),
                    rgb(l.border),
                    gpui::BorderStyle::Solid,
                ));
                if l.editing {
                    // Selected-text bar behind the glyphs.
                    window.paint_quad(gpui::fill(
                        Bounds {
                            origin: Point {
                                x: origin.x - px(2.),
                                y: origin.y,
                            },
                            size: Size {
                                width: px(l.line.width.as_f32() + 4.),
                                height: line_h,
                            },
                        },
                        rgb(t.accent),
                    ));
                }
                let _ = l.line.paint(
                    origin,
                    font_size_px(),
                    gpui::TextAlign::Center,
                    Some(px(l.line.width.as_f32())),
                    window,
                    cx,
                );
                // Editing caret: a blinking 1px bar right after the text.
                if l.editing {
                    let caret_visible = editor_hit
                        .upgrade()
                        .map(|ed| ed.read(cx).dim_caret_visible)
                        .unwrap_or(true);
                    if caret_visible {
                        window.paint_quad(gpui::fill(
                            Bounds {
                                origin: Point {
                                    x: origin.x + l.line.width + px(2.),
                                    y: origin.y,
                                },
                                size: Size {
                                    width: px(1.),
                                    height: line_h,
                                },
                            },
                            rgb(t.text_primary),
                        ));
                    }
                }
            }
        };

        canvas(prepaint, paint_labels)
            .absolute()
            .inset_0()
            .size_full()
    }

    // Constraint chips: DOM overlay glued to the painted geometry. Chip
    // square + glyph ride a real SVG asset (same convention as the
    // toolbar). Purely visual — hit-testing stays editor-side
    // (constraint_chip_at), so nothing here intercepts clicks.
    fn constraint_chip_layer(&self, cx: &App) -> impl IntoElement {
        use crate::core::constraints::ConstraintKind;

        let t = *crate::theme::active(cx);
        let markers = self
            .editor
            .upgrade()
            .map(|e| e.read(cx).constraint_markers.clone())
            .unwrap_or_default();

        div()
            .absolute()
            .inset_0()
            .size_full()
            .children(markers.iter().filter(|m| m.visible).map(|m| {
                const S: f32 = CHIP_SIZE;
                let icon = match m.constraint.kind {
                    ConstraintKind::Coincident => ICON_CHIP_COINCIDENT,
                    ConstraintKind::Horizontal => ICON_CHIP_HORIZONTAL,
                    ConstraintKind::Vertical => ICON_CHIP_VERTICAL,
                    ConstraintKind::Tangent => ICON_CHIP_TANGENT,
                    ConstraintKind::Parallel => ICON_CHIP_PARALLEL,
                };
                let border = if m.clicked { t.accent_border } else { t.accent };
                let bg = if m.emphasized { t.accent } else { t.bg_primary };
                let icon_color = if m.emphasized {
                    rgb(0xFFFFFF)
                } else {
                    // 70% so the glyph stays readable over bg_primary;
                    // full opacity once the chip itself is hovered.
                    rgba((t.accent_border << 8) | if m.hovered { 0xFF } else { 0xB3 })
                };
                div()
                    .absolute()
                    .left(px(m.cx_out - S / 2.))
                    .top(px(m.cy_out - S / 2.))
                    .w(px(S as f32 + 2.0))
                    .h(px(S as f32 + 2.0))
                    .rounded(px(6.))
                    .border(px(1.5))
                    .border_color(rgb(border))
                    .bg(rgb(bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(svg().data(icon).w(px(S)).h(px(S)).text_color(icon_color))
            }))
    }
}

struct LabelPrim {
    line: gpui::ShapedLine,
    center_x: f32,
    center_y: f32,
    bg: gpui::Background,
    border: u32,
    dim_index: Option<usize>,
    editing: bool,
}

fn make_label(
    window: &mut Window,
    text: String,
    center_x: f32,
    center_y: f32,
    bg: gpui::Background,
    accent: gpui::Hsla,
    border: u32,
    dim_index: Option<usize>,
    editing: bool,
) -> LabelPrim {
    let font_size = px(11.);
    let runs = [gpui::TextRun {
        len: text.len(),
        font: label_font(),
        color: accent,
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let line = window
        .text_system()
        .shape_line(text.into(), font_size, &runs, None);
    LabelPrim {
        line,
        center_x,
        center_y,
        bg,
        border,
        dim_index,
        editing,
    }
}

fn font_size_px() -> gpui::Pixels {
    px(13.)
}

// Dimensions always show two decimal places: 100.00, 37.50, 12.34.
pub fn fmt_dim(v: f64) -> String {
    format!("{v:.2}")
}

fn label_font() -> gpui::Font {
    gpui::Font {
        family: crate::theme::FONT_UI.into(),
        weight: gpui::FontWeight::SEMIBOLD,
        ..Default::default()
    }
}

impl CanvasView {
    fn paint_layer(&self) -> impl IntoElement {
        let editor = self.editor.clone();
        let editor_paint = self.editor.clone();

        let prepaint = move |bounds: Bounds<Pixels>,
                             window: &mut Window,
                             cx: &mut App|
              -> (Vec<paint::Primitive>, gpui::Hitbox) {
            let hitbox = window.insert_hitbox(bounds, HitboxBehavior::default());
            let Some(editor) = editor.upgrade() else {
                return (Vec::new(), hitbox);
            };
            // Keep the editor's snap-search region in sync with the view.
            let _ = editor.update(cx, |ed, _| {
                let w = f64::from(bounds.size.width);
                let h = f64::from(bounds.size.height);
                if (ed.viewport_size.0 - w).abs() > 0.5 || (ed.viewport_size.1 - h).abs() > 0.5 {
                    ed.viewport_size = (w, h);
                }
            });
            let ed = editor.read(cx);
            let t = *crate::theme::active(cx);
            let pending = ed.pending_shape.map(|p| p.bounds());
            let pending_ruler = ed.pending_ruler.map(|p| p.snapped(ed.shift));
            let pending_line = ed.pending_line.map(|p| {
                if ed.shift {
                    if let Some(tangent) = ed.tangent_snap_for_line(p.start, p.cursor) {
                        return (p.start, tangent);
                    }
                }
                p.snapped(ed.shift)
            });
            let cursor_doc = ed.last_cursor.map(|c| {
                ed.camera.screen_to_unit(crate::core::geometry::Point2::new(
                    f64::from(c.x),
                    f64::from(c.y),
                ))
            });
            let list = paint::build_draw_list(
                &ed.doc,
                &ed.camera,
                bounds.size,
                t,
                pending,
                &ed.selection,
                ed.hover.filter(|_| ed.dragging.is_none()),
                &ed.dim_renders,
                &ed.angle_dim_renders,
                &ed.snap_guides,
                ed.marquee,
                pending_ruler,
                pending_line,
                &ed.constraint_markers,
                ed.pending_circle,
                ed.show_grid,
                ed.tool,
                cursor_doc,
            );
            (list, hitbox)
        };

        let paint = move |bounds: Bounds<Pixels>,
                          (list, hitbox): (Vec<paint::Primitive>, gpui::Hitbox),
                          window: &mut Window,
                          cx: &mut App| {
            // Convert canvas-local prim coords to window space.
            let (ox, oy) = (bounds.origin.x, bounds.origin.y);
            // Dynamic cursor per tool/state.
            if let Some(editor) = editor_paint.upgrade() {
                let style = editor.read(cx).cursor_style();
                window.set_cursor_style(style, &hitbox);
            }
            for prim in list {
                match prim {
                    paint::Primitive::Rect { x, y, w, h, color } => {
                        window.paint_quad(fill(
                            Bounds {
                                origin: Point {
                                    x: px(x) + ox,
                                    y: px(y) + oy,
                                },
                                size: Size {
                                    width: px(w),
                                    height: px(h),
                                },
                            },
                            color,
                        ));
                    }
                    paint::Primitive::Polygon { points, color } => {
                        if points.len() < 3 {
                            continue;
                        }
                        let to_px = |(x, y): (f32, f32)| Point {
                            x: px(x) + ox,
                            y: px(y) + oy,
                        };
                        let mut path = gpui::Path::new(to_px(points[0]));
                        for &pt in &points[1..] {
                            path.line_to(to_px(pt));
                        }
                        path.line_to(to_px(points[0]));
                        window.paint_path(path, color);
                    }
                    paint::Primitive::Line {
                        ax,
                        ay,
                        bx,
                        by,
                        width,
                        color,
                    } => {
                        // Thin filled quad along the segment.
                        let dx = bx - ax;
                        let dy = by - ay;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len < 1e-3 {
                            continue;
                        }
                        let nx = -dy / len * width / 2.;
                        let ny = dx / len * width / 2.;
                        let mut path = gpui::Path::new(Point {
                            x: px(ax + nx) + ox,
                            y: px(ay + ny) + oy,
                        });
                        path.line_to(Point {
                            x: px(bx + nx) + ox,
                            y: px(by + ny) + oy,
                        });
                        path.line_to(Point {
                            x: px(bx - nx) + ox,
                            y: px(by - ny) + oy,
                        });
                        path.line_to(Point {
                            x: px(ax - nx) + ox,
                            y: px(ay - ny) + oy,
                        });
                        path.line_to(Point {
                            x: px(ax + nx) + ox,
                            y: px(ay + ny) + oy,
                        });
                        window.paint_path(path, color);
                    }
                    paint::Primitive::Outline { x, y, w, h } => {
                        window.paint_quad(gpui::quad(
                            Bounds {
                                origin: Point {
                                    x: px(x) + ox,
                                    y: px(y) + oy,
                                },
                                size: Size {
                                    width: px(w),
                                    height: px(h),
                                },
                            },
                            0.,
                            gpui::transparent_black(),
                            gpui::Edges::all(px(1.)),
                            rgb(crate::theme::active(cx).accent),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    paint::Primitive::Circle {
                        cx: mcx,
                        cy: mcy,
                        radius,
                    } => {
                        // Points render as rounded squares (2px corners).
                        let r = px(radius);
                        window.paint_quad(gpui::quad(
                            Bounds {
                                origin: Point {
                                    x: px(mcx) - r + ox,
                                    y: px(mcy) - r + oy,
                                },
                                size: Size {
                                    width: r * 2.,
                                    height: r * 2.,
                                },
                            },
                            px(3.),
                            rgb(0xFFFFFF),
                            gpui::Edges::all(px(1.)),
                            rgb(crate::theme::active(cx).accent),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    paint::Primitive::RulerLabel {
                        center_x,
                        anchor_y,
                        px_value,
                        in_value,
                    } => {
                        // Two-row vector label centered on the inch tick,
                        // sitting entirely BEYOND the tick tips: pixels row
                        // on top (nearest the dashes), inches below it.
                        // Value in ink; unit suffix in empty_text_primary.
                        const SIZE: f32 = 9.;
                        const ROW_GAP: f32 = 2.;
                        let t = *crate::theme::active(cx);
                        let value_color = rgb(t.text_secondary).into();
                        let unit_color = rgb(t.empty_text_primary).into();
                        let font = gpui::Font {
                            family: crate::theme::FONT_UI.into(),
                            weight: gpui::FontWeight::MEDIUM,
                            ..Default::default()
                        };

                        // Rows top -> bottom: px first, inches under it.
                        let rows = [
                            (px_value.clone(), "px", anchor_y),
                            (in_value.clone(), "in", anchor_y + SIZE + ROW_GAP),
                        ];
                        for (value, unit, top_y) in rows {
                            let text = format!("{value}{unit}");
                            let runs = [
                                gpui::TextRun {
                                    len: value.len(),
                                    font: font.clone(),
                                    color: value_color,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                },
                                gpui::TextRun {
                                    len: unit.len(),
                                    font: font.clone(),
                                    color: unit_color,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                },
                            ];
                            let line =
                                window
                                    .text_system()
                                    .shape_line(text.into(), px(SIZE), &runs, None);
                            // Center on the tick.
                            let origin_x = center_x - line.width.as_f32() / 2.;
                            let _ = line.paint(
                                Point {
                                    x: px(origin_x) + ox,
                                    y: px(top_y) + oy,
                                },
                                px(SIZE),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    }
                }
            }
        };

        canvas(prepaint, paint).absolute().inset_0().size_full()
    }

    // Creation-tool snap cursor: the makeshift crosshair that rides the real
    // cursor. Unsnapped, its center is glued to the cursor point; when a snap
    // locks (endpoint, edge, grid crossing) it detaches to the target and the
    // accent square lights up. The OS cursor stays a plain arrow. The stored
    // position is in DOC coords, re-projected through the live camera here —
    // so it tracks zoom/pan perfectly instead of drifting off the cursor.
    fn snap_cursor_layer(&self, cx: &App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let Some(editor) = self.editor.upgrade() else {
            return div().absolute();
        };
        let (camera, state) = {
            let ed = editor.read(cx);
            (ed.camera, ed.creation_cursor)
        };
        let Some((dx, dy, snapped)) = state else {
            return div().absolute();
        };
        let s = camera.unit_to_screen(crate::core::geometry::Point2::new(dx, dy));
        let (x, y) = (s.x as f32, s.y as f32);
        const S: f32 = 18.;
        let mut layer = div()
            .absolute()
            // Crosshair glyph centered on the (possibly snapped-away) point.
            .left(px(x - S / 2.))
            .top(px(y - S / 2.))
            .w(px(S))
            .h(px(S))
            .child(
                svg()
                    .data(ICON_CROSSHAIR)
                    .size_full()
                    .text_color(rgb(t.text_primary)),
            );
        if snapped {
            const SQUARE: f32 = 14.;
            layer = layer.child(
                div()
                    .absolute()
                    .left(px((S - SQUARE) / 2.))
                    .top(px((S - SQUARE) / 2.))
                    .w(px(SQUARE))
                    .h(px(SQUARE))
                    .border_1()
                    .border_color(rgb(t.accent)),
            );
        }
        layer
    }
}

// The snapping cursor glyph: a thin plus, drawn in ink; the accent square
// badge comes from the layer above it when a snap is engaged.
const ICON_CROSSHAIR: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 4v16m8-8H4" /></svg>"#;
