use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, svg};

use crate::core::constraints::{ConstraintKind, ElementRef};
use crate::editor::{PenMode, Tool};
use crate::theme::{Theme, lerp_rgb};
use crate::ui::inspector::INSPECTOR_WIDTH;

// Floating contextual menu: top-right over the canvas, left of the
// inspector. Appears for Pen (modes + constraints), Dimension (dimension
// types), and Move-with-selection (constraints) — and ONLY when it has at
// least one actionable row. Styling: bg_tertiary, 1px border_color,
// shadow_sm; rows highlight to bg_primary on hover. Reveal is a pop+fade
// (ease-out-cubic + rise) driven by Shell::floating_anim; the fade-OUT
// replays from a cached snapshot. Row-count changes tween the menu height
// (explicit height only while resizing, then back to auto layout).

/// Snapshot of whatever the menu is showing (or showed last, for fade-out),
/// including the resolved constraint rows so the fade-out never renders an
/// empty shell.
/// Which edge-pair dimension is armed (snapshot copy for the menu).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum EdgeDimKind {
    Lines,
    Angle,
    Mid,
}

#[derive(Clone)]
pub(crate) struct FloatingSnap {
    pub tool: Tool,
    pub pen_mode: PenMode,
    pub selection: Vec<ElementRef>,
    pub dim_lock: Option<String>,
    pub kinds: Vec<(MenuAction, &'static [u8], &'static str, &'static str)>,
    /// Self-typing armed target with no lock row (arc radius, point-line):
    /// shown as a muted "Auto" indicator.
    pub dim_auto: Option<&'static str>,
    /// Armed edge-pair dimension + its segment pair. None when no
    /// edge-pair target is armed.
    pub dim_edge: Option<(
        crate::core::ids::SegmentId,
        crate::core::ids::SegmentId,
        EdgeDimKind,
    )>,
    /// Whether the armed edge pair crosses (i.e. an Angle row is offered).
    pub dim_angle_row: bool,
    /// Whether Width/Height/Displacement rows apply to the armed target.
    pub dim_locks_shown: bool,
    /// Show the Distance row: a bezier is selected and no edge-pair
    /// target is armed.
    pub show_distance: bool,
    /// Distance row active: the armed target is the bezier curve length.
    pub dist_active: bool,
}

/// Menu visibility rule (shared with the hotkey layer in shell).
pub(crate) fn menu_visible(tool: Tool, selection: &[ElementRef]) -> bool {
    // NOTE: kept for the hotkey layer; the render path refines this with
    // the resolved row list (no rows -> hidden, see below).
    matches!(tool, Tool::Pen | Tool::Dimension) || (tool == Tool::Move && !selection.is_empty())
}

const ICON_BEZIER_TEMP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M21 4c-5 0-7.02 4.042-9 8s-4 8-9 8m7 0h2m3 0h2m0 0a2 2 0 1 0 4 0a2 2 0 0 0-4 0M12 4h2M7 4h2M7 4a2 2 0 1 1-4 0a2 2 0 0 1 4 0" /></svg>"#;

const GAP_H: f32 = 2.;

#[derive(IntoElement)]
pub struct FloatingMenu {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for FloatingMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let live: Option<FloatingSnap> = self.editor.upgrade().map(|e| {
            let ed = e.read(cx);
            let selection = ed.selection.clone();
            let kinds = if matches!(ed.tool, Tool::Pen | Tool::Move) {
                constraints_for_selection(&selection, cx, &self.editor)
            } else {
                Vec::new()
            };
            use crate::core::constraints::DimTarget as DT;
            let (dim_edge, dim_angle_row) = match ed.dim_target {
                Some(DT::Angle { a, b }) => (Some((a, b, EdgeDimKind::Angle)), true),
                Some(DT::Lines { a, b }) => {
                    let crossing = ed.lines_cross(a, b);
                    (Some((a, b, EdgeDimKind::Lines)), crossing)
                }
                Some(DT::EdgeMid { a, b, .. }) => {
                    let crossing = ed.lines_cross(a, b);
                    (Some((a, b, EdgeDimKind::Mid)), crossing)
                }
                _ => (None, false),
            };
            // Width/Height/Displacement rows apply everywhere except the
            // self-typing targets (arc radius, point-line spans).
            let dim_locks_shown = !matches!(
                ed.dim_target,
                Some(DT::Radius { .. }) | Some(DT::PointLine { .. })
            );
            // Distance row: single-bezier context only (not edge pairs).
            // Active exactly when the curve-length target is armed.
            let show_distance = selection.iter().any(|el| match el {
                ElementRef::Segment(sid) => ed
                    .doc
                    .segment(*sid)
                    .is_some_and(|s| {
                        s.kind == crate::core::document::SegmentKind::Bezier
                    }),
                _ => false,
            }) && dim_edge.is_none();
            let dist_active = show_distance
                && matches!(ed.dim_target, Some(DT::CurveLength { .. }));
            // Radius and point-line targets imply their own type.
            let dim_auto = match ed.dim_target {
                Some(DT::Radius { .. }) => Some("Auto · Radius"),
                Some(DT::PointLine { .. }) => Some("Auto · Point-line"),
                _ => None,
            };
            FloatingSnap {
                tool: ed.tool,
                pen_mode: ed.pen_mode,
                selection: selection.clone(),
                dim_lock: ed.dim_mode_lock.clone(),
                kinds,
                dim_auto,
                dim_edge,
                dim_angle_row,
                dim_locks_shown,
                show_distance,
                dist_active,
            }
        });
        let Some(live) = live else {
            return div().absolute().into_any_element();
        };
        // Visible only with something actionable: Pen/Dimension always
        // have rows; Move needs at least one constraint row.
        let visible = match live.tool {
            Tool::Pen | Tool::Dimension => true,
            Tool::Move => !live.kinds.is_empty(),
            _ => false,
        };
        let parts_key = parts_signature(&live);

