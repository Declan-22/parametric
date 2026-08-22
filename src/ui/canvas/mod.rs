use gpui::{
    App, Bounds, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, RenderOnce, ScrollDelta, ScrollWheelEvent, Window, canvas, div, fill, prelude::*, px,
    rgb,
};

use crate::editor::Editor;
use crate::ui::shell::title_bar::TITLE_BAR_HEIGHT;

pub mod paint;

// Canvas view: renders the document through the editor camera and owns the
// viewport interactions (pan, zoom). Stateless — all state lives on Editor.

#[derive(IntoElement)]
pub struct CanvasView {
    pub editor: gpui::WeakEntity<Editor>,
}

impl RenderOnce for CanvasView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let editor = self.editor.clone();
        let editor_down_l = self.editor.clone();
        let editor_down_m = self.editor.clone();
        let editor_up_l = self.editor.clone();
        let editor_up_m = self.editor.clone();
        let editor_scroll = self.editor.clone();

        div()
            .id("canvas")
            .relative()
            .size_full()
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                let shift = e.modifiers.shift;
                let _ = editor.update(cx, |ed, cx| {
                    if ed.canvas_drag(e.position, shift) {
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
                    ed.zoom_at(e.position, amount);
                    cx.notify();
                });
            })
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| {
                let _ = editor_down_l.update(cx, |ed, cx| {
                    if ed.canvas_down(MouseButton::Left, e.position) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_down(MouseButton::Middle, move |e: &MouseDownEvent, _, cx| {
                let _ = editor_down_m.update(cx, |ed, cx| {
                    if ed.canvas_down(MouseButton::Middle, e.position) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                let _ = editor_up_l.update(cx, |ed, cx| {
                    if ed.canvas_up(MouseButton::Left) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Middle, move |_: &MouseUpEvent, _, cx| {
                let _ = editor_up_m.update(cx, |ed, cx| {
                    if ed.canvas_up(MouseButton::Middle) {
                        cx.notify();
                    }
                });
            })
            .child(self.paint_layer())
    }
}

impl CanvasView {
    fn paint_layer(&self) -> impl IntoElement {
        let editor = self.editor.clone();

        let prepaint =
            move |bounds: Bounds<Pixels>, _: &mut Window, cx: &mut App| -> Vec<paint::Primitive> {
                let Some(editor) = editor.upgrade() else {
                    return Vec::new();
                };
                let ed = editor.read(cx);
                let t = *crate::theme::active(cx);
                let pending = ed.pending_shape.map(|p| (p.kind, p.bounds()));
                paint::build_draw_list(
                    &ed.doc,
                    &ed.camera,
                    bounds.size,
                    t,
                    pending,
                    ed.selection,
                )
            };

        let paint = move |_: Bounds<Pixels>,
                          list: Vec<paint::Primitive>,
                          window: &mut Window,
                          _: &mut App| {
            for prim in list {
                match prim {
                    paint::Primitive::Rect { bounds, color } => {
                        window.paint_quad(fill(bounds, color));
                    }
                    paint::Primitive::Ellipse { center, radii, color } => {
                        if let Some(path) = paint::ellipse_path(center, radii) {
                            window.paint_path(path, color);
                        }
                    }
                    paint::Primitive::Outline { bounds } => {
                        window.paint_quad(gpui::outline(
                            bounds,
                            rgb(crate::theme::ACCENT),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                }
            }
        };

        canvas(prepaint, paint)
            .absolute()
            .inset_0()
            .size_full()
    }
}

// Layout offset where the canvas starts below the title bar (for overlays).
pub const CANVAS_TOP_INSET: Pixels = px(TITLE_BAR_HEIGHT);
