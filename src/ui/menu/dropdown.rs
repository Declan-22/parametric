use gpui::{
    App, FontWeight, MouseButton, Point, SharedString, WeakEntity, Window, div, point, prelude::*,
    px, rgb, rgba,
};

use crate::theme::Theme;
use crate::ui::menu::{Menu, MenuItem, default_menus};
use crate::ui::shell::Shell;
use crate::ui::shell::title_bar::TITLE_BAR_HEIGHT;

pub const MENU_LEFT: f32 = 6.0;
pub const PANEL_WIDTH: f32 = 200.0;
pub const SUBMENU_WIDTH: f32 = 232.0;
pub const ENTRY_HEIGHT: f32 = 26.0;
pub const SEPARATOR_HEIGHT: f32 = 11.0;
pub const PANEL_PADDING_Y: f32 = 4.0;
pub const PANEL_PADDING_X: f32 = 4.0;
const SUBMENU_OVERLAP: f32 = 1.0;
const ITEM_GAP: f32 = 2.0;

#[derive(IntoElement)]
pub struct AppMenu {
    pub shell: WeakEntity<Shell>,
    pub active: Option<usize>,
    pub animation: f32,
}

impl RenderOnce for AppMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let menus = default_menus();
        let opacity = self.animation.min(1.0);

        div()
            .absolute()
            .top(px(TITLE_BAR_HEIGHT as f32 + 2.0))
            .left(px(MENU_LEFT))
            .child(self.render_panel(&menus, t, opacity, cx))
            .children(
                self.active
                    .and_then(|i| menus.get(i).map(|menu| (i, menu)))
                    .map(|(i, menu)| render_submenu(i, menu, t, opacity, self.shell.clone(), cx)),
            )
    }
}

impl AppMenu {
    fn render_panel(&self, menus: &[Menu], t: Theme, opacity: f32, cx: &App) -> impl IntoElement {
        div()
            .occlude()
            .w(px(PANEL_WIDTH))
            .flex()
            .flex_col()
            .px(px(PANEL_PADDING_X))
            .py(px(PANEL_PADDING_Y))
            .gap_y(px(ITEM_GAP))
            .bg(rgb(t.bg_darker))
            .border_1()
            .border_color(rgb(t.component_border_color))
            .rounded(px(8.))
            .shadow(vec![t.shadow_sm()])
            .opacity(opacity)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move({
                let shell = self.shell.clone();
                move |event, _, cx| {
                    let _ = shell.update(cx, |shell, _| shell.record_cursor(event.position));
                }
            })
            .children(
                menus
                    .iter()
                    .enumerate()
                    .map(|(i, menu)| self.render_entry(i, menu, t, cx)),
            )
    }

