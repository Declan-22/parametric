use gpui::{prelude::*, px, rgb};

use crate::theme::Theme;

// Tooltip: bg_tertiary, 1px border_color, shadow_sm, gap 6px between label and shortcut.
pub fn tooltip(t: Theme, label: &str, shortcut: &str, k: f32) -> impl gpui::IntoElement {
    gpui::div()
        .absolute()
        .left(px(42.))
        .top(px(6.))
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(6.))
        .py(px(2.))
        .bg(rgb(t.bg_tertiary))
        .border_1()
        .border_color(rgb(t.border_color))
        .rounded(px(8.))
        .shadow(vec![t.shadow_md()])
        .opacity(k)
        .when(k < 0.01, |d| d.hidden())
        .child(
            gpui::div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child(label.to_string()),
        )
        .child(
            gpui::div()
                .text_xs()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(t.empty_text_primary))
                .child(shortcut.to_string()),
        )
}
