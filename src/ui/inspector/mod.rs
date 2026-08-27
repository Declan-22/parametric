use gpui::{App, IntoElement, RenderOnce, Window, div, prelude::*, px, rgb};

use crate::theme::Theme;

// Inspector: right-side panel of the design view. Layout only for now —
// sections (geometry, appearance, constraints, dimensions) land later.

pub const INSPECTOR_WIDTH: f32 = 260.0;

#[derive(IntoElement)]
pub struct Inspector {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for Inspector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);

        div()
            // Single column, flush against the far-right edge, full height
            // of the canvas area.
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(INSPECTOR_WIDTH))
            .flex()
            .flex_col()
            .py(px(4.))
            .px(px(4.))
            .gap(px(2.))
            .bg(rgb(t.bg_primary))
            // Left-edge border separates the panel from the canvas.
            .border_l_1()
            .border_color(rgb(t.component_border_color))
    }
}
