use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    App, Bounds, IntoElement, MouseButton, Pixels, Point, RenderOnce, Size, WeakEntity, Window,
    canvas, div, fill, prelude::*, px, rgb, rgba, svg,
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
    pub designs: Vec<DesignMeta>,
    pub new_design_opacity: f32,
    pub renaming: Option<crate::ui::shell::RenameState>,
    pub caret_visible: bool,
}

impl RenderOnce for HomeView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let designs = self.designs;

        // Uniform grid: fit as many preferred-width cards as possible per
        // row, then stretch them all to the same width. Full rows and the
        // last row share identical card dimensions.
        const GAP: f32 = 16.;
        const PAD: f32 = 16.;
        let content_w = (window.viewport_size().width.as_f32() - PAD * 2.).max(CARD_WIDTH);
        let per_row = (((content_w + GAP) / (CARD_WIDTH + GAP)).floor() as i32).max(1) as f32;
        let card_w = (content_w - GAP * (per_row - 1.)) / per_row;

        let shell = self.shell.clone();
        let shell_hover = self.shell.clone();
        let new_opacity = self.new_design_opacity;
        let new_btn = div()
            .id("new-design")
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .h(px(32.))
            .rounded(px(8.))
            .cursor_pointer()
            .border_2()
            .border_color(rgb(t.accent_border))
            .shadow(vec![t.shadow_sm()])
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
            .child(
                svg()
                    .data(br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" /></svg>"#)
                    .w(px(14.))
                    .h(px(14.))
                    .mt(px(-1.))
                    .text_color(rgb(0xffffff)),
            )
            .child("New design");

        div()
            .id("home")
            .size_full()
            .overflow_y_scroll()
            .child(div().flex().justify_end().p(px(16.)).child(new_btn))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(16.))
                    .px(px(16.))
                    .children(
                        designs.into_iter().map(|meta| {
                            let is_renaming =
                                self.renaming.as_ref().map(|r| r.id) == Some(meta.id);
                            let rename_value = if is_renaming {
                                self.renaming
                                    .as_ref()
                                    .filter(|r| r.id == meta.id)
                                    .map(|r| r.value.clone())
                                    .unwrap_or_default()
                            } else {
                                String::new()
                            };
                            DesignCard {
                                meta,
                                shell: self.shell.clone(),
                                is_renaming,
                                caret_visible: self.caret_visible,
                                rename_value,
                                card_w,
                            }
                        }),
                    ),
            )
    }
}

#[derive(IntoElement)]
struct DesignCard {
    meta: DesignMeta,
    shell: WeakEntity<Shell>,
    is_renaming: bool,
    caret_visible: bool,
    rename_value: String,
    card_w: f32,
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
                        None, // pending
                        &[],  // selection
                        None, // hover
                        &[],  // dims
                        &[],  // angle dims
                        &[],  // snap guides
                        None, // marquee
                        None, // pending ruler
                        None, // pending line
                        &[],  // constraint markers
                        None, // pending circle
                        false, // show_grid (thumbnails never show grid)
                        crate::editor::Tool::Move,
                        None, // cursor_doc (no midpoint reveal in thumbnails)
                        None, // pending_pen (never mid-draw in thumbnails)
                    ),
                    None => Vec::new(),
                }
            };

        let paint_thumbs = move |bounds: Bounds<Pixels>,
                                 list: Vec<paint::Primitive>,
                                 window: &mut Window,
                                 _: &mut App| {
            // Convert canvas-local prim coords to window space. Without the
            // element origin, thumbnails painted at the window origin and
            // were clipped away by their cards (blank thumbnails).
            let (ox, oy) = (bounds.origin.x, bounds.origin.y);
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
                    paint::Primitive::Polygon { .. } => {}
                    paint::Primitive::Line { .. } => {}
                    paint::Primitive::Outline { .. } => {}
                    paint::Primitive::Circle { .. } => {}
                    paint::Primitive::RulerLabel { .. } => {}
                }
            }
        };

        let is_renaming = self.is_renaming;
        let shell_caret_visible = self.caret_visible;
        let rename_value = self.rename_value.clone();

        // 2px accent border fades in on hover; transparent at rest so the
        // layout never shifts. Alpha-only fade (no dark flash mid-tween).
        // The right-clicked card keeps its border while its menu is open.
        let k = self
            .shell
            .upgrade()
            .map(|s| {
                let s = s.read(cx);
                let menued = s.context_menu.as_ref().map(|m| m.id) == Some(meta_id);
                s.fade(&format!("card-{meta_id}"))
                    .max(if menued { 1.0 } else { 0.0 })
            })
            .unwrap_or(0.0);
        let card_border = crate::theme::fade_in((t.accent << 8) | 0xFF, k);

        div()
            .id(gpui::ElementId::NamedInteger(
                "design-card".into(),
                meta_id as u64,
            ))
            .w(px(self.card_w))
            .cursor_pointer()
            .p(px(5.))
            .rounded(px(10.))
            .bg(rgb(t.bg_secondary))
            .border_2()
            .border_color(rgba(card_border))
            .on_hover({
                let shell_hover = self.shell.clone();
                move |hovered, _, cx| {
                    let _ = shell_hover.update(cx, |shell, cx| {
                        shell.animate_fade(
                            &format!("card-{meta_id}"),
                            if *hovered { 1.0 } else { 0.0 },
                            cx,
                        );
                    });
                }
            })
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

