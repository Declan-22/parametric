// Layers panel: absolute left dock between the tool rail and the canvas
// (overlay, like the inspector — the canvas keeps drawing underneath).
// Opens/closes via the ModeBar sidebar toggle ONLY (no close button on
// the panel itself) with a slide tween on the shared fade infra.
//
// The tree shows real `Document.layers`. Groups are just layers with
// children; a group's eye folds its whole subtree by derivation. This
// is the seed of the parametric hierarchy (Layers -> Bodies/islands ->
// geometry -> modifiers): bodies and modifiers will slot in as new row
// kinds without changing the panel contract.

use gpui::{
    AnyElement, App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb,
    rgba, svg,
};

use crate::core::document::{BodyShape, LayerKind};
use crate::editor::InspectorField;
use crate::theme::{Theme, fade_in, lerp_rgb};
use crate::ui::modebar::MODEBAR_HEIGHT;

pub const LAYERS_WIDTH: f32 = 264.0;

const ROW_H: f32 = 28.0;
const INDENT: f32 = 16.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LayersTab {
    #[default]
    Layers,
    Constraints,
}

#[derive(IntoElement)]
pub struct LayerPanel {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

const CHEVRON_RIGHT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m9 6l6 6l-6 6" /></svg>"#;
const CHEVRON_DOWN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m6 9l6 6l6-6" /></svg>"#;
const EYE_OPEN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"><path d="M2.062 12.348a1 1 0 0 1 0-.696a10.75 10.75 0 0 1 19.876 0a1 1 0 0 1 0 .696a10.75 10.75 0 0 1-19.876 0" /><circle cx="12" cy="12" r="3" /></g></svg>"#;
const EYE_OFF: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"><path d="M10.733 5.076a10.7 10.7 0 0 1 8.205 3.58a1 1 0 0 1 0 1.392a10.75 10.75 0 0 1-3.437 2.371m-2.195 1.143a10.7 10.7 0 0 1-4.744 1.05a10.7 10.7 0 0 1-4.5-.964a1 1 0 0 1 0-1.39a10.7 10.7 0 0 1 3.435-2.369m2.196-1.142A10.76 10.76 0 0 1 12 7.652" /><path d="m2 2l20 20" /><path d="M9.88 4.24A9.1 9.1 0 0 1 12 4c7 0 10 8 10 8a13.2 13.2 0 0 1-1.67 2.68" /></g></svg>"#;
// Per-kind row icons: the icon IS the layer — folder for groups,
// shape glyphs for bodies (derived live from contents).
const ICON_BODY_RECT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><rect width="16" height="12" x="4" y="6" rx="1.5" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" /></svg>"#;
const ICON_BODY_CIRCLE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" stroke-width="1.5" /></svg>"#;
const ICON_BODY_ARC: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="M4 18a8 8 0 0 1 16 0" /></svg>"#;
const ICON_BODY_LINE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="M6 18L18 6" /><circle cx="5.5" cy="18.5" r="1.7" fill="currentColor" /><circle cx="18.5" cy="5.5" r="1.7" fill="currentColor" /></svg>"#;
const ICON_BODY_CURVE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="M5 19c0-6 4-6 7-7s7-1 7-7" /></svg>"#;
const ICON_BODY_PATH: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 18l6-8l4 4l6-8" /></svg>"#;
const ICON_BODY_SHAPE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><rect width="14" height="14" x="5" y="5" rx="3" fill="currentColor" /></svg>"#;
const ICON_BODY_POINTS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><circle cx="6" cy="13" r="1.8" fill="currentColor" /><circle cx="12" cy="7" r="1.8" fill="currentColor" /><circle cx="17.5" cy="15" r="1.8" fill="currentColor" /></svg>"#;
const ICON_BODY_RULERS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"><rect width="18" height="6" x="3" y="9" rx="1" /><path d="M7 9v3M11 9v2M15 9v3" /></g></svg>"#;
const ICON_BODY_EMPTY: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><rect width="14" height="14" x="5" y="5" rx="2" fill="none" stroke="currentColor" stroke-dasharray="3 2" stroke-width="1.5" /></svg>"#;
const ICON_ORGANIZE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="currentColor" d="M12 3l1.7 4.3L18 9l-4.3 1.7L12 15l-1.7-4.3L6 9l4.3-1.7zM19 15l.9 2.1L22 18l-2.1.9L19 21l-.9-2.1L16 18l2.1-.9z" /></svg>"#;
const ICON_PLUS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="M12 5v14M5 12h14" /></svg>"#;
const ICON_GROUP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="none" /><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /></svg>"#;

// One flattened tree row for render.
struct TreeRow {
    id: u64,
    name: String,
    kind: Option<LayerKind>,
    shape: BodyShape,
    self_visible: bool,
    effective_visible: bool,
    depth: usize,
    has_children: bool,
    expanded: bool,
}

fn layer_icon(kind: Option<LayerKind>, has_children: bool, shape: BodyShape) -> &'static [u8] {
    if has_children || kind == Some(LayerKind::Group) {
        return ICON_GROUP;
    }
    match shape {
        BodyShape::Rectangle => ICON_BODY_RECT,
        BodyShape::Circle => ICON_BODY_CIRCLE,
        BodyShape::Arc => ICON_BODY_ARC,
        BodyShape::Line => ICON_BODY_LINE,
        BodyShape::Curve => ICON_BODY_CURVE,
        BodyShape::Path => ICON_BODY_PATH,
        BodyShape::Shape => ICON_BODY_SHAPE,
        BodyShape::Points => ICON_BODY_POINTS,
        BodyShape::Rulers => ICON_BODY_RULERS,
        BodyShape::Empty => ICON_BODY_EMPTY,
    }
}

