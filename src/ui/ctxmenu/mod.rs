use gpui::{
    App, IntoElement, MouseButton, Point, RenderOnce, SharedString, Window, div, prelude::*, px,
    rgb, rgba, svg,
};

use crate::theme::{fade_in, lerp_rgb};
use crate::ui::shell::title_bar::TITLE_BAR_HEIGHT;

// Canvas right-click menu (Clay Phase 0.3): generic infra, target-aware.
// Target granularity grows with the model — Selection/Empty now (no
// components exist yet); Joint/Span arms land in Phase 1 without touching
// this renderer. Every leaf fires a REAL op; no stubs, no dead buttons.
//
// Known Phase-0 limitation: arming a constraint tool clears the selection
// (all tool switches do). Pre-seeding picks from the selection is Phase 4.

#[derive(Clone, Copy)]
pub struct CanvasMenuState {
    pub position: Point<gpui::Pixels>,
    pub open_sub: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum MenuAction {
    DeleteSelection,
    CopySelection,
    CutSelection,
    Paste,
    // Direct application (floating-menu path): these COMMIT the constraint
    // using the current selection — never tool-arm (arming clears picks).
    ApplyConstraint(crate::core::constraints::ConstraintKind),
    MergePoints,
    SetGrade(crate::editor::joints::Grade),
    // Flip Object ↔ Edit (same as Tab).
    ToggleMode(crate::editor::InteractionMode),
}

enum MenuItem {
    Entry {
        label: String,
        icon: Option<&'static [u8]>,
        shortcut: Option<&'static str>,
        destructive: bool,
        checked: bool,
        enabled: bool,
        action: MenuAction,
    },
    Separator,
    Submenu {
        label: &'static str,
        items: Vec<MenuItem>,
    },
}

impl MenuItem {
    /// Dead buttons don't render at all (no muted noise).
    fn is_live(&self) -> bool {
        match self {
            MenuItem::Entry { enabled, .. } => *enabled,
            _ => true,
        }
    }
}

const ENTRY_H: f32 = 24.0;

const ICON_DELETE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256"><path d="M0 0h256v256H0z" fill="none" /><path fill="currentColor" d="M216 50h-42V40a22 22 0 0 0-22-22h-48a22 22 0 0 0-22 22v10H40a6 6 0 0 0 0 12h10v146a14 14 0 0 0 14 14h128a14 14 0 0 0 14-14V62h10a6 6 0 0 0 0-12M94 40a10 10 0 0 1 10-10h48a10 10 0 0 1 10 10v10H94Zm100 168a2 2 0 0 1-2 2H64a2 2 0 0 1-2-2V62h132Zm-84-104v64a6 6 0 0 1-12 0v-64a6 6 0 0 1 12 0m48 0v64a6 6 0 0 1-12 0v-64a6 6 0 0 1 12 0" /></svg>"#;

const ICON_COPY: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256"><path d="M0 0h256v256H0z" fill="none" /><g fill="currentColor"><path d="M216 40v128h-48V88H88V40Z" opacity=".2" /><path d="M216 32H88a8 8 0 0 0-8 8v40H40a8 8 0 0 0-8 8v128a8 8 0 0 0 8 8h128a8 8 0 0 0 8-8v-40h40a8 8 0 0 0 8-8V40a8 8 0 0 0-8-8m-56 176H48V96h112Zm48-48h-32V88a8 8 0 0 0-8-8H96V48h112Z" /></g></svg>"#;

const ICON_PASTE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256"><path d="M0 0h256v256H0z" fill="none" /><path fill="currentColor" d="M200 32h-36.26a47.92 47.92 0 0 0-71.48 0H56a16 16 0 0 0-16 16v168a16 16 0 0 0 16 16h144a16 16 0 0 0 16-16V48a16 16 0 0 0-16-16m-72 0a32 32 0 0 1 32 32H96a32 32 0 0 1 32-32m72 184H56V48h26.75A47.9 47.9 0 0 0 80 64v8a8 8 0 0 0 8 8h80a8 8 0 0 0 8-8v-8a47.9 47.9 0 0 0-2.75-16H200Z" /></svg>"#;

const ICON_CUT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256"><path d="M0 0h256v256H0z" fill="none" /><path fill="currentColor" d="M157.73 113.13a8 8 0 0 1 2.09-11.13l67.66-46.3a8 8 0 0 1 9 13.21l-67.67 46.3a7.9 7.9 0 0 1-4.51 1.4a8 8 0 0 1-6.57-3.48m80.87 85.09a8 8 0 0 1-11.12 2.08L136 137.7l-42.51 29.08a36 36 0 1 1-9-13.19L121.83 128l-37.39-25.59a35.86 35.86 0 1 1 9-13.19l143 97.87a8 8 0 0 1 2.16 11.13M80 180a20 20 0 1 0-5.86 14.14A19.85 19.85 0 0 0 80 180m-5.86-89.87a20 20 0 1 0-28.28 0a19.85 19.85 0 0 0 28.28 0" /></svg>"#;

const ICON_MERGE_POINTS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 256 256"><path d="M0 0h256v256H0z" fill="none" /><path fill="currentColor" d="M172.91 83.09a78 78 0 1 0-89.82 89.82a78 78 0 1 0 89.82-89.82M30 96a66 66 0 0 1 130.49-14H160a78.09 78.09 0 0 0-78 78v.49A66.1 66.1 0 0 1 30 96m64 64a65.6 65.6 0 0 1 6-27.49L123.49 156A65.6 65.6 0 0 1 96 162c-.65 0-1.3 0-2-.05zm40.23-10.25l-28-28a66.5 66.5 0 0 1 15.52-15.52l28 28a66.5 66.5 0 0 1-15.52 15.52M162 96a65.6 65.6 0 0 1-6 27.49L132.51 100A65.6 65.6 0 0 1 160 94h1.95c.05.7.05 1.35.05 2m-2 130a66.1 66.1 0 0 1-64.49-52H96a78.09 78.09 0 0 0 78-78v-.49A66 66 0 0 1 160 226" /></svg>"#;

const ICON_EDIT_ROW: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><g fill="none" stroke="currentColor" stroke-linejoin="round" stroke-width="1.5"><path d="m12.791 8.768l4.693 1.836c2.706 1.059 4.06 1.589 4.015 2.429s-1.457 1.225-4.282 1.995c-.841.23-1.262.344-1.553.636c-.292.291-.406.712-.636 1.553c-.77 2.825-1.155 4.237-1.995 4.282s-1.37-1.308-2.43-4.015L8.769 12.79c-1.11-2.834-1.664-4.25-.946-4.969c.718-.718 2.135-.163 4.969.946Z" /><path stroke-linecap="round" d="M4.5 2.5h-1a1 1 0 0 0-1 1v1a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-1a1 1 0 0 0-1-1m0 11h-1a1 1 0 0 0-1 1v1a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-1a1 1 0 0 0-1-1m11-11h-1a1 1 0 0 0-1 1v1a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-1a1 1 0 0 0-1-1m-2 1.5h-8M4 5.5v8" /></g></svg>"#;

const ICON_GRADE_CORNER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 4v16h16" /></svg>"#;

const ICON_GRADE_SMOOTH: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="2" d="M3 15c4 0 4-6 9-6s5 6 9 6" /></svg>"#;

const ICON_GRADE_CURVE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><g fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="7" /><circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" /></g></svg>"#;
const MENU_W: f32 = 168.0;
const SUBMENU_W: f32 = 200.0;
const SUB_OVERLAP: f32 = 3.0;
const ITEM_GAP: f32 = 2.0;
const PANEL_PAD: f32 = 4.0;

#[derive(IntoElement)]
pub struct CanvasMenu {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for CanvasMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let Some(shell_e) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let Some(state) = shell_e.read(cx).canvas_menu else {
            return div().into_any_element();
        };

