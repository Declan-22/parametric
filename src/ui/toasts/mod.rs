use std::time::Duration;

use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, svg};

use crate::theme::Theme;
use crate::ui::inspector::INSPECTOR_WIDTH;

// Toast notification stack: bottom-right over the canvas, left of the
// inspector. The container's right edge matches the floating menu exactly
// (`INSPECTOR_WIDTH + 12`), it just docks to the bottom instead of the top.
// Styling mirrors the floating menu: bg_tertiary, 1px border_color,
// shadow_sm, 14px rounding. Entry is a rise+fade (ease-out-cubic) driven
// per-toast by `Shell::toast_anim`; dismissal replays it in reverse from the
// cached value before the entry is removed, so stacked toasts never pop.
//
// Reusable for anything (not just over-constraints): push via
// `Shell::push_toast(kind, title, body, hint, blocks, cx)`. Errors persist
// ~10s, infos ~6s — long enough to read the explanation. Identical pushes
// re-arm the existing toast's timer instead of stacking duplicates.

/// Reusable toast severity. Error is the over-constraint path; Info covers
/// future non-blocking notices (saves, merges, hints).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Error,
    Info,
}

impl ToastKind {
    /// How long the toast stays up before auto-dismissing. Errors carry
    /// multi-sentence conflict explanations, so they dwell a full 10s;
    /// infos get 6s.
    pub fn duration(self) -> Duration {
        match self {
            ToastKind::Error => Duration::from_millis(10_000),
            ToastKind::Info => Duration::from_millis(6000),
        }
    }

    /// Severity color, painted as the flat left-edge bar.
    fn edge(self) -> u32 {
        match self {
            ToastKind::Error => 0xE53E3E,
            ToastKind::Info => crate::theme::theme::ACCENT,
        }
    }
}

/// One live toast owned by `Shell` (see `ui::shell`).
#[derive(Clone)]
pub(crate) struct ToastEntry {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub body: String,
    pub hint: String,
    pub blocks: Vec<crate::editor::BlockChip>,
    /// 0..1 entry progress (1 = fully in; dismissal tweens back to 0).
    pub anim: f32,
    /// Tween generation so re-armed/expired timers never fight.
    pub tween: u64,
    /// Expiry generation: re-arming the timer bumps this so the stale
    /// sleeper becomes a no-op.
    pub epoch: u64,
    pub dismissing: bool,
}

/// Blocker-chip glyph per lock kind. Reuses the canvas constraint-chip
/// SVGs (plus the dimension glyph) with the exact same
/// `svg().data().text_color()` pattern, so they tint identically.
fn lock_icon(kind: crate::editor::LockKind) -> &'static [u8] {
    use crate::editor::LockKind as LK;
    match kind {
        LK::Horizontal => crate::ui::canvas::ICON_CHIP_HORIZONTAL,
        LK::Vertical => crate::ui::canvas::ICON_CHIP_VERTICAL,
        LK::Coincident => crate::ui::canvas::ICON_CHIP_COINCIDENT,
        LK::Tangent => crate::ui::canvas::ICON_CHIP_TANGENT,
        LK::Parallel => crate::ui::canvas::ICON_CHIP_PARALLEL,
        LK::Perpendicular => crate::ui::canvas::ICON_CHIP_PERPENDICULAR,
        LK::Dimension => crate::ui::toolbar::ICON_DIMENSION,
    }
}

pub(crate) const TOAST_WIDTH: f32 = 320.0;

// Accent-strip end caps: 6x9 shapes whose outer ends are cut along the
// card's own 14px corner circles (center (14,14) r=14 in card-outer
// coords, strip sitting 1px in for the border). Fixed size, never
// scaled, so the arcs stay exact. Top cap arc runs (0,7.8)->(6,0.88);
// bottom is its vertical mirror.
const CAP_TOP_RED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="9" viewBox="0 0 6 9"><path d="M0 9 L0 7.8 A14 14 0 0 1 6 0.88 L6 9 Z" fill="#E53E3E"/></svg>"##;
const CAP_BOTTOM_RED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="9" viewBox="0 0 6 9"><path d="M0 0 L6 0 L6 8.12 A14 14 0 0 0 0 0.2 Z" fill="#E53E3E"/></svg>"##;
const CAP_TOP_BLUE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="9" viewBox="0 0 6 9"><path d="M0 9 L0 7.8 A14 14 0 0 1 6 0.88 L6 9 Z" fill="#4C8DFF"/></svg>"##;
const CAP_BOTTOM_BLUE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="9" viewBox="0 0 6 9"><path d="M0 0 L6 0 L6 8.12 A14 14 0 0 0 0 0.2 Z" fill="#4C8DFF"/></svg>"##;