    fn render_entry(&self, index: usize, menu: &Menu, t: Theme, cx: &App) -> impl IntoElement {
        use crate::theme::{fade_in, lerp_rgb};

        let is_active = self.active == Some(index);
        let shell = self.shell.clone();

        // Hover tween blended over the active state.
        let hov = shell
            .upgrade()
            .map(|s| s.read(cx).fade(&format!("menu-entry-{index}")))
            .unwrap_or(0.0);
        let k = hov.max(if is_active { 1.0 } else { 0.0 });
        let bg = lerp_rgb(t.bg_darker, t.bg_tertiary, k);
        // Alpha-only fade: lerping RGB from black causes a dark flash.
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        let mut shadow = t.shadow_sm();
        shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();

        div()
            .id(SharedString::from(format!("app-menu-entry-{index}")))
            .flex()
            .items_center()
            .justify_between()
            .h(px(ENTRY_HEIGHT))
            .px(px(10.))
            .rounded(px(6.))
            .text_sm()
            .text_color(rgb(t.text_primary))
            .cursor_pointer()
            .bg(rgb(bg))
            .border_1()
            .border_color(rgba(border))
            .shadow(vec![shadow])
            .on_hover(move |hovered, window, cx| {
                let cursor = window.mouse_position();
                // Drive the hover fade tween alongside the menu-aim logic.
                let fade_target = if *hovered { 1.0 } else { 0.0 };
                let mut suppressed = false;
                let _ = shell.update(cx, |shell, cx| {
                    shell.animate_fade(&format!("menu-entry-{index}"), fade_target, cx);
                    shell.record_cursor(cursor);
                    shell.hovered_entry = if *hovered {
                        Some(index)
                    } else if shell.hovered_entry == Some(index) {
                        None
                    } else {
                        shell.hovered_entry
                    };
                    if !*hovered || !shell.menu_open || shell.active_menu == Some(index) {
                        return;
                    }
                    // Safe triangle only applies while the cursor is still
                    // traveling toward the open submenu; moving back toward
                    // the list switches immediately (menu-aim intent).
                    if let Some(active) = shell.active_menu {
                        if cursor_in_safe_triangle(Some(active), cursor)
                            && moving_toward_submenu(&shell.cursor_velocity())
                        {
                            suppressed = true;
                            return;
                        }
                    }
                    shell.set_active_menu(Some(index), cx);
                });

                // Dwell intent: if we were suppressed by the safe triangle
                // but the user keeps resting on this entry, switch anyway.
                if suppressed {
                    let dwell_shell = shell.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(280))
                            .await;
                        let _ = dwell_shell.update(cx, |shell, cx| {
                            if shell.menu_open
                                && shell.hovered_entry == Some(index)
                                && shell.active_menu != Some(index)
                            {
                                shell.set_active_menu(Some(index), cx);
                            }
                        });
                    })
                    .detach();
                }
            })
            .on_mouse_down(MouseButton::Left, {
                let shell = self.shell.clone();
                move |_, _, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.set_active_menu(Some(index), cx);
                        cx.notify();
                    });
                }
            })
            .child(menu.label.clone())
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(t.text_secondary))
                    .child("\u{203A}"),
            )
    }
}

fn render_submenu(
    index: usize,
    menu: &Menu,
    t: Theme,
    opacity: f32,
    shell: WeakEntity<Shell>,
    cx: &App,
) -> impl IntoElement {
    let placement = submenu_placement(index, menu);

    div()
        .occlude()
        .absolute()
        .left(px(PANEL_WIDTH - SUBMENU_OVERLAP))
        .top(px(placement.rel_top))
        .w(px(SUBMENU_WIDTH))
        .flex()
        .flex_col()
        .px(px(PANEL_PADDING_X))
        .py(px(PANEL_PADDING_Y))
        .gap_y(px(ITEM_GAP))
        .bg(rgb(t.bg_darker))
        .border_1()
        .border_color(rgb(t.component_border_color))
        .rounded(px(8.))
        .shadow(vec![t.shadow_sm()])
        .opacity(opacity)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_move({
            let shell = shell.clone();
            move |event, _, cx| {
                let _ = shell.update(cx, |shell, _| shell.record_cursor(event.position));
            }
        })
        .children(
            menu.items
                .iter()
                .enumerate()
                .map(|(i, item)| render_item(item, i, t, shell.clone(), cx)),
        )
}

fn render_item(
    item: &MenuItem,
    index: usize,
    t: Theme,
    shell: WeakEntity<Shell>,
    cx: &App,
) -> impl IntoElement {
    use crate::theme::{fade_in, lerp_rgb};

    match item {
        MenuItem::Separator => div()
            .h(px(1.))
            .mx(px(10.))
            .my(px(5.))
            .bg(rgb(t.component_border_color))
            .into_any_element(),
        MenuItem::Entry(entry) => {
            let action = entry.action.boxed_clone();
            let k = shell
                .upgrade()
                .map(|s| s.read(cx).fade(&format!("submenu-{index}")))
                .unwrap_or(0.0);
            let bg = lerp_rgb(t.bg_darker, t.bg_tertiary, k);
            let border = fade_in((t.border_color << 8) | 0xFF, k);
            let mut shadow = t.shadow_sm();
            shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();

            div()
                .id(SharedString::from(format!("submenu-item-{index}")))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .h(px(ENTRY_HEIGHT))
                .px(px(10.))
                .rounded(px(6.))
                .text_sm()
                .text_color(rgb(t.text_primary))
                .cursor_pointer()
                .bg(rgb(bg))
                .border_1()
                .border_color(rgba(border))
                .shadow(vec![shadow])
                .on_hover({
                    let shell = shell.clone();
                    move |hovered, _, cx| {
                        let _ = shell.update(cx, |shell, cx| {
                            shell.animate_fade(
                                &format!("submenu-{index}"),
                                if *hovered { 1.0 } else { 0.0 },
                                cx,
                            );
                        });
                    }
                })
                .on_click(move |_, window, cx| {
                    window.dispatch_action(action.boxed_clone(), cx);
                    let _ = shell.update(cx, |shell, cx| shell.close_menu(cx));
                })
                .child(entry.label.clone())
                .children(entry.shortcut.clone().map(|shortcut| {
                    div()
                        .pl(px(24.))
                        .text_xs()
                        .font_family(crate::theme::FONT_UI)
                        .text_color(rgb(t.empty_text_primary))
                        .child(shortcut)
                }))
                .into_any_element()
        }
    }
}