        // Target derives live at render: selection, mode, clipboard.
        // Drop dead rows + collapse the separators around them (leading,
        // trailing, doubled) so indices, heights, and submenu alignment
        // stay exact.
        let items = self
            .editor
            .upgrade()
            .map(|e| {
                let ed = e.read(cx);
                let can_paste = cx
                    .read_from_clipboard()
                    .and_then(|i| i.text())
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                let built = build_items(
                    &ed.selection.clone(),
                    ed.interaction_mode == crate::editor::InteractionMode::Edit,
                    can_paste,
                    &ed.doc,
                    cx,
                    &self.editor,
                );
                let mut clean: Vec<MenuItem> = Vec::with_capacity(built.len());
                for item in built.into_iter().filter(|i| i.is_live()) {
                    let dup_sep = matches!(item, MenuItem::Separator)
                        && matches!(clean.last(), Some(MenuItem::Separator));
                    if !dup_sep {
                        clean.push(item);
                    }
                }
                while matches!(clean.first(), Some(MenuItem::Separator)) {
                    clean.remove(0);
                }
                while matches!(clean.last(), Some(MenuItem::Separator)) {
                    clean.pop();
                }
                clean
            })
            .unwrap_or_default();

        // Clamp FULLY inside the design column (below the title bar, left
        // of the inspector dock): never half off-screen. Reserve submenu
        // width while one is open so it fits too.
        let vp = window.viewport_size();
        let est_h = PANEL_PAD * 2. + items_height(&items);
        let col_h = f32::from(vp.height) - TITLE_BAR_HEIGHT;
        let right_dock = crate::ui::inspector::INSPECTOR_WIDTH + 8.;
        let sub_w = if state.open_sub.is_some() {
            SUBMENU_W
        } else {
            0.
        };
        let x = f32::from(state.position.x)
            .min(f32::from(vp.width) - MENU_W - sub_w - right_dock)
            .max(4.);
        let y = (f32::from(state.position.y) - TITLE_BAR_HEIGHT)
            .min(col_h - est_h - 4.)
            .max(4.);

