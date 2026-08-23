use gpui::{
    App, Bounds, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, RenderOnce, ScrollDelta, ScrollWheelEvent, Size, Window, canvas,
    div, fill, prelude::*, px, rgb,
};

use crate::editor::Editor;
use crate::ui::shell::title_bar::TITLE_BAR_HEIGHT;

pub mod paint;

// Canvas view: renders the document through the editor camera and owns the
// viewport interactions (pan, zoom). Stateless — all state lives on Editor.

#[derive(IntoElement)]
pub struct CanvasView {
    pub editor: gpui::WeakEntity<Editor>,
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
        let focus_l = self.focus.clone();
        let focus_m = self.focus.clone();

        div()
            .id("canvas")
            .relative()
            .size_full()
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
                    if ed.canvas_down(MouseButton::Left, e.position) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_down(MouseButton::Middle, move |e: &MouseDownEvent, window, cx| {
                window.focus(&focus_m, cx);
                let _ = editor_down_m.update(cx, |ed, cx| {
                    if ed.canvas_down(MouseButton::Middle, e.position) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                let shift = e.modifiers.shift;
                let _ = editor_move.update(cx, |ed, cx| {
                    ed.alt_down = e.modifiers.alt;
                    ed.update_dim_geom();
                    let mut changed = false;
                    // While idle, track which resize handle is under the
                    // cursor (used for cursor styling).
                    if ed.pan_start_none() {
                        changed |= ed.canvas_hover(e.position);
                    }
                    changed |= ed.canvas_drag(e.position, shift);
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
                    ed.zoom_at(e.position, amount);
                    cx.notify();
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
            .child(self.dimension_layer())
    }
}

impl CanvasView {
    // Dimension labels painted directly into the canvas pass — same
    // coordinates as the lines, so they are always anchored together.
    fn dimension_layer(&self) -> impl IntoElement {
        let editor = self.editor.clone();

        let prepaint = move |_: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            let Some(editor) = editor.upgrade() else {
                return Vec::new();
            };
            let ed = editor.read(cx);
            let Some(geom) = ed.dim_geom else {
                return Vec::new();
            };
            let Some((sw, sh)) = ed.selection_size() else {
                return Vec::new();
            };
            let t = *crate::theme::active(cx);

            let centers = paint::dimension_label_centers(
                geom.x,
                geom.y,
                geom.w,
                geom.h,
                geom.ext,
            );
            let font_size = px(11.);
            let font = label_font();
            let color: gpui::Hsla = rgb(crate::theme::ACCENT).into();

            [(format!("{sw:.2}"), centers[0]), (format!("{sh:.2}"), centers[1])]
                .into_iter()
                .map(|(text, (cx_, cy_))| {
                    let runs = [gpui::TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }];
                    let line = window
                        .text_system()
                        .shape_line(text.into(), font_size, &runs, None);
                    LabelPrim {
                        line,
                        center_x: cx_,
                        center_y: cy_,
                        bg: rgb(t.bg_primary).into(),
                    }
                })
                .collect::<Vec<_>>()
        };

        let paint_labels =
            move |_: Bounds<Pixels>,
                  labels: Vec<LabelPrim>,
                  window: &mut Window,
                  cx: &mut App| {
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
                        x: px(l.center_x - box_w / 2. + PAD_X + BORDER / 2.),
                        y: px(l.center_y - line_h.as_f32() / 2. + OPTICAL_NUDGE),
                    };
                    // Container background + accent border.
                    window.paint_quad(gpui::quad(
                        Bounds {
                            origin: Point {
                                x: px(l.center_x - box_w / 2.),
                                y: px(l.center_y - BOX_H / 2.),
                            },
                            size: Size { width: px(box_w), height: px(BOX_H) },
                        },
                        px(6.),
                        l.bg,
                        gpui::Edges::all(px(2.)),
                        rgb(crate::theme::ACCENT),
                        gpui::BorderStyle::Solid,
                    ));
                    let _ = l.line.paint(
                        origin,
                        font_size_px(),
                        gpui::TextAlign::Center,
                        Some(px(l.line.width.as_f32())),
                        window,
                        cx,
                    );
                }
            };

        canvas(prepaint, paint_labels)
            .absolute()
            .inset_0()
            .size_full()
    }
}

struct LabelPrim {
    line: gpui::ShapedLine,
    center_x: f32,
    center_y: f32,
    bg: gpui::Background,
}

fn font_size_px() -> gpui::Pixels {
    px(13.)
}

fn label_font() -> gpui::Font {
    gpui::Font {
        family: crate::theme::FONT_UI.into(),
        weight: gpui::FontWeight::SEMIBOLD,
        ..Default::default()
    }
}

impl CanvasView {
    fn dimension_labels(&self, cx: &App) -> Vec<gpui::AnyElement> {
        let Some(editor) = self.editor.upgrade() else {
            return Vec::new();
        };
        let ed = editor.read(cx);
        let Some(geom) = ed.dim_geom else {
            return Vec::new();
        };
        let t = *crate::theme::active(cx);

        let Some((sw, sh)) = ed.selection_size() else {
            return Vec::new();
        };
        let width_text = format!("{:.2}", sw);
        let height_text = format!("{:.2}", sh);
        let centers = paint::dimension_label_centers(geom.x, geom.y, geom.w, geom.h, geom.ext);
        let texts = [(width_text, 0), (height_text, 1)];

        let font_size = px(11.);
        let font = gpui::Font {
            family: crate::theme::FONT_UI.into(),
            weight: gpui::FontWeight::MEDIUM,
            ..Default::default()
        };

        texts
            .into_iter()
            .map(|(text, i)| {
                let (cx_, cy_) = centers[i as usize];
                // Measure the actual rendered text so the container is
                // perfectly centered on the dimension line.
                let font_id = cx.text_system().resolve_font(&font);
                let text_w: f32 = text
                    .chars()
                    .map(|ch| cx.text_system().layout_width(font_id, font_size, ch).as_f32())
                    .sum();
                // Exact box: text + 2*6px padding + 2*2px border, fixed height.
                const PAD_X: f32 = 6.;
                const BORDER: f32 = 4.;
                const BOX_H: f32 = 22.;
                let est_w = text_w + PAD_X * 2. + BORDER;
                div()
                    .absolute()
                    .left(px(cx_ - est_w / 2.))
                    .top(px(cy_ - BOX_H / 2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(est_w))
                    .h(px(BOX_H))
                    .bg(rgb(t.bg_primary))
                    .border_2()
                    .border_color(rgb(crate::theme::ACCENT))
                    .rounded(px(6.))
                    .font_family(crate::theme::FONT_UI)
                    .text_size(font_size)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(crate::theme::ACCENT))
                    .child(text)
                    .into_any_element()
            })
            .collect()
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
            let ed = editor.read(cx);
            let t = *crate::theme::active(cx);
            let pending = ed.pending_shape.map(|p| (p.kind, p.bounds()));
            let list = paint::build_draw_list(
                &ed.doc,
                &ed.camera,
                bounds.size,
                t,
                pending,
                ed.selection,
                ed.dim_geom,
                &ed.snap_guides,
            );
            (list, hitbox)
        };

