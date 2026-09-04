use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb};

use crate::theme::{Theme, lerp_rgb};

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
        let (has_selection, show_grid, snap_to_grid, snap_to_objects) = self
            .editor
            .upgrade()
            .map(|e| {
                let ed = e.read(cx);
                (
                    !ed.selection.is_empty(),
                    ed.show_grid,
                    ed.snap_to_grid,
                    ed.snap_to_objects,
                )
            })
            .unwrap_or((true, false, false, true));

        let mut root = div()
            // Single column, flush against the far-right edge, full height
            // of the canvas area.
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(INSPECTOR_WIDTH))
            // No horizontal padding here: rows pad themselves so the
            // section dividers can span the full inspector width.
            .flex()
            .flex_col()
            .py(px(8.))
            .gap(px(8.))
            .bg(rgb(t.bg_primary))
            // Left-edge border separates the panel from the canvas.
            .border_l_1()
            .border_color(rgb(t.component_border_color));

        // Seed toggle tweens at their live values so the first paint is
        // exact (fade() defaults missing keys to 0). Reading the shell
        // below subscribes this view to it, so tween ticks repaint us.
        if let Some(shell) = self.shell.upgrade() {
            let _ = shell.update(cx, |shell, _| {
                for (key, on) in [
                    ("inspector-show-grid", show_grid),
                    ("inspector-snap-to-grid", snap_to_grid),
                    ("inspector-snap-to-objects", snap_to_objects),
                ] {
                    shell
                        .fades
                        .entry(key.to_string())
                        .or_insert(if on { 1.0 } else { 0.0 });
                }
            });
        }

        if !has_selection {
            root = root.child(self.grid_section(
                t,
                show_grid,
                snap_to_grid,
                snap_to_objects,
                cx,
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
        snap_to_grid: bool,
        snap_to_objects: bool,
        cx: &App,
    ) -> impl IntoElement {
        let editor_show = self.editor.clone();
        let editor_snap_grid = self.editor.clone();
        let editor_snap_obj = self.editor.clone();
        let shell_show = self.shell.clone();
        let shell_snap_grid = self.shell.clone();
        let shell_snap_obj = self.shell.clone();
        // Tween positions (0 = off, 1 = on). Reading subscribes us to the
        // shell so each animation tick repaints.
        let fade = |key: &str| {
            self.shell
                .upgrade()
                .map(|s| s.read(cx).fade(key))
                .unwrap_or(0.0)
        };

        // Flat sections, no container: dividers span the full inspector
        // width (the root carries no horizontal padding; rows pad
        // themselves).
        let divider = || div().h(px(1.)).w_full().bg(rgb(t.component_border_color));
        let header = |label: &'static str, on: bool| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_secondary))
                        .child(label),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(header("Grid", show_grid))
            .child(Self::row(
                "inspector-row-show-grid",
                "inspector-show-grid",
                "Show Grid",
                t,
                show_grid,
                fade("inspector-show-grid"),
                editor_show,
                shell_show,
                |ed, cx| ed.set_show_grid(!ed.show_grid, cx),
            ))
            .child(divider())
            .child(header("Snapping", snap_to_grid || snap_to_objects))
            .child(Self::row(
                "inspector-row-snap-to-grid",
                "inspector-snap-to-grid",
                "Snap to Grid",
                t,
                snap_to_grid,
                fade("inspector-snap-to-grid"),
                editor_snap_grid,
                shell_snap_grid,
                |ed, cx| ed.set_snap_to_grid(!ed.snap_to_grid, cx),
            ))
            // Both snap modes coexist: object snaps take priority over the
            // grid lattice while snapping, so neither toggle hides the other.
            .child(Self::row(
                "inspector-row-snap-to-objects",
                "inspector-snap-to-objects",
                "Snap to Objects",
                t,
                snap_to_objects,
                fade("inspector-snap-to-objects"),
                editor_snap_obj,
                shell_snap_obj,
                |ed, cx| ed.set_snap_to_objects(!ed.snap_to_objects, cx),
            ))
            .child(divider())
    }

    // Setting row styled like a menu entry in hover state: bg_tertiary
    // fill, 1px border_color outline, shadow_sm. The whole row clicks, and
    // the switch thumb slides + fades on the shell tween (same easing as
    // menu hovers) instead of snapping.
    fn row<F>(
        id: &'static str,
        fade_key: &'static str,
        label: &'static str,
        t: Theme,
        on: bool,
        k: f32,
        editor: gpui::WeakEntity<crate::editor::Editor>,
        shell: gpui::WeakEntity<crate::ui::shell::Shell>,
        toggle_fn: F,
    ) -> impl IntoElement
    where
        F: Fn(&mut crate::editor::Editor, &mut gpui::Context<crate::editor::Editor>) + 'static,
    {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .h(px(28.))
            .pl(px(6.))
            .pr(px(4.))
            .mx(px(8.))
            .rounded(px(8.))
            .text_sm()
            .text_color(rgb(t.text_primary))
            .cursor_pointer()
            .bg(rgb(t.bg_tertiary))
            .border_1()
            .border_color(rgb(t.border_color))
            .shadow(vec![t.shadow_sm()])
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| {
                    toggle_fn(ed, cx);
                });
                let _ = shell.update(cx, |shell, cx| {
                    shell.animate_fade(fade_key, if on { 0.0 } else { 1.0 }, cx);
                });
            })
            .child(div().child(label))
            .child(Self::switch_visual(k, t))
    }

    fn switch_visual(k: f32, t: Theme) -> impl IntoElement {
        // Square track 36x20 with the row's rounding (8), thumb 14 with
        // proportional rounding (8 * 14/20 = 5.6 -> 5). The thumb glides on
        // left padding (2 -> 18px) while track/border colors cross-fade.
        let bg = lerp_rgb(t.bg_tertiary, t.accent, k);
        let border = lerp_rgb(t.component_border_color, t.accent, k);

        div()
            .w(px(36.))
            .h(px(20.))
            .rounded(px(8.))
            .bg(rgb(bg))
            .border_1()
            .border_color(rgb(border))
            .py(px(2.))
            .pr(px(2.))
            .pl(px(2. + 16. * k))
            .flex()
            .items_center()
            .justify_start()
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded(px(5.))
                    .bg(rgb(0xFFFFFF))
                    .shadow(vec![gpui::BoxShadow {
                        color: gpui::rgba(0x0000001A).into(),
                        offset: gpui::Point {
                            x: px(0.),
                            y: px(1.),
                        },
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                        inset: false,
                    }]),
            )
    }
}