        div()
            .absolute()
            .left(px(x.max(4.)))
            .top(px(y))
            .w(px(MENU_W))
            .flex()
            .flex_col()
            .px(px(PANEL_PAD))
            .py(px(PANEL_PAD))
            .gap_y(px(ITEM_GAP))
            .bg(rgb(t.menu_bg))
            .border_1()
            .border_color(rgb(t.menu_border_color))
            .rounded(px(10.))
            .shadow(vec![t.shadow_sm()])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| self.render_item(item, i, state.open_sub, t, cx)),
            )
            // Open submenu as a SIBLING with 1px overlap (dropdown.rs
            // contract): nested-in-row positioning painted under the main
            // panel border. Sibling + overlap puts it on top.
            .child(
                state
                    .open_sub
                    .and_then(|i| match items.get(i) {
                        Some(MenuItem::Submenu { items: sub, .. }) => Some(
                            self.render_submenu(sub, submenu_top(&items, i), t, cx)
                                .into_any_element(),
                        ),
                        _ => None,
                    })
                    .unwrap_or(div().into_any_element()),
            )
            .into_any_element()
    }
}

/// Top of the i-th row (uniform rows + 11px separators), mirroring the
/// panel layout so the sibling submenu aligns with its parent row.
fn submenu_top(items: &[MenuItem], index: usize) -> f32 {
    let mut top = PANEL_PAD;
    for it in items.iter().take(index) {
        top += match it {
            MenuItem::Separator => 11.,
            _ => ENTRY_H,
        } + ITEM_GAP;
    }
    top
}

fn items_height(items: &[MenuItem]) -> f32 {
    items
        .iter()
        .map(|it| match it {
            MenuItem::Separator => 11.,
            _ => ENTRY_H,
        })
        .sum::<f32>()
        + ITEM_GAP * items.len().saturating_sub(1) as f32
}