        let paint = move |_: Bounds<Pixels>,
                          (list, hitbox): (Vec<paint::Primitive>, gpui::Hitbox),
                          window: &mut Window,
                          cx: &mut App| {
            // Dynamic cursor for resize handles.
            if let Some(editor) = editor_paint.upgrade() {
                let style = editor.read(cx).cursor_style();
                window.set_cursor_style(style, &hitbox);
            }
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
                        window.paint_quad(gpui::quad(
                            bounds,
                            0.,
                            gpui::transparent_black(),
                            gpui::Edges::all(px(2.)),
                            rgb(crate::theme::ACCENT),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    paint::Primitive::Circle { center, radius } => {
                        let r = radius;
                        window.paint_quad(gpui::quad(
                            Bounds {
                                origin: Point { x: center.x - r, y: center.y - r },
                                size: Size { width: r * 2., height: r * 2. },
                            },
                            r,
                            rgb(0xFFFFFF),
                            gpui::Edges::all(px(1.)),
                            rgb(crate::theme::ACCENT),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    paint::Primitive::CornerHandle { center } => {
                        const SIZE: f32 = 7.;
                        let bounds = Bounds {
                            origin: Point {
                                x: center.x - px(SIZE / 2.),
                                y: center.y - px(SIZE / 2.),
                            },
                            size: Size { width: px(SIZE), height: px(SIZE) },
                        };
                        window.paint_quad(gpui::quad(
                            bounds,
                            1.,
                            rgb(0xFFFFFF),
                            gpui::Edges::all(px(1.)),
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