const ICON_OPEN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 16.158V7.84c0-1.847 0-2.77.518-3.444c.517-.674 1.41-.912 3.194-1.387l3.508-.936A2.21 2.21 0 0 1 14 4.21v15.58a2.21 2.21 0 0 1-2.78 2.136l-3.508-.936c-1.785-.476-2.677-.714-3.194-1.387C4 18.928 4 18.005 4 16.158M11 11v2m6.5 7c.465 0 .697 0 .89-.04a2 2 0 0 0 1.572-1.57c.038-.194.038-.426.038-.89v-11c0-.465 0-.698-.038-.89a2 2 0 0 0-1.572-1.572c-.193-.039-.425-.039-.89-.039" /></svg>"#;
const ICON_RENAME: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 16 16">
	<path d="M0 0h16v16H0z" fill="none" />
	<path fill="currentColor" d="M6.5 2a.5.5 0 0 0 0 1h1v10h-1a.5.5 0 0 0 0 1h3a.5.5 0 0 0 0-1h-1V3h1a.5.5 0 0 0 0-1zM4 4h2.5v1H4a1 1 0 0 0-1 1v3.997a1 1 0 0 0 1 1h2.5v1H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2m8 6.997H9.5v1H12a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2H9.5v1H12a1 1 0 0 1 1 1v3.997a1 1 0 0 1-1 1" />
</svg>"#;
const ICON_DELETE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="m19.5 5.5l-.62 10.025c-.158 2.561-.237 3.842-.88 4.763a4 4 0 0 1-1.2 1.128c-.957.584-2.24.584-4.806.584c-2.57 0-3.855 0-4.814-.585a4 4 0 0 1-1.2-1.13c-.642-.922-.72-2.205-.874-4.77L4.5 5.5M3 5.5h18m-4.944 0l-.683-1.408c-.453-.936-.68-1.403-1.071-1.695a2 2 0 0 0-.275-.172C13.594 2 13.074 2 12.035 2c-1.066 0-1.599 0-2.04.234a2 2 0 0 0-.278.18c-.395.303-.616.788-1.058 1.757L8.053 5.5m1.447 11v-6m5 6v-6" /></svg>"#;

// Right-click context menu for a gallery card. Styling mirrors the app
// dropdown menu (bg_darker panel, entries highlight on hover).
pub fn render_context_menu(
    menu: &crate::ui::shell::DesignContextMenu,
    shell_entity: WeakEntity<Shell>,
    shell: &Shell,
    t: Theme,
    _cx: &App,
) -> impl IntoElement {
    use crate::theme::{fade_in, lerp_rgb};
    use gpui::{MouseDownEvent, SharedString, rgba};

    let menu_id = menu.id;
    let left = f32::from(menu.position.x);
    let top = f32::from(menu.position.y);

    let fade_of = |index: usize| -> f32 { shell.fade(&format!("ctx-{index}")) };

    let entry = |label: &'static str,
                 icon: &'static [u8],
                 index: usize,
                 destructive: bool|
     -> gpui::AnyElement {
        let shell_weak = shell_entity.clone();
        let k = fade_of(index);
        let bg = lerp_rgb(t.bg_darker, t.bg_tertiary, k);
        // Border is transparent at rest and fades in with the hover.
        // Alpha-only fade: lerping RGB from black causes a dark flash.
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        let mut shadow = t.shadow_sm();
        shadow.color = gpui::rgba(fade_in(t.item_shadow_color, k)).into();
        let fg = if destructive {
            rgb(0xE53E3E)
        } else {
            rgb(t.text_primary)
        };

        div()
            .id(SharedString::from(format!("ctx-{index}")))
            .flex()
            .items_center()
            .gap_x(px(4.))
            .h(px(26.))
            .px(px(4.))
            .rounded(px(6.))
            .text_sm()
            .text_color(fg)
            .cursor_pointer()
            .border_1()
            .border_color(rgba(border))
            .bg(rgb(bg))
            .shadow(vec![shadow])
            .on_hover({
                let shell_weak2 = shell_entity.clone();
                move |hovered, _, cx| {
                    let _ = shell_weak2.update(cx, |shell, cx| {
                        shell.animate_fade(
                            &format!("ctx-{index}"),
                            if *hovered { 1.0 } else { 0.0 },
                            cx,
                        );
                    });
                }
            })
            .on_mouse_down(MouseButton::Left, {
                let shell_weak2 = shell_entity.clone();
                move |_: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let id = menu_id;
                    let _ = shell_weak2.update(cx, |shell, cx| match label {
                        "Open" => {
                            shell.context_menu = None;
                            shell.open_design(id, cx);
                        }
                        "Rename" => shell.start_rename(id, window, cx),
                        "Delete" => shell.request_delete(id, cx),
                        _ => {}
                    });
                }
            })
            .child(svg().data(icon).w(px(13.)).h(px(13.)).text_color(fg))
            .child(label)
            .into_any_element()
    };

    div()
        .occlude()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(120.))
        .flex()
        .flex_col()
        .px(px(4.))
        .py(px(4.))
        .gap_y(px(2.))
        .bg(rgb(t.bg_darker))
        .border_1()
        .border_color(rgb(t.menu_border_color))
        .rounded(px(8.))
        .shadow(vec![t.shadow_sm()])
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(entry("Open", ICON_OPEN, 0, false))
        .child(entry("Rename", ICON_RENAME, 1, false))
        .child(entry("Delete", ICON_DELETE, 2, true))
}