fn build_items(
    selection: &[crate::core::constraints::ElementRef],
    edit_mode: bool,
    can_paste: bool,
    doc: &crate::core::document::Document,
    cx: &App,
    editor: &gpui::WeakEntity<crate::editor::Editor>,
) -> Vec<MenuItem> {
    use crate::editor::joints::Grade;
    use crate::ui::floating_menu::{
        MenuAction as FMA, constraint_row_parts, constraints_for_selection,
    };
    let mut items = Vec::new();
    // Top: clipboard group (Cut above Copy).
    if !selection.is_empty() {
        items.push(MenuItem::Entry {
            label: "Cut".into(),
            icon: Some(ICON_CUT),
            shortcut: Some("Ctrl+X"),
            destructive: false,
            checked: false,
            enabled: true,
            action: MenuAction::CutSelection,
        });
    }
    items.push(MenuItem::Entry {
        label: "Copy".into(),
        icon: Some(ICON_COPY),
        shortcut: Some("Ctrl+C"),
        destructive: false,
        checked: false,
        enabled: true,
        action: MenuAction::CopySelection,
    });
    items.push(MenuItem::Entry {
        label: "Paste".into(),
        icon: Some(ICON_PASTE),
        shortcut: Some("Ctrl+V"),
        destructive: false,
        checked: false,
        enabled: can_paste,
        action: MenuAction::Paste,
    });
    if !selection.is_empty() {
        items.push(MenuItem::Separator);
        // Mode toggle: breaks the selection into Edit, or back out to
        // Object when already there. Key stays Tab both ways.
        let (mode_label, mode_icon, to_mode) = if edit_mode {
            (
                "Object...",
                crate::ui::modebar::ICON_OBJECT,
                crate::editor::InteractionMode::Object,
            )
        } else {
            (
                "Edit...",
                ICON_EDIT_ROW,
                crate::editor::InteractionMode::Edit,
            )
        };
        items.push(MenuItem::Entry {
            label: mode_label.into(),
            icon: Some(mode_icon),
            shortcut: Some("Tab"),
            destructive: false,
            checked: false,
            enabled: true,
            action: MenuAction::ToggleMode(to_mode),
        });
        items.push(MenuItem::Separator);
        // Constraint section: flat rows (never a dropdown). Applicability
        // comes from the SAME gates the floating menu uses; dead rows are
        // filtered below (never rendered). HV splits by inferred orientation —
        // never the long label when live.
        let applicable = constraints_for_selection(selection, cx, editor);
        let is_on = |action: FMA| applicable.iter().any(|(a, _, _, _)| *a == action);
        let hv_live = is_on(FMA::Constraint(
            crate::core::constraints::ConstraintKind::Horizontal,
        ));
        let (hv_label, hv_kind, hv_enabled) = match hv_live_label(doc, selection) {
            Some((label, kind)) => (label, kind, true),
            None => (
                "Horizontal / Vertical",
                crate::core::constraints::ConstraintKind::Horizontal,
                false,
            ),
        };
        let (hv_icon, _, _) = constraint_row_parts(FMA::Constraint(
            crate::core::constraints::ConstraintKind::Horizontal,
        ));
        items.push(MenuItem::Entry {
            label: hv_label.into(),
            icon: hv_icon,
            shortcut: Some("H"),
            destructive: false,
            checked: false,
            enabled: hv_enabled,
            action: MenuAction::ApplyConstraint(hv_kind),
        });
        for action in [
            FMA::Constraint(crate::core::constraints::ConstraintKind::Tangent),
            FMA::Constraint(crate::core::constraints::ConstraintKind::Coincident),
            FMA::Constraint(crate::core::constraints::ConstraintKind::Parallel),
            FMA::Constraint(crate::core::constraints::ConstraintKind::Perpendicular),
        ] {
            let (icon, label, shortcut) = constraint_row_parts(action);
            let kind = match action {
                FMA::Constraint(k) => k,
                FMA::MergePoints => unreachable!(),
            };
            items.push(MenuItem::Entry {
                label: label.into(),
                icon,
                shortcut: if shortcut.is_empty() {
                    None
                } else {
                    Some(shortcut)
                },
                destructive: false,
                checked: false,
                enabled: is_on(action),
                action: MenuAction::ApplyConstraint(kind),
            });
        }
        // Merge points (own icon, not the floating one).
        items.push(MenuItem::Entry {
            label: "Merge points".into(),
            icon: Some(ICON_MERGE_POINTS),
            shortcut: None,
            destructive: false,
            checked: false,
            enabled: is_on(FMA::MergePoints),
            action: MenuAction::MergePoints,
        });
        // Clay grades: mouse-only loop for Edit joint work (mirrors V/H).
        if edit_mode
            && selection
                .iter()
                .any(|el| matches!(el, crate::core::constraints::ElementRef::Point(_)))
        {
            items.push(MenuItem::Separator);
            items.push(MenuItem::Submenu {
                label: "Grade",
                items: vec![
                    grade_entry("Corner", Some(ICON_GRADE_CORNER), Some("V"), Grade::G0),
                    grade_entry("Smooth", Some(ICON_GRADE_SMOOTH), Some("H"), Grade::G1),
                    grade_entry(
                        "Curvature",
                        Some(ICON_GRADE_CURVE),
                        Some("Shift+H"),
                        Grade::G2,
                    ),
                ],
            });
        }
        items.push(MenuItem::Separator);
        items.push(MenuItem::Entry {
            label: "Delete".into(),
            icon: Some(ICON_DELETE),
            shortcut: Some("Del"),
            destructive: true,
            checked: false,
            enabled: true,
            action: MenuAction::DeleteSelection,
        });
    }
    items
}

/// HV orientation label from the same dominant-axis inference the apply
/// path uses (endpoints, not the row text). None when inapplicable.
fn hv_live_label(
    doc: &crate::core::document::Document,
    selection: &[crate::core::constraints::ElementRef],
) -> Option<(&'static str, crate::core::constraints::ConstraintKind)> {
    use crate::core::constraints::ConstraintKind as K;
    let (a, b, _) = crate::editor::Editor::hv_candidates(doc, selection)?;
    let (pa, pb) = (doc.point(a)?, doc.point(b)?);
    if (pa.y - pb.y).abs() <= (pa.x - pb.x).abs() {
        Some(("Horizontal", K::Horizontal))
    } else {
        Some(("Vertical", K::Vertical))
    }
}

