use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb};

use crate::theme::Theme;

// Inspector: right-side panel of the design view. When nothing is selected
// it shows canvas-level settings (stage-1: Show Grid + endless grid background).
// Later stages add Grid Size / Snap toggles in the same conditional section.

pub const INSPECTOR_WIDTH: f32 = 260.0;

#[derive(IntoElement)]
pub struct Inspector {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for Inspector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);

        // Snapshot editor state for this frame. Reading through `App`
        // subscribes this window to Editor notifications.
        let (has_selection, show_grid, grid_size, snap_to_grid, snap_to_objects) = self
            .editor
            .upgrade()
            .map(|e| {
                let ed = e.read(cx);
                (
                    !ed.selection.is_empty(),
                    ed.show_grid,
                    ed.grid_size,
                    ed.snap_to_grid,
                    ed.snap_to_objects,
                )
            })
            .unwrap_or((true, false, 20.0, false, true));

        let mut root = div()
            // Single column, flush against the far-right edge, full height
            // of the canvas area.
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(INSPECTOR_WIDTH))
            .flex()
            .flex_col()
            .py(px(8.))
            .px(px(8.))
            .gap(px(8.))
            .bg(rgb(t.bg_primary))
            // Left-edge border separates the panel from the canvas.
            .border_l_1()
            .border_color(rgb(t.component_border_color));

        if !has_selection {
            root = root.child(self.grid_section(
                t,
                show_grid,
                grid_size,
                snap_to_grid,
                snap_to_objects,
            ));
        }

        root
    }
}

impl Inspector {
    fn grid_section(
        &self,
        t: Theme,
        show_grid: bool,
        grid_size: f64,
        snap_to_grid: bool,
        snap_to_objects: bool,
    ) -> impl IntoElement {
        let editor_show = self.editor.clone();
        let editor_size_dec = self.editor.clone();
        let editor_size_inc = self.editor.clone();
        let editor_snap_grid = self.editor.clone();
        let editor_snap_obj = self.editor.clone();

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(8.))
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.component_border_color))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Grid"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.empty_text_primary))
                            .child(if show_grid { "On" } else { "Off" }),
                    ),
            )
            .child(div().h(px(1.)).w_full().bg(rgb(t.component_border_color)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(t.text_secondary))
                            .child("Show Grid"),
                    )
                    .child(self.toggle(
                        "show-grid",
                        show_grid,
                        editor_show,
                        t,
                        |ed| ed.show_grid = !ed.show_grid,
                    )),
            )
            .child(
                // Grid Size stepper
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(t.text_secondary))
                            .child("Grid Size"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .id("grid-size-dec")
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .bg(rgb(t.bg_tertiary))
                                    .border_1()
                                    .border_color(rgb(t.component_border_color))
                                    .text_sm()
                                    .text_color(rgb(t.text_primary))
                                    .cursor_pointer()
                                    .child("−")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |_, _, cx| {
                                            cx.stop_propagation();
                                            let _ = editor_size_dec.update(cx, |ed, cx| {
                                                ed.grid_size = (ed.grid_size - 5.0).max(5.0);
                                                cx.notify();
                                            });
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(42.))
                                    .h(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .bg(rgb(t.bg_primary))
                                    .border_1()
                                    .border_color(rgb(t.component_border_color))
                                    .text_sm()
                                    .text_color(rgb(t.text_primary))
                                    .child(format!("{:.0}", grid_size)),
                            )
                            .child(
                                div()
                                    .id("grid-size-inc")
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .bg(rgb(t.bg_tertiary))
                                    .border_1()
                                    .border_color(rgb(t.component_border_color))
                                    .text_sm()
                                    .text_color(rgb(t.text_primary))
                                    .cursor_pointer()
                                    .child("+")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |_, _, cx| {
                                            cx.stop_propagation();
                                            let _ = editor_size_inc.update(cx, |ed, cx| {
                                                ed.grid_size = (ed.grid_size + 5.0).min(100.0);
                                                cx.notify();
                                            });
                                        },
                                    ),
                            ),
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
                            .text_sm()
                            .text_color(rgb(t.text_secondary))
                            .child("Snap to Grid"),
                    )
                    .child(self.toggle(
                        "snap-to-grid",
                        snap_to_grid,
                        editor_snap_grid,
                        t,
                        |ed| ed.snap_to_grid = !ed.snap_to_grid,
                    )),
            );

        // Snap to Objects only visible when Snap to Grid is OFF — when Grid
        // is on, object snaps are completely disabled (spec).
        if !snap_to_grid {
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(t.text_secondary))
                            .child("Snap to Objects"),
                    )
                    .child(self.toggle(
                        "snap-to-objects",
                        snap_to_objects,
                        editor_snap_obj,
                        t,
                        |ed| ed.snap_to_objects = !ed.snap_to_objects,
                    )),
            );
        }

        section
    }

    fn toggle<F>(
        &self,
        id: &'static str,
        on: bool,
        editor: gpui::WeakEntity<crate::editor::Editor>,
        t: Theme,
        toggle_fn: F,
    ) -> impl IntoElement
    where
        F: Fn(&mut crate::editor::Editor) + 'static,
    {
        // Pill track: 36x20, thumb 16, 2px inner padding.
        // On = accent (like toolbar active), Off = bg_tertiary so it reads
        // as "inset" against the bg_secondary section.
        let bg = if on { t.accent } else { t.bg_tertiary };
        let border = if on {
            t.accent
        } else {
            t.component_border_color
        };

        div()
            .id(id)
            .w(px(36.))
            .h(px(20.))
            .rounded(px(10.))
            .bg(rgb(bg))
            .border_1()
            .border_color(rgb(border))
            .cursor_pointer()
            .p(px(2.))
            .flex()
            .items_center()
            .justify_start()
            .when(on, |d| d.justify_end())
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| {
                    toggle_fn(ed);
                    cx.notify();
                });
            })
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded(px(7.))
                    .bg(rgb(0xFFFFFF))
                    .shadow(vec![gpui::BoxShadow {
                        color: gpui::rgba(0x0000001A).into(),
                        offset: gpui::Point { x: px(0.), y: px(1.) },
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                        inset: false,
                    }]),
            )
    }
}