        // Latch transitions and drive the single slide+fade tween. Height
        // is always auto layout; row-count changes replay a short slide
        // instead of tweening heights (no estimates, no end jumps).
        let mut anim = 0.0f32;
        if let Some(shell) = self.shell.upgrade() {
            let _ = shell.update(cx, |shell, cx| {
                if visible {
                    shell.floating_cache = Some(live.clone());
                    if !shell.floating_shown {
                        shell.floating_shown = true;
                        shell.floating_anim = 0.0;
                        shell.start_floating_anim(1.0, cx);
                    }
                    shell.floating_parts = parts_key;
                } else if shell.floating_shown {
                    shell.floating_shown = false;
                    shell.start_floating_anim(0.0, cx);
                }
                anim = shell.floating_anim;
            });
        }
        let data: Option<FloatingSnap> = if visible {
            Some(live)
        } else if anim > 0.001 {
            self.shell
                .upgrade()
                .and_then(|s| s.read(cx).floating_cache.clone())
        } else {
            None
        };
        let Some(data) = data else {
            return div().absolute().into_any_element();
        };

        let a = anim.clamp(0., 1.);
        let pop = 1. - (1. - a).powi(3);
        let top = 12. - 10. * (1. - pop);

        let mut menu = div()
            .absolute()
            .top(px(top))
            .right(px(INSPECTOR_WIDTH + 12.))
            .w(px(224.))
            .flex()
            .flex_col()
            .gap(px(GAP_H))
            .p(px(6.))
            .rounded(px(14.))
            .bg(rgb(t.bg_tertiary))
            .border_1()
            .border_color(rgb(t.border_color))
            .shadow(vec![t.shadow_sm()])
            .opacity(pop)
            // NOTE: the id is load-bearing — without it this container has
            // no hitbox, clicks on padding/labels fall through to the
            // canvas behind and drop stray pen points.
            .id(gpui::SharedString::from("floating-menu"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        if data.tool == Tool::Pen {
            menu = menu.child(section_label(t, "Pen"));
            for (mode, icon, label, shortcut) in [
                (PenMode::Line, crate::ui::toolbar::ICON_LINE, "Line", "L"),
                (PenMode::Bezier, ICON_BEZIER_TEMP, "Bezier", "B"),
                (PenMode::Arc, crate::ui::toolbar::ICON_CIRCLE, "Arc", "A"),
            ] {
                let editor = self.editor.clone();
                menu = menu.child(mode_row(
                    &self.shell,
                    t,
                    mode,
                    data.pen_mode,
                    Some(icon),
                    label,
                    shortcut,
                    cx,
                    move |cx: &mut App| {
                        let _ = editor.update(cx, |ed, cx| {
                            if ed.set_pen_mode(mode) {
                                cx.notify();
                            }
                        });
                    },
                ));
            }
        }

        // Constraints surface for Pen AND Move-with-selection.
        if matches!(data.tool, Tool::Pen | Tool::Move) && !data.kinds.is_empty() {
            if data.tool == Tool::Pen {
                menu = menu.child(menu_divider(t));
            }
            menu = menu.child(section_label(t, "Constraints"));
            for (action, icon, label, shortcut) in data.kinds.clone() {
                let editor = self.editor.clone();
                menu = menu.child(action_row(
                    &self.shell,
                    t,
                    &format!("constraint-{}", apply_key(action)),
                    Some(icon),
                    label,
                    shortcut,
                    false,
                    cx,
                    move |cx: &mut App| {
                        let _ = editor.update(cx, |ed, cx| {
                            let changed = match action {
                                MenuAction::Constraint(kind) => ed.apply_constraint_from_menu(kind),
                                MenuAction::MergePoints => ed.merge_selected_points(),
                            };
                            if changed {
                                cx.notify();
                            }
                        });
                    },
                ));
            }
        }

        if data.tool == Tool::Dimension {
            menu = menu.child(section_label(t, "Dimension"));
            if let Some(auto) = data.dim_auto {
                menu = menu.child(auto_label(t, auto));
            }
            // Angle first (the odd one out), then the usual trio. No
            // Distance row: bezier length is the default single-pick type
            // and edge gaps measure as midpoint displacement.
            if data.dim_angle_row {
                let editor = self.editor.clone();
                let angle_active = matches!(data.dim_edge, Some((_, _, EdgeDimKind::Angle)));
                menu = menu.child(action_row(
                    &self.shell,
                    t,
                    "dim-kind-angle",
                    Some(crate::ui::toolbar::ICON_DIMENSION),
                    "Angle",
                    "A",
                    angle_active,
                    cx,
                    move |cx: &mut App| {
                        let _ = editor.update(cx, |ed, cx| {
                            ed.set_dim_edge_kind(!angle_active);
                            cx.notify();
                        });
                    },
                ));
            }
            // Distance (bezier curve length) comes first so the default
            // type is one click away; it is active exactly when the
            // curve-length target is armed. Clicking lifts any lock,
            // which re-arms the default for the picks.
            if data.show_distance {
                let editor = self.editor.clone();
                menu = menu.child(action_row(
                    &self.shell,
                    t,
                    "dim-mode-distance",
                    Some(crate::ui::toolbar::ICON_DIMENSION),
                    "Distance",
                    "C",
                    data.dist_active,
                    cx,
                    move |cx: &mut App| {
                        let _ = editor.update(cx, |ed, cx| {
                            ed.set_dim_mode_lock(None);
                            cx.notify();
                        });
                    },
                ));
            }
            if data.dim_locks_shown {
                for (mode, label, shortcut) in [
                    ("width", "Width", "X"),
                    ("height", "Height", "Y"),
                    ("displacement", "Displacement", "D"),
                ] {
                    let editor = self.editor.clone();
                    let owned = mode.to_string();
                    let active = data.dim_lock.as_deref() == Some(mode);
                    menu = menu.child(action_row(
                        &self.shell,
                        t,
                        &format!("dim-mode-{mode}"),
                        Some(crate::ui::toolbar::ICON_DIMENSION),
                        label,
                        shortcut,
                        active,
                        cx,
                        move |cx: &mut App| {
                            let owned = owned.clone();
                            let _ = editor.update(cx, |ed, cx| {
                                ed.set_dim_mode_lock(if active {
                                    None
                                } else {
                                    Some(owned.clone())
                                });
                                cx.notify();
                            });
                        },
                    ));
                }
            }
        }

        menu.into_any_element()
    }
}

/// Content signature: changes exactly when the row list changes (drives
/// the expand/collapse height tween).
fn parts_signature(snap: &FloatingSnap) -> String {
    let kinds: Vec<&str> = snap.kinds.iter().map(|(_, _, l, _)| *l).collect();
    format!(
        "{:?}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}",
        snap.tool,
        snap.pen_mode,
        kinds.join(","),
        snap.dim_lock.as_deref().unwrap_or(""),
        snap.show_distance,
        snap.dist_active,
        snap.dim_auto.unwrap_or(""),
        snap.dim_edge
            .map(|(_, _, k)| match k {
                EdgeDimKind::Angle => "angle",
                EdgeDimKind::Lines => "lines",
                EdgeDimKind::Mid => "mid",
            })
            .unwrap_or("-"),
        snap.dim_angle_row,
        snap.dim_locks_shown,
    )
}

fn section_label(t: Theme, label: &'static str) -> impl IntoElement {
    div()
        .px(px(6.))
        .pt(px(2.))
        .pb(px(2.))
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_secondary))
        .child(label)
}

