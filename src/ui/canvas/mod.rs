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
            let t = *crate::theme::active(cx);

            // Edge-resize: one label for the changing axis.
            if let Some((geom, is_width)) = ed.edge_dim {
                let Some(rid) = ed.resizing.as_ref().map(|rs| rs.id) else {
                    return Vec::new();
                };
                let Some(b) = ed.doc.shape_bounds(rid) else {
                    return Vec::new();
                };
                let value = if is_width { b.size.w } else { b.size.h };
                let centers =
                    paint::dimension_label_centers(geom.x, geom.y, geom.w, geom.h, geom.ext);
                let (cx_, cy_) = centers[if is_width { 0 } else { 1 }];
                return vec![make_label(
                    window,
                    format!("{value:.2}"),
                    cx_,
                    cy_,
                    rgb(t.bg_primary).into(),
                    rgb(t.accent).into(),
                )];
            }

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
            let color: gpui::Hsla = rgb(t.accent).into();

            [(format!("{sw:.2}"), centers[0]), (format!("{sh:.2}"), centers[1])]
                .into_iter()
                .map(|(text, (cx_, cy_))| {
                    make_label(window, text, cx_, cy_, rgb(t.bg_primary).into(), color)
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
                    // Container background + border: sized to the measured
                    // text (adaptable width) via padding, not fixed.
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
                        rgb(crate::theme::active(cx).accent),
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

fn make_label(
    window: &mut Window,
    text: String,
    center_x: f32,
    center_y: f32,
    bg: gpui::Background,
    accent: gpui::Hsla,
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
    LabelPrim { line, center_x, center_y, bg }
}

fn font_size_px() -> gpui::Pixels {
    px(13.)
}

// Two-decimal precision without the ugly trailing zeros: 100.00 -> "100",
// 37.50 -> "37.5", 12.34 stays "12.34".
pub fn fmt_dim(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
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
        let width_text = fmt_dim(sw);
        let height_text = fmt_dim(sh);
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
                    .border_color(rgb(t.accent))
                    .rounded(px(6.))
                    .font_family(crate::theme::FONT_UI)
                    .text_size(font_size)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(t.accent))
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
                &ed.selection,
                ed.dim_geom,
                &ed.snap_guides,
                ed.hover
                    .map(|h| (h.shape, h.handle))
                    .filter(|_| ed.resizing.is_none() && ed.dragging.is_none()),
                ed.selected_handles.as_slice(),
                ed.edge_dim,
                ed.marquee,
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
                    paint::Primitive::Outline { bounds } => {
                        window.paint_quad(gpui::quad(
                            bounds,
                            0.,
                            gpui::transparent_black(),
                            gpui::Edges::all(px(2.)),
                            rgb(crate::theme::active(cx).accent),
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
                            rgb(crate::theme::active(cx).accent),
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
                            rgb(crate::theme::active(cx).accent),
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