#[derive(IntoElement)]
pub struct Toasts {
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for Toasts {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let entries: Vec<ToastEntry> = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).toasts.clone())
            .unwrap_or_default();
        if entries.is_empty() {
            return div().absolute().into_any_element();
        }
        let mut stack = div()
            .absolute()
            .bottom(px(12.))
            // Right edge aligns exactly with the floating menu's right
            // edge (`INSPECTOR_WIDTH + 12`); wider body for explanations.
            .right(px(INSPECTOR_WIDTH + 12.))
            .w(px(TOAST_WIDTH))
            .flex()
            .flex_col()
            .gap(px(8.))
            // NOTE: load-bearing id — without it the stack has no hitbox
            // and clicks fall through to the canvas behind.
            .id(gpui::SharedString::from("toast-stack"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for entry in entries {
            stack = stack.child(toast_card(&self.shell, t, entry));
        }
        stack.into_any_element()
    }
}

fn toast_card(
    shell: &gpui::WeakEntity<crate::ui::shell::Shell>,
    t: Theme,
    entry: ToastEntry,
) -> impl IntoElement {
    let a = entry.anim.clamp(0., 1.);
    let pop = 1f32 - (1f32 - a).powi(3);
    let rise = 8. * (1. - pop);
    let shell_close = shell.clone();
    let id = entry.id;
    let edge = entry.kind.edge();
    let (cap_top, cap_bottom) = match entry.kind {
        ToastKind::Error => (CAP_TOP_RED, CAP_BOTTOM_RED),
        ToastKind::Info => (CAP_TOP_BLUE, CAP_BOTTOM_BLUE),
    };
    let key = format!("toast-close-{id}");
    div()
        .id(gpui::SharedString::from(format!("toast-{id}")))
        .relative()
        .top(px(rise))
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        // Left room for the flat severity bar (6px) + breathing space.
        .pl(px(18.))
        .pr(px(10.))
        .py(px(10.))
        .rounded(px(14.))
        .overflow_hidden()
        .bg(rgb(t.bg_tertiary))
        .border_1()
        .border_color(rgb(t.border_color))
        .shadow(vec![t.shadow_sm()])
        .opacity(pop)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Severity as a flat 6px strip down the whole left edge — a plain
        // shape, deliberately NOT a border. Neither `overflow_hidden` nor
        // per-corner radii can crop it to the card's rounding in this
        // GPUI version (masks are rectangular-only; radii get clamped to
        // the 6px strip width), so the strip's ends are exact SVG arcs
        // cut along the card's own 14px corner circles — true cropping
        // by construction, with a plain rect filling the middle.
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(6.))
                .flex()
                .flex_col()
                .child(
                    svg()
                        .data(cap_top)
                        .w(px(6.))
                        .h(px(9.))
                        .text_color(rgb(edge)),
                )
                .child(div().flex_1().w_full().bg(rgb(edge)))
                .child(
                    svg()
                        .data(cap_bottom)
                        .w(px(6.))
                        .h(px(9.))
                        .text_color(rgb(edge)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(entry.title.clone()),
                )
                .child(
                    div()
                        .id(gpui::SharedString::from(key))
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .text_xs()
                        .text_color(rgb(t.text_secondary))
                        .cursor_pointer()
                        .child("×")
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = shell_close.update(cx, |shell, cx| {
                                    shell.dismiss_toast(id, cx);
                                });
                            },
                        ),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_secondary))
                .child(entry.body.clone()),
        )
        .when(!entry.blocks.is_empty(), |card| {
            let mut chips = div().flex().flex_col().gap(px(4.)).w_full();
            for chip in &entry.blocks {
                chips = chips.child(block_chip(t, chip));
            }
            card.child(chips)
        })
        .when(!entry.hint.is_empty(), |card| {
            card.child(
                div()
                    .text_xs()
                    .text_color(rgb(t.text_secondary))
                    .child(format!("→ {}", entry.hint.clone())),
            )
        })
}

/// One blocker chip: a small container with the lock-kind glyph and a
/// compact label. Full-width rows so a short list of blockers scans
/// like a list, not a paragraph.
fn block_chip(t: Theme, chip: &crate::editor::BlockChip) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .h(px(24.))
        .w_full()
        .rounded(px(7.))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.component_border_color))
        .child(
            svg()
                .data(lock_icon(chip.kind))
                .w(px(12.))
                .h(px(12.))
                .text_color(rgb(t.text_secondary)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_primary))
                .child(chip.label.clone()),
        )
}