fn menu_divider(t: Theme) -> impl IntoElement {
    div()
        .h(px(1.))
        .w_full()
        .my(px(4.))
        .bg(rgb(t.component_border_color))
}

/// Muted non-interactive indicator (e.g. self-typing dimension targets).
fn auto_label(t: Theme, label: &'static str) -> impl IntoElement {
    div()
        .px(px(6.))
        .pt(px(2.))
        .pb(px(2.))
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(t.empty_text_secondary))
        .child(label)
}

/// A floating-menu action row: a geometric constraint or a point merge.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum MenuAction {
    Constraint(ConstraintKind),
    MergePoints,
}

fn apply_key(action: MenuAction) -> &'static str {
    match action {
        MenuAction::Constraint(kind) => match kind {
            ConstraintKind::Horizontal | ConstraintKind::Vertical => "hv",
            ConstraintKind::Coincident => "coincident",
            ConstraintKind::Tangent => "tangent",
            ConstraintKind::Parallel => "parallel",
            ConstraintKind::Perpendicular => "perpendicular",
        },
        MenuAction::MergePoints => "merge",
    }
}

// Merge mark: two points joining into one.
const ICON_MERGE: &[u8] = br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="2.5" cy="3.5" r="1.1" fill="black"/><circle cx="2.5" cy="8.5" r="1.1" fill="black"/><path d="M3.5 4L7.5 6L3.5 8" stroke="black" stroke-width="0.9" stroke-linecap="round" stroke-linejoin="round"/><circle cx="9" cy="6" r="1.2" fill="black"/></svg>"#;