impl RenderOnce for LayerPanel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let k = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).fade("layers-panel"))
            .unwrap_or(0.0);
        if k < 0.01 {
            return div().into_any_element();
        }
        // Smoothstep on the shared exponential tween: fast attack, soft
        // landing — the "super clean" slide. Far-left dock: hidden
        // off-screen, open at x=0 with the tool rail riding its edge.
        let e = k * k * (3.0 - 2.0 * k);
        let left = -LAYERS_WIDTH * (1.0 - e);

        let Some(shell_ent) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let Some(ed_ent) = self.editor.upgrade() else {
            return div().into_any_element();
        };
        let (tab, collapsed, hovered) = {
            let s = shell_ent.read(cx);
            (s.layers_tab, s.layers_collapsed.clone(), s.hovered_layer)
        };
        let (rows, active, rename_value) = {
            let ed = ed_ent.read(cx);
            let mut rows = Vec::new();
            flatten(&ed.doc, None, 0, &collapsed, &mut rows);
            let rename_value = match &ed.inspector_input {
                Some(input) => match &input.field {
                    InspectorField::LayerName(rid) => Some((*rid, input.value.clone())),
                    _ => None,
                },
                None => None,
            };
            (rows, ed.active_layer, rename_value)
        };

        let mut root = div()
            .absolute()
            .top(px(MODEBAR_HEIGHT))
            .bottom_0()
            .left(px(left))
            .w(px(LAYERS_WIDTH))
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            .border_r_1()
            .border_color(rgb(t.component_border_color))
            // The panel owns its strip: nothing leaks to the canvas
            // beneath (same contract as the inspector dock).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.tab_strip(tab, t, cx));
        if tab == LayersTab::Layers {
            let mut list = div().id("layers-list").flex_1().flex().flex_col().overflow_y_scroll();
            for row in &rows {
                list = list.child(layer_row(
                    row,
                    active == row.id,
                    hovered == Some(row.id),
                    rename_value.clone(),
                    t,
                    self.editor.clone(),
                    self.shell.clone(),
                ));
            }
            root = root.child(list);
        } else {
            root = root.child(self.constraints_tab(t, cx));
        }
        root.into_any_element()
    }
}

