use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    App, Bounds, IntoElement, MouseButton, Pixels, RenderOnce, WeakEntity, Window, canvas, div,
    fill, prelude::*, px, rgb, rgba,
};

use crate::persistence::registry::DesignMeta;
use crate::theme::Theme;
use crate::ui::canvas::paint;
use crate::ui::shell::Shell;

pub const CARD_WIDTH: f32 = 224.;
pub const THUMB_HEIGHT: f32 = 132.;

// Home: a gallery of all designs. Nothing else.

#[derive(IntoElement)]
pub struct HomeView {
    pub shell: WeakEntity<Shell>,
}

impl RenderOnce for HomeView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let designs = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).designs(cx))
            .unwrap_or_default();

        let shell = self.shell.clone();
        let shell_hover = self.shell.clone();
        let new_opacity = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).new_design_opacity)
            .unwrap_or(1.0);
        let new_btn = div()
            .id("new-design")
            .flex()
            .items_center()
            .px(px(10.))
            .h(px(28.))
            .rounded(px(6.))
            .cursor_pointer()
            // High-contrast action button; hover fades it to 80% opacity.
            .bg(rgb(t.accent))
            .text_sm()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(rgb(0xffffff))
            .opacity(new_opacity)
            .on_hover(move |hovered, _, cx| {
                let _ = shell_hover.update(cx, |shell, cx| {
                    shell.animate_new_design(if *hovered { 0.8 } else { 1.0 }, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let _ = shell.update(cx, |shell, cx| shell.create_design(cx));
            })
            .child("+ New design");

        div()
            .id("home")
            .size_full()
            .overflow_y_scroll()
            .child(div().flex().justify_end().p(px(16.)).child(new_btn))
            .child(div().flex().flex_wrap().gap(px(16.)).px(px(16.)).children(
                designs.into_iter().map(|meta| DesignCard {
                    meta,
                    shell: self.shell.clone(),
                }),
            ))
    }
}

#[derive(IntoElement)]
struct DesignCard {
    meta: DesignMeta,
    shell: WeakEntity<Shell>,
}

impl RenderOnce for DesignCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let shell_click = self.shell.clone();
        let shell_thumb = self.shell.clone();
        let meta_id = self.meta.id;
        let meta_for_thumb = self.meta.clone();

        // Thumbnail: live mini-render of the design through a fit camera,
        // loaded lazily from the design's file.
        let prepaint =
            move |bounds: Bounds<Pixels>, _: &mut Window, cx: &mut App| -> Vec<paint::Primitive> {
                let Some(shell) = shell_thumb.upgrade() else {
                    return Vec::new();
                };
                let viewport = (bounds.size.width.as_f32(), THUMB_HEIGHT);
                let _ = shell.update(cx, |shell, _| {
                    shell.ensure_thumb(&meta_for_thumb, viewport);
                });
                let t = *crate::theme::active(cx);
                match shell.read(cx).thumb_snapshot(meta_id) {
                    Some((doc, camera)) => paint::build_draw_list(
                        &doc,
                        &camera,
                        bounds.size,
                        t,
                        None,
                        &[],
                        None,
                        &[],
                        None,
                        &[],
                        None,
                        None,
                    ),
                    None => Vec::new(),
                }
            };

        let paint_thumbs = move |_: Bounds<Pixels>,
                                 list: Vec<paint::Primitive>,
                                 window: &mut Window,
                                 _: &mut App| {
            for prim in list {
                match prim {
                    paint::Primitive::Rect { bounds, color } => {
                        window.paint_quad(fill(bounds, color));
                    }
                    paint::Primitive::Outline { bounds: _ } => {}
                    paint::Primitive::Circle { .. } => {}
                    paint::Primitive::CornerHandle { .. } => {}
                }
            }
        };

        let is_renaming = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).renaming.as_ref().map(|r| r.id) == Some(meta_id))
            .unwrap_or(false);
        let shell_caret_visible = if is_renaming {
            self.shell
                .upgrade()
                .map(|s| s.read(cx).caret_visible)
                .unwrap_or(true)
        } else {
            false
        };
        let rename_value = if is_renaming {
            self.shell
                .upgrade()
                .and_then(|s| {
                    s.read(cx)
                        .renaming
                        .as_ref()
                        .filter(|r| r.id == meta_id)
                        .map(|r| r.value.clone())
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        div()
            .id(gpui::ElementId::NamedInteger(
                "design-card".into(),
                meta_id as u64,
            ))
            .w(px(CARD_WIDTH))
            .cursor_pointer()
            .p(px(5.))
            .rounded(px(10.))
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.component_border_color))
            // Constant border; no hover restyle, just the pointer cursor.
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let id = meta_id;
                let _ = shell_click.update(cx, |shell, cx| shell.open_design(id, cx));
            })
            .on_mouse_down(MouseButton::Right, {
                let shell = self.shell.clone();
                move |e: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let pos = e.position;
                    let _ = shell.update(cx, |shell, cx| {
                        shell.open_context_menu(meta_id, pos);
                        cx.notify();
                    });
                }
            })
            .child(
                // Name + edited-ago above the thumbnail. The name slot keeps
                // identical geometry whether idle or focused so entering
                // rename doesn't shift anything.
                div()
                    .px(px(2.))
                    .pb(px(6.))
                    .child(if is_renaming {
                        div()
                            .flex()
                            .items_center()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .bg(rgb(t.bg_primary))
                            .border_2()
                            .border_color(rgb(t.accent))
                            .rounded(px(4.))
                            .px(px(3.))
                            .h(px(22.))
                            .cursor_text()
                            .child(rename_value)
                            // Blinking caret at the end of the text.
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(14.))
                                    .ml(px(0.5))
                                    .when(shell_caret_visible, |d| d.bg(rgb(t.text_primary))),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .flex()
                            .items_center()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .border_2()
                            .border_color(rgba(0x00000000))
                            .rounded(px(4.))
                            .px(px(3.))
                            .h(px(22.))
                            .child(self.meta.name.clone())
                            .into_any_element()
                    })
                    .child(
                        div()
                            .mt(px(2.))
                            .text_xs()
                            .text_color(rgb(t.text_secondary))
                            .child(edited_ago(self.meta.updated_at)),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .rounded(px(8.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(t.component_border_color))
                    .bg(rgb(t.bg_primary))
                    .child(canvas(prepaint, paint_thumbs).w_full().h(px(THUMB_HEIGHT))),
            )
    }
}

