use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, div,
    linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba,
};

use crate::editor::Editor;
use crate::theme::Theme;

const PICKER_WIDTH: Pixels = px(236.0);
const SURFACE_HEIGHT: Pixels = px(170.0);
const SLIDER_HEIGHT: Pixels = px(14.0);

#[derive(Clone, Copy)]
struct ColorState {
    h: f32,
    s: f32,
    v: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragTarget {
    Surface,
    Hue,
    Brightness,
}

/// Reusable inspector color picker.
///
/// Layout:
///
///     ┌──────────────────────────┐
///     │                          │
///     │       S × V surface      │
///     │            ●             │
///     │                          │
///     └──────────────────────────┘
///
///     ━━━━━━━━━━━━━●━━━━━━━━━━━━━  Hue
///
///     ━━━━━━━━━━━━━●━━━━━━━━━━━━━  Brightness
///
///     ● #RRGGBB
///
/// All three controls operate on the same HSV state.
pub(crate) fn render(
    editor: gpui::WeakEntity<Editor>,
    color: u32,
    t: Theme,
    _cx: &App,
) -> impl gpui::IntoElement {
    let (h, s, v) = rgb_to_hsv(color);

    // This is intentionally shared by all three controls.
    //
    // Since this picker is rendered from the current editor color,
    // the editor remains the source of truth. This small local state
    // is only used while dragging so the controls can move smoothly.
    let state = Rc::new(RefCell::new(ColorState { h, s, v }));
    let dragging = Rc::new(RefCell::new(None::<DragTarget>));

    let current_color = hsv_rgb(h, s, v);
    let pure_hue = hsv_rgb(h, 1.0, 1.0);

    // ============================================================
    // SATURATION / VALUE SURFACE
    // ============================================================

    let surface = {
        let state_down = state.clone(); let state_move = state.clone();
        let dragging_down = dragging.clone(); let dragging_move = dragging.clone();
        let dragging_up = dragging.clone(); let dragging_out = dragging.clone();
        let editor_down = editor.clone(); let editor_move = editor.clone();

        div()
            .id("color-picker-surface")
            .relative()
            .w(PICKER_WIDTH)
            .h(SURFACE_HEIGHT)
            .rounded(px(8.0))
            .overflow_hidden()
            .cursor_crosshair()
            // Base hue.
            .bg(rgb(pure_hue))
            // White -> transparent.
            //
            // X axis:
            //     0 = white
            //     1 = pure hue
            .child(div().absolute().inset_0().bg(linear_gradient(
                90.0,
                linear_color_stop(rgb(0xffffff), 0.0),
                linear_color_stop(rgba(0xffffff00), 1.0),
            )))
            // Transparent -> black.
            //
            // Y axis:
            //     0 = bright
            //     1 = black
            .child(div().absolute().inset_0().bg(linear_gradient(
                0.0,
                linear_color_stop(rgba(0x00000000), 0.0),
                linear_color_stop(rgb(0x000000), 1.0),
            )))
            // Selection cursor.
            .child(surface_cursor(s, v, t))
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                *dragging_down.borrow_mut() = Some(DragTarget::Surface);

                update_surface(event.position.x, event.position.y, &state_down, &editor_down, cx);
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                if *dragging_move.borrow() != Some(DragTarget::Surface) {
                    return;
                }

                update_surface(event.position.x, event.position.y, &state_move, &editor_move, cx);
            })
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, _| {
                *dragging_up.borrow_mut() = None;
            })
            .on_mouse_up_out(MouseButton::Left, move |_: &MouseUpEvent, _, _| {
                *dragging_out.borrow_mut() = None;
            })
    };

    // ============================================================
    // HUE SLIDER
    // ============================================================

    let hue_slider = {
        let state_down = state.clone(); let state_move = state.clone();
        let dragging_down = dragging.clone(); let dragging_move = dragging.clone();
        let dragging_up = dragging.clone(); let dragging_out = dragging.clone();
        let editor_down = editor.clone(); let editor_move = editor.clone();

        div()
            .id("color-picker-hue")
            .relative()
            .w(PICKER_WIDTH)
            .h(SLIDER_HEIGHT)
            .flex()
            .rounded(px(7.0))
            .overflow_hidden()
            .cursor_pointer()
            // We cannot give linear_gradient() six stops because
            // GPUI's API takes exactly two stops.
            //
            // Instead, make six flexible segments. Every segment
            // gets flex_1, so NOTHING here depends on a hardcoded
            // width.
            .child(hue_segment(0xff0000, 0xffff00))
            .child(hue_segment(0xffff00, 0x00ff00))
            .child(hue_segment(0x00ff00, 0x00ffff))
            .child(hue_segment(0x00ffff, 0x0000ff))
            .child(hue_segment(0x0000ff, 0xff00ff))
            .child(hue_segment(0xff00ff, 0xff0000))
            // Hue handle.
            .child(slider_handle(h, t))
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                *dragging_down.borrow_mut() = Some(DragTarget::Hue);

                update_horizontal_slider(event.position.x, &state_down, DragTarget::Hue, &editor_down, cx);
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                if *dragging_move.borrow() != Some(DragTarget::Hue) {
                    return;
                }

                update_horizontal_slider(event.position.x, &state_move, DragTarget::Hue, &editor_move, cx);
            })
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, _| {
                *dragging_up.borrow_mut() = None;
            })
            .on_mouse_up_out(MouseButton::Left, move |_: &MouseUpEvent, _, _| {
                *dragging_out.borrow_mut() = None;
            })
    };

    // ============================================================
    // BRIGHTNESS / VALUE SLIDER
    // ============================================================

    let brightness_slider = {
        let state_down = state.clone(); let state_move = state.clone();
        let dragging_down = dragging.clone(); let dragging_move = dragging.clone();
        let dragging_up = dragging.clone(); let dragging_out = dragging.clone();
        let editor_down = editor.clone(); let editor_move = editor.clone();

        div()
            .id("color-picker-brightness")
            .relative()
            .w(PICKER_WIDTH)
            .h(SLIDER_HEIGHT)
            .rounded(px(7.0))
            .overflow_hidden()
            .cursor_pointer()
            // Black -> current fully-bright color.
            //
            // Unlike your original picker, this is NOT black -> white.
            // It represents actual value/brightness for the selected hue
            // and saturation.
            .bg(linear_gradient(
                90.0,
                linear_color_stop(rgb(0x000000), 0.0),
                linear_color_stop(rgb(hsv_rgb(h, s, 1.0)), 1.0),
            ))
            .child(slider_handle(v, t))
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                *dragging_down.borrow_mut() = Some(DragTarget::Brightness);

                update_horizontal_slider(
                    event.position.x,
                    &state_down,
                    DragTarget::Brightness,
                    &editor_down,
                    cx,
                );
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                if *dragging_move.borrow() != Some(DragTarget::Brightness) {
                    return;
                }

                update_horizontal_slider(
                    event.position.x,
                    &state_move,
                    DragTarget::Brightness,
                    &editor_move,
                    cx,
                );
            })
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, _| {
                *dragging_up.borrow_mut() = None;
            })
            .on_mouse_up_out(MouseButton::Left, move |_: &MouseUpEvent, _, _| {
                *dragging_out.borrow_mut() = None;
            })
    };

    // ============================================================
    // COLOR SWATCH
    // ============================================================

    let swatch = div()
        .w(px(18.0))
        .h(px(18.0))
        .rounded(px(5.0))
        .bg(rgb(current_color))
        .border_1()
        .border_color(rgb(t.border_color));

    // ============================================================
    // POPUP
    // ============================================================

    div()
        .absolute()
        .right(px(8.0))
        .top(px(145.0))
        .w(px(256.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .p(px(10.0))
        .rounded(px(10.0))
        .bg(rgb(t.bg_tertiary))
        .border_1()
        .border_color(rgb(t.border_color))
        .shadow(vec![t.shadow_sm()])
        .child(surface)
        .child(hue_slider)
        .child(brightness_slider)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pt(px(2.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(swatch)
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(t.text_secondary))
                                .child("Color"),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(t.text_primary))
                        .child(format!("#{current_color:06X}")),
                ),
        )
}