fn row_icon(icon: Option<&'static [u8]>, t: Theme) -> impl IntoElement {
    match icon {
        Some(data) => div()
            .w(px(14.))
            .h(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .data(data)
                    .w(px(14.))
                    .h(px(14.))
                    .text_color(rgb(t.text_secondary)),
            )
            .into_any_element(),
        None => div().w(px(14.)).h(px(14.)).into_any_element(),
    }
}

/// Pen mode row: active row uses bg_primary + border (toolbar contract),
/// idle rows hover-fade to bg_primary via the shell tween.
#[allow(clippy::too_many_arguments)]
fn mode_row(
    shell: &gpui::WeakEntity<crate::ui::shell::Shell>,
    t: Theme,
    mode: PenMode,
    active: PenMode,
    icon: Option<&'static [u8]>,
    label: &'static str,
    shortcut: &'static str,
    cx: &App,
    on_pick: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let key = format!("pen-mode-{}", mode.as_str());
    let k = shell
        .upgrade()
        .map(|s| s.read(cx).fade(&key))
        .unwrap_or(0.0);
    let is_active = mode == active;
    let bg = if is_active {
        t.bg_primary
    } else {
        lerp_rgb(t.bg_tertiary, t.bg_primary, k)
    };
    let shell_hover = shell.clone();
    let key_h = key.clone();
    let active_shadow = if is_active { vec![t.shadow_sm()] } else { Vec::new() };
    div()
        .id(gpui::SharedString::from(key.clone()))
        .flex()
        .items_center()
        .justify_between()
        .h(px(28.))
        .pl(px(8.))
        .pr(px(5.))
        .rounded(px(10.))
        .text_xs()
        .text_color(rgb(if is_active {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .cursor_pointer()
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(if is_active {
            t.border_color
        } else {
            t.bg_tertiary
        }))
        .shadow(active_shadow)
        .on_hover(move |hovered, _, cx| {
            let _ = shell_hover.update(cx, |shell, cx| {
                shell.animate_fade(&key_h, if *hovered { 1.0 } else { 0.0 }, cx);
            });
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            on_pick(cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(row_icon(icon, t))
                .child(div().child(label)),
        )
        .child(if shortcut.is_empty() {
            div().into_any_element()
        } else {
            // Keybind hint pill: perfect 18px square (all keybinds are
            // one letter), centered glyph, 6px rounding against the row's
            // 10px. 5px spacing on every side inside the 28px row.
            div()
                .w(px(18.))
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(rgb(t.bg_primary))
                .border_1()
                .border_color(rgb(t.border_color))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(t.empty_text_secondary))
                        .child(shortcut),
                )
                .into_any_element()
        })
}

/// Generic action row (constraints, dimension types). An active row
/// (locked dimension type) renders like an active pen mode: bg_primary
/// fill, border_color outline, primary text — unmissable at a glance.
#[allow(clippy::too_many_arguments)]
fn action_row(
    shell: &gpui::WeakEntity<crate::ui::shell::Shell>,
    t: Theme,
    key: &str,
    icon: Option<&'static [u8]>,
    label: &'static str,
    shortcut: &'static str,
    active: bool,
    cx: &App,
    on_click: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let k = shell.upgrade().map(|s| s.read(cx).fade(key)).unwrap_or(0.0);
    let bg = if active {
        t.bg_primary
    } else {
        lerp_rgb(t.bg_tertiary, t.bg_primary, k)
    };
    let shell_hover = shell.clone();
    let active_shadow = if active { vec![t.shadow_sm()] } else { Vec::new() };
    let key_owned = key.to_string();
    let key_hover = key_owned.clone();
    div()
        .id(gpui::SharedString::from(key_owned.clone()))
        .flex()
        .items_center()
        .justify_between()
        .h(px(28.))
        .pl(px(8.))
        .pr(px(5.))
        .rounded(px(10.))
        .text_xs()
        .text_color(rgb(if active {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .cursor_pointer()
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(if active {
            t.border_color
        } else {
            t.bg_tertiary
        }))
        .shadow(active_shadow)
        .on_hover(move |hovered, _, cx| {
            let _ = shell_hover.update(cx, |shell, cx| {
                shell.animate_fade(&key_hover, if *hovered { 1.0 } else { 0.0 }, cx);
            });
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            on_click(cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(row_icon(icon, t))
                .child(div().child(label)),
        )
        .child(if shortcut.is_empty() {
            div().into_any_element()
        } else {
            // Keybind hint pill: perfect 18px square (all keybinds are
            // one letter), centered glyph, 6px rounding against the row's
            // 10px. 5px spacing on every side inside the 28px row.
            div()
                .w(px(18.))
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(rgb(t.bg_primary))
                .border_1()
                .border_color(rgb(t.border_color))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(t.empty_text_primary))
                        .child(shortcut),
                )
                .into_any_element()
        })
}

/// Contextual action rows for the current selection, each with its
/// application hotkey (live while the menu is visible). Every row goes
/// through the shared `Editor::gate_*` candidates — the exact same
/// functions the apply path uses — so rows only appear when the
/// constraint can actually apply: same-segment pairs never offer
/// H/V or Coincident, Merge needs close unglued points, and
/// tangent/parallel/perpendicular consult existing H/V locks.
pub(crate) fn constraints_for_selection(
    selection: &[ElementRef],
    cx: &App,
    editor: &gpui::WeakEntity<crate::editor::Editor>,
) -> Vec<(MenuAction, &'static [u8], &'static str, &'static str)> {
    use crate::editor::Editor;
    use crate::ui::canvas;
    use crate::ui::toolbar;
    let hv = || {
        (
            MenuAction::Constraint(ConstraintKind::Horizontal),
            toolbar::ICON_CONSTRAINT_HV,
            "Horizontal / Vertical",
            "H",
        )
    };
    let coincident = || {
        (
            MenuAction::Constraint(ConstraintKind::Coincident),
            canvas::ICON_CHIP_COINCIDENT,
            "Coincident",
            "C",
        )
    };
    let merge = || (MenuAction::MergePoints, ICON_MERGE, "Merge points", "");
    let tangent_row = || {
        (
            MenuAction::Constraint(ConstraintKind::Tangent),
            canvas::ICON_CHIP_TANGENT,
            "Tangent",
            "T",
        )
    };
    let Some(ed) = editor.upgrade() else {
        return Vec::new();
    };
    let ed = ed.read(cx);
    let doc = &ed.doc;
    let tol = crate::editor::SNAP_TOL_PX / ed.camera.zoom.max(1e-6);
    let mut out = Vec::new();
    if Editor::hv_candidates(doc, selection).is_some() {
        out.push(hv());
    }
    // Parallel / perpendicular stay explicit-segment-only (bare points
    // route through coincident/tangent instead of sprouting rows).
    if let Some((a, b)) = Editor::line_pair(doc, selection) {
        if Editor::line_pair_feasible(doc, a, b, true) {
            out.push((
                MenuAction::Constraint(ConstraintKind::Parallel),
                toolbar::ICON_CONSTRAINT_PARALLEL,
                "Parallel",
                "P",
            ));
        }
        if Editor::line_pair_feasible(doc, a, b, false) {
            out.push((
                MenuAction::Constraint(ConstraintKind::Perpendicular),
                toolbar::ICON_CONSTRAINT_PERPENDICULAR,
                "Perpendicular",
                "E",
            ));
        }
    }
    // Tangent covers line+curve AND line+line (collinear); bare points
    // contribute their owning segments, so two points on two lines still
    // offer it.
    if let Some((line, other, contact, _)) = Editor::tangent_candidate(doc, selection) {
        if Editor::tangent_feasible(doc, line, other, contact) {
            out.push(tangent_row());
        }
    }
    if Editor::coincident_candidates(doc, selection).is_some() {
        out.push(coincident());
    }
    if !Editor::merge_candidate_pairs(doc, selection, tol).is_empty() {
        out.push(merge());
    }
    out
}