fn edited_ago(updated_at: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now - updated_at).max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        86400..=2591999 => format!("{}d ago", secs / 86400),
        _ => format!("{}mo ago", secs / 2592000),
    }
}

// Right-click context menu for a gallery card. Styling mirrors the app
// dropdown menu (bg_darker panel, bordered hover entries).
pub fn render_context_menu(
    menu: &crate::ui::shell::DesignContextMenu,
    shell: WeakEntity<Shell>,
    t: Theme,
    cx: &App,
) -> impl IntoElement {
    use crate::theme::{fade_in, lerp_rgb, lerp_rgba};
    use gpui::{MouseDownEvent, SharedString, rgba};

    let menu_id = menu.id;
    let left = f32::from(menu.position.x);
    let top = f32::from(menu.position.y);

    let fade_of = |index: usize| -> f32 {
        shell
            .upgrade()
            .map(|s| s.read(cx).fade(&format!("ctx-{index}")))
            .unwrap_or(0.0)
    };

    let entry = |label: &'static str, index: usize| -> gpui::AnyElement {
        let shell = shell.clone();
        let k = fade_of(index);
        let bg = lerp_rgb(t.bg_darker, t.bg_tertiary, k);
        let border = lerp_rgba(
            (t.component_border_color << 8) | 0xFF,
            (t.border_color << 8) | 0xFF,
            k,
        );
        let mut shadow = t.shadow_sm();
        shadow.color = gpui::rgba(fade_in(t.item_shadow_color, k)).into();

        div()
            .id(SharedString::from(format!("ctx-{index}")))
            .flex()
            .items_center()
            .h(px(26.))
            .px(px(10.))
            .rounded(px(6.))
            .text_sm()
            .text_color(rgb(t.text_primary))
            .cursor_pointer()
            .border_1()
            .border_color(rgba(border))
            .bg(rgb(bg))
            .shadow(vec![shadow])
            .on_hover({
                let shell = shell.clone();
                move |hovered, _, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.animate_fade(
                            &format!("ctx-{index}"),
                            if *hovered { 1.0 } else { 0.0 },
                            cx,
                        );
                    });
                }
            })
            .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let id = menu_id;
                let _ = shell.update(cx, |shell, cx| match label {
                    "Open" => {
                        shell.context_menu = None;
                        shell.open_design(id, cx);
                    }
                    "Rename" => shell.start_rename(id, window, cx),
                    _ => {}
                });
            })
            .child(label)
            .into_any_element()
    };

    div()
        .occlude()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(160.))
        .flex()
        .flex_col()
        .px(px(4.))
        .py(px(4.))
        .gap_y(px(2.))
        .bg(rgb(t.bg_darker))
        .border_1()
        .border_color(rgb(t.component_border_color))
        .rounded(px(8.))
        .shadow(vec![t.shadow_sm()])
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(entry("Open", 0))
        .child(entry("Rename", 1))
}