// ================================================================
// HUE BAR
// ================================================================

fn hue_segment(from: u32, to: u32) -> impl gpui::IntoElement {
    div().flex_1().h_full().bg(linear_gradient(
        90.0,
        linear_color_stop(rgb(from), 0.0),
        linear_color_stop(rgb(to), 1.0),
    ))
}

// ================================================================
// SURFACE CURSOR
// ================================================================

fn surface_cursor(saturation: f32, value: f32, t: Theme) -> impl gpui::IntoElement {
    let x = PICKER_WIDTH * saturation;
    let y = SURFACE_HEIGHT * (1.0 - value);

    div()
        .absolute()
        .left(x - px(7.0))
        .top(y - px(7.0))
        .w(px(14.0))
        .h(px(14.0))
        .rounded(px(7.0))
        .border_2()
        .border_color(rgb(0xffffff))
        .shadow(vec![t.shadow_sm()])
}

// ================================================================
// SLIDER HANDLE
// ================================================================

fn slider_handle(position: f32, t: Theme) -> impl gpui::IntoElement {
    div()
        .absolute()
        .left(PICKER_WIDTH * position - px(5.0))
        .top(px(-2.0))
        .w(px(10.0))
        .h(SLIDER_HEIGHT + px(4.0))
        .rounded(px(5.0))
        .border_2()
        .border_color(rgb(0xffffff))
        .shadow(vec![t.shadow_sm()])
}