// Pre-order flatten of the layer tree, skipping collapsed subtrees.
fn flatten(
    doc: &crate::core::document::Document,
    parent: Option<u64>,
    depth: usize,
    collapsed: &std::collections::HashSet<u64>,
    out: &mut Vec<TreeRow>,
) {
    for id in doc.layer_children(parent) {
        let Some(layer) = doc.layer(id) else { continue };
        let kids = doc.layer_children(Some(id));
        let expanded = !collapsed.contains(&id);
        out.push(TreeRow {
            id,
            name: layer.name.clone(),
            kind: layer.kind,
            shape: doc.body_shape(id),
            self_visible: layer.visible,
            effective_visible: doc.layer_effective_visible(id),
            depth,
            has_children: !kids.is_empty(),
            expanded,
        });
        if expanded {
            flatten(doc, Some(id), depth + 1, collapsed, out);
        }
    }
}

impl LayerPanel {
    fn tab_strip(&self, tab: LayersTab, t: Theme, cx: &App) -> AnyElement {
        let shell = self.shell.clone();
        let tab_btn = |which: LayersTab, label: &'static str| {
            let shell_c = shell.clone();
            let selected = tab == which;
            div()
                .px(px(10.))
                .h(px(32.))
                .flex()
                .items_center()
                .cursor_pointer()
                .border_b_1()
                .border_color(if selected {
                    rgb(t.accent)
                } else {
                    rgba(0x00000000)
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(if selected {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::NORMAL
                        })
                        .text_color(rgb(if selected {
                            t.text_primary
                        } else {
                            t.text_secondary
                        }))
                        .child(label),
                )
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = shell_c.update(cx, |shell, cx| {
                        shell.layers_tab = which;
                        cx.notify();
                    });
                })
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(rgb(t.component_border_color))
            .child(tab_btn(LayersTab::Layers, "Layers"))
            .child(tab_btn(LayersTab::Constraints, "Constraints"))
            .child(div().flex_1())
            .when(tab == LayersTab::Layers, |d| {
                d.child(self.organize_button(t, cx))
                    .child(self.add_button(t, false, cx))
                    .child(self.add_button(t, true, cx))
            })
            .into_any_element()
    }

    // Organize: splits flat/multi-island layers into one body layer per
    // island (shape-named). Geometry ids are stable, so selection and
    // constraints survive.
    fn organize_button(&self, t: Theme, cx: &App) -> AnyElement {
        let editor = self.editor.clone();
        let key = "layers-organize";
        let k = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).fade(key))
            .unwrap_or(0.0);
        let bg = lerp_rgb(t.bg_primary, t.bg_secondary, k);
        let shell_hov = self.shell.clone();
        div()
            .id(gpui::SharedString::from(key))
            .w(px(24.))
            .h(px(24.))
            .mr(px(4.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(rgb(bg))
            .text_color(rgb(t.text_secondary))
            .on_hover(move |hovered, _, cx| {
                let _ = shell_hov.update(cx, |shell, cx| {
                    shell.animate_fade(key, if *hovered { 1.0 } else { 0.0 }, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| ed.organize_layers(cx));
            })
            .child(svg().data(ICON_ORGANIZE).w(px(14.)).h(px(14.)))
            .into_any_element()
    }

    // Header buttons: `+` layer, folder group. Styled like toolbar rail
    // buttons at panel scale (24px, hover bg_secondary).
    fn add_button(&self, t: Theme, group: bool, cx: &App) -> AnyElement {
        let editor = self.editor.clone();
        let key = if group { "layers-add-group" } else { "layers-add" };
        let k = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).fade(key))
            .unwrap_or(0.0);
        let bg = lerp_rgb(t.bg_primary, t.bg_secondary, k);
        let shell_hov = self.shell.clone();
        let key_owned = key.to_string();
        div()
            .id(gpui::SharedString::from(key_owned.clone()))
            .w(px(24.))
            .h(px(24.))
            .mr(px(4.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(rgb(bg))
            .text_color(rgb(t.text_secondary))
            .on_hover(move |hovered, _, cx| {
                let _ = shell_hov.update(cx, |shell, cx| {
                    shell.animate_fade(&key_owned, if *hovered { 1.0 } else { 0.0 }, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| {
                    // New rows land INSIDE the active layer when it is
                    // structural (empty), else beside it as a sibling.
                    let (parent, name) = {
                        let active_empty = ed
                            .doc
                            .layer(ed.active_layer)
                            .is_some_and(|l| l.elements.is_empty());
                        let parent = if active_empty {
                            Some(ed.active_layer)
                        } else {
                            ed.doc.layer(ed.active_layer).and_then(|l| l.parent)
                        };
                        let base = if group { "Group" } else { "Layer" };
                        let mut n = ed.doc.layers.len() + 1;
                        while ed
                            .doc
                            .layers
                            .iter()
                            .any(|l| l.name == format!("{base} {n}"))
                        {
                            n += 1;
                        }
                        (parent, format!("{base} {n}"))
                    };
                    ed.add_layer_at(&name, parent, if group { LayerKind::Group } else { LayerKind::Body });
                    cx.notify();
                });
            })
            .child(
                svg()
                    .data(if group { ICON_GROUP } else { ICON_PLUS })
                    .w(px(14.))
                    .h(px(14.)),
            )
            .into_any_element()
    }

    fn constraints_tab(&self, t: Theme, cx: &App) -> AnyElement {
        let (n_con, n_dim) = self
            .editor
            .upgrade()
            .map(|e| {
                let ed = e.read(cx);
                (ed.doc.constraints.len(), ed.doc.dimensions.len())
            })
            .unwrap_or((0, 0));
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(t.text_secondary))
                    .child(format!("{n_con} constraints · {n_dim} dimensions")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(lerp_rgb(t.text_secondary, t.bg_primary, 0.4)))
                    .child("Formula editing lands here — pick a constraint to drive it by equation."),
            )
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_row(
    row: &TreeRow,
    active: bool,
    hovered: bool,
    rename_value: Option<(u64, String)>,
    t: Theme,
    editor: gpui::WeakEntity<crate::editor::Editor>,
    shell: gpui::WeakEntity<crate::ui::shell::Shell>,
) -> AnyElement {
    let id = row.id;
    let ed_select = editor.clone();
    let shell_collapse = shell.clone();
    let shell_hover = shell.clone();
    let ed_eye = editor.clone();
    let ed_delete = editor.clone();

    let bg = if active {
        t.bg_tertiary
    } else if hovered {
        t.bg_secondary
    } else {
        t.bg_primary
    };
    // Ancestor-hidden rows read dimmed (their own eye still toggles).
    let label_color = if row.effective_visible {
        t.text_primary
    } else {
        lerp_rgb(t.text_secondary, t.bg_primary, 0.35)
    };
    let icon_color = lerp_rgb(t.text_secondary, t.bg_primary, 0.15);

    // Indent guides: one hairline per depth level (reference look).
    let mut line = div().flex().flex_row().items_center().h(px(ROW_H)).w_full();
    for _ in 0..row.depth {
        line = line.child(
            div()
                .w(px(INDENT))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().w(px(1.)).h_full().bg(rgba(fade_in(
                    (t.border_color << 8) | 0xFF,
                    0.55,
                )))),
        );
    }

    // Disclosure: chevron when the layer has children, spacer when not.
    if row.has_children {
        let icon = if row.expanded { CHEVRON_DOWN } else { CHEVRON_RIGHT };
        line = line.child(
            div()
                .w(px(INDENT))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(rgb(icon_color))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = shell_collapse.update(cx, |shell, cx| {
                        if !shell.layers_collapsed.remove(&id) {
                            shell.layers_collapsed.insert(id);
                        }
                        cx.notify();
                    });
                })
                .child(svg().data(icon).w(px(12.)).h(px(12.))),
        );
    } else {
        line = line.child(div().w(px(INDENT)));
    }

    // Eye: every row carries one (reference look); a group's toggle
    // folds its whole subtree by derivation.
    let eye = if row.self_visible { EYE_OPEN } else { EYE_OFF };
    let eye_color = if row.self_visible {
        icon_color
    } else {
        lerp_rgb(t.text_secondary, t.bg_primary, 0.5)
    };
    line = line.child(
        div()
            .w(px(20.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(rgb(eye_color))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = ed_eye.update(cx, |ed, cx| ed.toggle_layer_visible(id, cx));
            })
            .child(svg().data(eye).w(px(14.)).h(px(14.))),
    );

    // Kind icon on EVERY row: the icon is the layer (folder for
    // groups, shape glyph for bodies).
    line = line.child(
        div()
            .w(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(icon_color))
            .child(
                svg()
                    .data(layer_icon(row.kind, row.has_children, row.shape))
                    .w(px(13.))
                    .h(px(13.)),
            ),
    );

    // Label or inline rename box (Enter commits, Esc cancels — the
    // shared inspector-input pipeline; keystrokes already route).
    match rename_value {
        Some((rid, value)) if rid == id => {
            line = line.child(
                div()
                    .flex_1()
                    .h(px(20.))
                    .px(px(6.))
                    .mr(px(4.))
                    .flex()
                    .items_center()
                    .bg(rgb(t.bg_tertiary))
                    .border_1()
                    .border_color(rgb(t.accent_border))
                    .rounded(px(6.))
                    .text_xs()
                    .text_color(rgb(t.text_primary))
                    .child(value),
            );
        }
        _ => {
            line = line.child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(label_color))
                    .overflow_hidden()
                    .child(row.name.clone()),
            );
        }
    }

    // Hover X: delete (no confirmation — undo restores; the last layer
    // standing empties instead of dying).
    if hovered {
        line = line.child(
            div()
                .w(px(20.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(rgb(icon_color))
                .text_xs()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = ed_delete.update(cx, |ed, cx| ed.delete_layer(id, cx));
                })
                .child("✕"),
        );
    }

    div()
        .id(gpui::SharedString::from(format!("layer-row-{id}")))
        .w_full()
        .bg(rgb(bg))
        .cursor_pointer()
        .on_hover(move |h, _, cx| {
            let _ = shell_hover.update(cx, |shell, cx| {
                let next = if *h { Some(id) } else { None };
                if shell.hovered_layer != next {
                    shell.hovered_layer = next;
                    cx.notify();
                }
            });
        })
        .on_mouse_down(MouseButton::Left, move |e, _, cx| {
            cx.stop_propagation();
            let dbl = e.click_count == 2;
            let _ = ed_select.update(cx, |ed, cx| {
                if dbl {
                    if let Some(layer) = ed.doc.layer(id) {
                        let name = layer.name.clone();
                        ed.begin_inspector_input(InspectorField::LayerName(id), name, cx);
                    }
                    return;
                }
                // Click = work here: contents selected, layer active for
                // new geometry. Groups select their whole subtree.
                let subtree = ed.doc.layer_subtree_ids(id);
                let mut els = Vec::new();
                for sid in &subtree {
                    if let Some(layer) = ed.doc.layer(*sid) {
                        for &el in &layer.elements {
                            if !els.contains(&el) {
                                els.push(el);
                            }
                        }
                    }
                }
                ed.selection = els;
                ed.active_layer = id;
                cx.notify();
            });
        })
        .child(line)
        .into_any_element()
}