fn grade_entry(
    label: &'static str,
    icon: Option<&'static [u8]>,
    shortcut: Option<&'static str>,
    grade: crate::editor::joints::Grade,
) -> MenuItem {
    MenuItem::Entry {
        label: label.into(),
        icon,
        shortcut,
        destructive: false,
        checked: false,
        enabled: true,
        action: MenuAction::SetGrade(grade),
    }
}

impl CanvasMenu {
    fn render_item(
        &self,
        item: &MenuItem,
        index: usize,
        open_sub: Option<usize>,
        t: crate::theme::Theme,
        cx: &App,
    ) -> gpui::AnyElement {
        match item {
            MenuItem::Separator => div()
                .h(px(1.))
                .mx(px(-PANEL_PAD))
                .my(px(5.))
                .bg(rgb(t.menu_border_color))
                .into_any_element(),
            MenuItem::Entry {
                label,
                icon,
                shortcut,
                destructive,
                checked,
                enabled,
                action,
            } => {
                let editor = self.editor.clone();
                let shell = self.shell.clone();
                let shell_hov = self.shell.clone();
                let action = *action;
                let interactive = *enabled;
                let key = format!("ctxmenu-{index}");
                let k = shell
                    .upgrade()
                    .map(|s| s.read(cx).fade(&key))
                    .unwrap_or(0.0);
                let bg = lerp_rgb(t.menu_bg, t.menu_hover_bg, if interactive { k } else { 0.0 });
                let mut shadow = t.shadow_sm();
                shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();
                // Dead rows: muted ink, no fade, no click. Destructive red
                // wins over muted only when live.
                let fg = if *destructive && interactive {
                    0xE53E3E
                } else if !interactive {
                    t.empty_text_secondary
                } else {
                    t.text_primary
                };
                div()
                    .id(SharedString::from(key.clone()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(ENTRY_H))
                    .pl(px(6.))
                    .pr(px(4.))
                    .rounded(px(8.))
                    .text_xs()
                    .text_color(rgb(fg))
                    .bg(rgb(bg))
                    .shadow(vec![shadow])
                    .when(interactive, |d| {
                        d.cursor_pointer().on_hover(move |hovered, _, cx| {
                            let _ = shell_hov.update(cx, |shell, cx| {
                                shell.animate_fade(&key, if *hovered { 1.0 } else { 0.0 }, cx);
                            });
                        })
                    })
                    .when(interactive, |d| {
                        d.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            fire_action(&editor, &shell, action, cx);
                        })
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .child(row_icon(*icon, fg, t))
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .when(*checked, |d| {
                                d.child(
                                    div().text_xs().text_color(rgb(t.text_secondary)).child("✓"),
                                )
                            })
                            .children(shortcut.iter().map(|s| keycap(s, t, !interactive, 1.0))),
                    )
                    .into_any_element()
            }
            MenuItem::Submenu { label, items } => {
                let shell = self.shell.clone();
                let shell_hov = self.shell.clone();
                let key = format!("ctxmenu-sub-{index}");
                let hov = shell
                    .upgrade()
                    .map(|s| s.read(cx).fade(&key))
                    .unwrap_or(0.0);
                let expanded = open_sub == Some(index);
                let k = hov.max(if expanded { 1.0 } else { 0.0 });
                let bg = lerp_rgb(t.menu_bg, t.menu_hover_bg, k);
                let mut shadow = t.shadow_sm();
                shadow.color = rgba(fade_in(t.item_shadow_color, k)).into();
                div()
                    .id(SharedString::from(key.clone()))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(ENTRY_H))
                    .pl(px(6.))
                    .pr(px(4.))
                    .rounded(px(8.))
                    .text_xs()
                    .text_color(rgb(t.text_primary))
                    .cursor_pointer()
                    .bg(rgb(bg))
                    .shadow(vec![shadow])
                    .on_hover(move |hovered, _, cx| {
                        let _ = shell_hov.update(cx, |shell, cx| {
                            shell.animate_fade(&key, if *hovered { 1.0 } else { 0.0 }, cx);
                            if *hovered {
                                shell.canvas_menu_sub(index, cx);
                            }
                        });
                    })
                    .child(label.to_string())
                    .child(div().text_xs().text_color(rgb(t.text_secondary)).child("›"))
                    .into_any_element()
            }
        }
    }

    fn render_submenu(
        &self,
        items: &[MenuItem],
        top: f32,
        t: crate::theme::Theme,
        cx: &App,
    ) -> impl IntoElement {
        div()
            .occlude()
            .absolute()
            .left(px(MENU_W - SUB_OVERLAP))
            .top(px(top))
            .w(px(SUBMENU_W))
            .flex()
            .flex_col()
            .px(px(PANEL_PAD))
            .py(px(PANEL_PAD))
            .gap_y(px(ITEM_GAP))
            .bg(rgb(t.menu_bg))
            .border_1()
            .border_color(rgb(t.border_color))
            .rounded(px(10.))
            .shadow(vec![t.shadow_sm()])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(|(j, item)| self.render_item(item, 100 + j, None, t, cx)),
            )
    }
}