struct SubmenuPlacement {
    rel_top: f32,
    height: f32,
}

fn submenu_placement(index: usize, menu: &Menu) -> SubmenuPlacement {
    let rel_top = PANEL_PADDING_Y + index as f32 * (ENTRY_HEIGHT + ITEM_GAP);
    let item_heights: Vec<f32> = menu
        .items
        .iter()
        .map(|item| match item {
            MenuItem::Separator => SEPARATOR_HEIGHT,
            MenuItem::Entry(_) => ENTRY_HEIGHT,
        })
        .collect();
    let gaps = item_heights.len().saturating_sub(1) as f32 * ITEM_GAP;
    SubmenuPlacement {
        rel_top,
        height: PANEL_PADDING_Y * 2. + item_heights.iter().sum::<f32>() + gaps,
    }
}

// Safe triangle: from the midpoint of the open entry down to the bottom-left
// corner of its submenu. Diagonal travel inside this region must not switch
// submenus, so fast mouse movement toward deep items doesn't flicker.
fn cursor_in_safe_triangle(active: Option<usize>, cursor: Point<gpui::Pixels>) -> bool {
    let Some(active) = active else {
        return false;
    };
    let menus = default_menus();
    let Some(menu) = menus.get(active) else {
        return false;
    };
    let placement = submenu_placement(active, menu);

    let row_cy = TITLE_BAR_HEIGHT + placement.rel_top + ENTRY_HEIGHT / 2.;
    let m = point(px(MENU_LEFT + PANEL_WIDTH / 2.), px(row_cy));
    let rb = point(
        px(MENU_LEFT + PANEL_WIDTH),
        px(TITLE_BAR_HEIGHT + placement.rel_top + ENTRY_HEIGHT),
    );
    let sb = point(
        px(MENU_LEFT + PANEL_WIDTH - SUBMENU_OVERLAP),
        px(TITLE_BAR_HEIGHT + placement.rel_top + placement.height),
    );

    point_in_triangle(cursor, m, rb, sb)
}

// Suppress switching only while the cursor is genuinely traveling toward
// the submenu (rightward). Paused or leftward/upward movement means the
// user is heading back to the list, so rows switch normally.
fn moving_toward_submenu(velocity: &(f32, f32)) -> bool {
    let (vx, vy) = *velocity;
    vx > 0.15 && (vx.abs() + vy.abs()) > 0.2
}

fn point_in_triangle(
    p: Point<gpui::Pixels>,
    a: Point<gpui::Pixels>,
    b: Point<gpui::Pixels>,
    c: Point<gpui::Pixels>,
) -> bool {
    let cross = |o: Point<gpui::Pixels>, u: Point<gpui::Pixels>| {
        f32::from(o.x) * f32::from(u.y) - f32::from(o.y) * f32::from(u.x)
    };
    let v0 = c - a;
    let v1 = b - a;
    let v2 = p - a;

    let denom = cross(v0, v1);
    if denom.abs() < f32::EPSILON {
        return false;
    }
    let l1 = cross(v2, v1) / denom;
    let l2 = cross(v0, v2) / denom;
    let l3 = 1. - l1 - l2;

    const TOL: f32 = 0.04;
    l1 >= -TOL && l2 >= -TOL && l3 >= -TOL
}
