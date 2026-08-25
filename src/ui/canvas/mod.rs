use gpui::{
    svg, App, Bounds, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, RenderOnce, ScrollDelta, ScrollWheelEvent, Size, Window, canvas,
    div, fill, prelude::*, px, rgb, rgba,
};

use crate::editor::Editor;
use crate::ui::shell::title_bar::TITLE_BAR_HEIGHT;

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
                    if ed.canvas_down(MouseButton::Left, e.position, e.modifiers.shift, e.click_count) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_down(MouseButton::Middle, move |e: &MouseDownEvent, window, cx| {
                window.focus(&focus_m, cx);
                let _ = editor_down_m.update(cx, |ed, cx| {
                    if ed.canvas_down(MouseButton::Middle, e.position, false, 1) {
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
                    if ed.is_idle() {
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
            .child(self.dimension_layer())
            .child(self.constraint_layer(cx))
    }
}

// Constraint chip icons (vertical / horizontal), stroke = currentColor.
const ICON_CONSTRAINT_V: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 5c.59-.607 2.16-3 3-3s2.41 2.393 3 3M9 19c.59.607 2.16 3 3 3s2.41-2.393 3-3M12 2.231V21.77" />
</svg>"#;

const ICON_CONSTRAINT_H: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M5 9c-.607.59-3 2.16-3 3s2.393 2.41 3 3m14-6c.607.59 3 2.16 3 3s-2.393 2.41-3 3M2.423 11.98h19.445" />
</svg>"#;

const CHIP_SIZE: f32 = 18.;
const CHIP_ICON: f32 = 10.;

impl CanvasView {
    // Constraint chips: DOM overlay so each chip is hoverable/clickable.
    // The flip-slide tweens between the outer and flipped spots via the
    // shell fade system, keyed per constraint.
    fn constraint_layer(&self, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let mut layer = div().absolute().inset_0();

        let Some(editor) = self.editor.upgrade() else {
            return layer;
        };
        let markers = editor.read(cx).constraint_markers.clone();
        let shell = self.shell.upgrade();

        for m in markers {
            let slide_key = format!("{}-slide", m.key);
            let hover_key = format!("{}-hover", m.key);
            let target_slide = if m.flipped { 1.0 } else { 0.0 };
            let (k_slide, k_hover) = if let Some(sh) = &shell {
                // Kick the tween if the flip state changed.
                let cur = sh.read(cx).fade(&slide_key);
                if (cur - target_slide).abs() > 0.001 {
                    sh.update(cx, |sh, cx| sh.animate_fade(&slide_key, target_slide, cx));
                }
                let cur2 = sh.read(cx);
                (cur2.fade(&slide_key), cur2.fade(&hover_key))
            } else {
                (target_slide, 0.0)
            };

            let cxp = m.cx_out + (m.cx_in - m.cx_out) * k_slide;
            let cyp = m.cy_out + (m.cy_in - m.cy_out) * k_slide;

            // Always accent; hover LOWERS opacity (same treatment as the
            // home page's + New Design button). Selected: solid accent
            // container, white icon, visible border.
            let color = if m.selected {
                rgba((t.accent << 8) | 0xFF)
            } else {
                let alpha = (0xFF as f32 - 0xFF as f32 * 0.55 * k_hover) as u32;
                rgba((t.accent << 8) | alpha)
            };
            let bg_color = if m.selected {
                Some(rgb(t.accent))
            } else {
                None
            };

            let editor_click = self.editor.clone();
            let was_selected = m.selected;
            let constraint = m.constraint;
            let key_for_hover = hover_key;
            let shell_hover = self.shell.clone();
            let icon = if m.vertical { ICON_CONSTRAINT_V } else { ICON_CONSTRAINT_H };

            layer = layer.child(
                div()
                    .id(m.key)
                    .absolute()
                    .left(px(cxp - CHIP_SIZE / 2.))
                    .top(px(cyp - CHIP_SIZE / 2.))
                    .size(px(CHIP_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_2()
                    .border_color(color)
                    .rounded(px(6.))
                    .when_some(bg_color, |d, bg| d.bg(bg))
                    .on_hover(move |hovered, _, cx| {
                        if let Some(sh) = shell_hover.upgrade() {
                            let _ = sh.update(cx, |sh, cx| {
                                sh.animate_fade(
                                    &key_for_hover,
                                    if *hovered { 1.0 } else { 0.0 },
                                    cx,
                                )
                            });
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |_: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let _ = editor_click.update(cx, |ed, _| {
                            if was_selected {
                                ed.selected_constraints.retain(|&c| c != constraint);
                            } else {
                                ed.selected_constraints.clear();
                                ed.selected_constraints.push(constraint);
                            }
                        });
                    })
                    .child(
                        svg()
                            .data(icon)
                            .w(px(CHIP_ICON))
                            .h(px(CHIP_ICON))
                            .text_color(if m.selected {
                                rgb(0xFFFFFF)
                            } else {
                                color
                            }),
                    ),
            );
        }
        layer
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

            ed.dim_renders
                .iter()
                .map(|d| {
                    make_label(
                        window,
                        d.text.clone(),
                        d.label_cx,
                        d.label_cy,
                        rgb(t.bg_primary).into(),
                        rgb(t.accent).into(),
                    )
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
            let ed = editor.read(cx);
            let t = *crate::theme::active(cx);
            let pending = ed.pending_shape.map(|p| p.bounds());
            let pending_ruler = ed
                .pending_ruler
                .map(|p| p.snapped(ed.shift));
            let pending_line =
                ed.pending_line.map(|p| p.snapped(ed.shift));
            let list = paint::build_draw_list(
                &ed.doc,
                &ed.camera,
                bounds.size,
                t,
                pending,
                &ed.selection,
                ed.hover.filter(|_| ed.dragging.is_none()),
                &ed.dim_renders,
                &ed.snap_guides,
                ed.marquee,
                pending_ruler,
                pending_line,
            );
            (list, hitbox)
        };

        let paint = move |_: Bounds<Pixels>,
                          (list, hitbox): (Vec<paint::Primitive>, gpui::Hitbox),
                          window: &mut Window,
                          cx: &mut App| {
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
                                origin: Point { x: px(x), y: px(y) },
                                size: Size { width: px(w), height: px(h) },
                            },
                            color,
                        ));
                    }
                    paint::Primitive::Polygon { points, color } => {
                        if points.len() < 3 {
                            continue;
                        }
                        let to_px = |(x, y): (f32, f32)| Point { x: px(x), y: px(y) };
                        let mut path = gpui::Path::new(to_px(points[0]));
                        for &pt in &points[1..] {
                            path.line_to(to_px(pt));
                        }
                        path.line_to(to_px(points[0]));
                        window.paint_path(path, color);
                    }
                    paint::Primitive::Line { ax, ay, bx, by, width, color } => {
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
                            x: px(ax + nx),
                            y: px(ay + ny),
                        });
                        path.line_to(Point { x: px(bx + nx), y: px(by + ny) });
                        path.line_to(Point { x: px(bx - nx), y: px(by - ny) });
                        path.line_to(Point { x: px(ax - nx), y: px(ay - ny) });
                        path.line_to(Point { x: px(ax + nx), y: px(ay + ny) });
                        window.paint_path(path, color);
                    }
                    paint::Primitive::Outline { x, y, w, h } => {
                        window.paint_quad(gpui::quad(
                            Bounds {
                                origin: Point { x: px(x), y: px(y) },
                                size: Size { width: px(w), height: px(h) },
                            },
                            0.,
                            gpui::transparent_black(),
                            gpui::Edges::all(px(1.)),
                            rgb(crate::theme::active(cx).accent),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    paint::Primitive::Circle { cx: mcx, cy: mcy, radius } => {
                        let r = px(radius);
                        window.paint_quad(gpui::quad(
                            Bounds {
                                origin: Point { x: px(mcx) - r, y: px(mcy) - r },
                                size: Size { width: r * 2., height: r * 2. },
                            },
                            r,
                            rgb(0xFFFFFF),
                            gpui::Edges::all(px(1.)),
                            rgb(crate::theme::active(cx).accent),
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    paint::Primitive::RulerLabel { center_x, anchor_y, px_value, in_value } => {
                        // Two-row vector label centered on the inch tick,
                        // sitting entirely BEYOND the tick tips: pixels row
                        // on top (nearest the dashes), inches below it.
                        // Value in ink; unit suffix in empty_text_primary.
                        const SIZE: f32 = 9.;
                        const ROW_GAP: f32 = 2.;
                        let t = crate::theme::active(cx);
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
                            let line = window.text_system().shape_line(
                                text.into(),
                                px(SIZE),
                                &runs,
                                None,
                            );
                            // Center on the tick.
                            let origin_x = center_x - line.width.as_f32() / 2.;
                            let _ = line.paint(
                                Point { x: px(origin_x), y: px(top_y) },
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

        canvas(prepaint, paint)
            .absolute()
            .inset_0()
            .size_full()
    }
}

// Layout offset where the canvas starts below the title bar (for overlays).
pub const CANVAS_TOP_INSET: Pixels = px(TITLE_BAR_HEIGHT);