fn fire_action(
    editor: &gpui::WeakEntity<crate::editor::Editor>,
    shell: &gpui::WeakEntity<crate::ui::shell::Shell>,
    action: MenuAction,
    cx: &mut App,
) {
    let _ = shell.update(cx, |shell, cx| {
        shell.close_canvas_menu(cx);
        match action {
            MenuAction::DeleteSelection => shell.delete_selection(cx),
            MenuAction::CopySelection => shell.copy_selection(cx),
            MenuAction::CutSelection => shell.cut_selection(cx),
            MenuAction::Paste => shell.paste_clipboard(cx),
            MenuAction::ToggleMode(mode) => {
                let _ = editor.update(cx, |ed, cx| {
                    if ed.set_interaction_mode(mode) {
                        cx.notify();
                    }
                });
            }
            MenuAction::ApplyConstraint(kind) => {
                let _ = editor.update(cx, |ed, cx| {
                    if ed.apply_constraint_from_menu(kind) {
                        cx.notify();
                    }
                });
            }
            MenuAction::MergePoints => {
                let _ = editor.update(cx, |ed, cx| {
                    if ed.merge_selected_points() {
                        cx.notify();
                    }
                });
            }
            MenuAction::SetGrade(grade) => {
                if let Some(ed) = shell.editor.as_ref() {
                    let changed = ed.update(cx, |ed, _| {
                        let pts: Vec<_> =
                            ed.selection.iter().filter_map(|el| el.as_point()).collect();
                        if pts.is_empty() {
                            return false;
                        }
                        for p in pts {
                            ed.set_grade(p, grade);
                        }
                        ed.derive_handles();
                        true
                    });
                    if changed {
                        shell.invalidate_thumbs_all();
                    }
                }
            }
        }
        cx.notify();
    });
}

/// Row icon slot (floating-menu contract): 14px box + clean gap, tinted
/// with the row ink (muted when the row is dead). None keeps a spacer so
/// labels align across iconed and iconless rows.
fn row_icon(icon: Option<&'static [u8]>, fg: u32, t: crate::theme::Theme) -> gpui::AnyElement {
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
                    .text_color(rgb(fg)),
            )
            .into_any_element(),
        None => div().w(px(14.)).h(px(14.)).into_any_element(),
    }
}

/// Keycap hint (dropdown.rs contract): flexible width, compact square for
/// single chars. Chords render spaced ("Ctrl + X"). Dead rows mute the ink
/// and drop opacity. Scale shrinks all px geometry (button pills reuse the
/// design smaller); the font step stays put.
pub(crate) fn keycap(
    shortcut: &str,
    t: crate::theme::Theme,
    dimmed: bool,
    scale: f32,
) -> gpui::AnyElement {
    let pretty: String = shortcut.split('+').collect::<Vec<_>>().join(" + ");
    let width = (18. + pretty.len() as f32 * 5.).max(18.) * scale;
    div()
        .min_w(px(18. * scale))
        .w(px(width))
        .h(px(16. * scale))
        .px(px(4. * scale))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5. * scale))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border_color))
        .opacity(if dimmed { 0.5 } else { 1.0 })
        .child(
            div()
                // Explicit size so scaled pills shrink their text too
                // (12px == text_xs, so scale 1.0 renders identical).
                .text_size(px(12. * scale))
                .font_family(crate::theme::FONT_UI)
                .text_color(rgb(if dimmed {
                    t.empty_text_secondary
                } else {
                    t.empty_text_primary
                }))
                .child(pretty),
        )
        .into_any_element()
}