// ================================================================
// SURFACE INTERACTION
// ================================================================

fn update_surface(
    mouse_x: Pixels,
    mouse_y: Pixels,
    state: &Rc<RefCell<ColorState>>,
    editor: &gpui::WeakEntity<Editor>,
    cx: &mut gpui::App,
) {
    let saturation = (mouse_x / PICKER_WIDTH).clamp(0.0, 1.0);
    let value = 1.0 - (mouse_y / SURFACE_HEIGHT).clamp(0.0, 1.0);

    let mut state = state.borrow_mut();

    state.s = saturation;
    state.v = value;

    let color = hsv_rgb(state.h, state.s, state.v);

    let _ = editor.update(cx, |editor, cx| {
        editor.inspector_set_color(color, cx);
    });
}

// ================================================================
// HORIZONTAL SLIDER INTERACTION
// ================================================================

fn update_horizontal_slider(
    mouse_x: Pixels,
    state: &Rc<RefCell<ColorState>>,
    target: DragTarget,
    editor: &gpui::WeakEntity<Editor>,
    cx: &mut gpui::App,
) {
    let value = (mouse_x / PICKER_WIDTH).clamp(0.0, 1.0);

    let mut state = state.borrow_mut();

    match target {
        DragTarget::Hue => {
            state.h = value;
        }

        DragTarget::Brightness => {
            state.v = value;
        }

        DragTarget::Surface => {
            return;
        }
    }

    let color = hsv_rgb(state.h, state.s, state.v);

    let _ = editor.update(cx, |editor, cx| {
        editor.inspector_set_color(color, cx);
    });
}

// ================================================================
// RGB → HSV
// ================================================================

fn rgb_to_hsv(color: u32) -> (f32, f32, f32) {
    let r = ((color >> 16) & 0xff) as f32 / 255.0;
    let g = ((color >> 8) & 0xff) as f32 / 255.0;
    let b = (color & 0xff) as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let v = max;

    if delta <= f32::EPSILON {
        return (0.0, 0.0, v);
    }

    let s = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };

    let mut h = if (max - r).abs() < f32::EPSILON {
        (g - b) / delta
    } else if (max - g).abs() < f32::EPSILON {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };

    h /= 6.0;

    if h < 0.0 {
        h += 1.0;
    }

    (h, s, v)
}

// ================================================================
// HSV → RGB
// ================================================================

fn hsv_rgb(h: f32, s: f32, v: f32) -> u32 {
    let h = h.rem_euclid(1.0) * 6.0;

    let i = h.floor() as i32;
    let f = h - i as f32;

    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let z = v * (1.0 - (1.0 - f) * s);

    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, z, p),
        1 => (q, v, p),
        2 => (p, v, z),
        3 => (p, q, v),
        4 => (z, p, v),
        _ => (v, p, q),
    };

    ((r * 255.0).round() as u32) << 16
        | ((g * 255.0).round() as u32) << 8
        | (b * 255.0).round() as u32
}
