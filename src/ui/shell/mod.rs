use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, Context, Entity, MouseButton, Point, Window, div, prelude::*, px, rgb};

use crate::core::geometry::Point2;
use crate::core::constraints::ElementRef;
use crate::core::document::{Document, Layer};
use crate::editor::{Camera, Editor};
use crate::persistence::database::Database;
use crate::persistence::paths;
use crate::persistence::registry::{DesignMeta, Registry};
use crate::theme::{self, ThemeState};
pub mod title_bar;

use crate::ui::canvas::CanvasView;
use crate::ui::home::HomeView;
use crate::ui::menu::dropdown::AppMenu;
use crate::ui::shell::title_bar::{TITLE_BAR_HEIGHT, TitleBar};
use crate::ui::toolbar::Toolbar;

// What the shell is currently showing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    Home,
    Design(i64),
}

// Cached thumbnail source: the loaded document plus a fit-to-card camera.
pub(crate) struct Thumb {
    pub doc: Document,
    pub camera: Camera,
}

// Right-click menu state for a gallery card.
#[derive(Clone)]
pub(crate) struct DesignContextMenu {
    pub id: i64,
    pub position: Point<gpui::Pixels>,
}

// In-progress inline rename of a design.
pub(crate) struct RenameState {
    pub id: i64,
    pub value: String,
}

pub struct Shell {
    pub(crate) view: View,
    pub(crate) editor: Option<Entity<Editor>>,
    pub(crate) design_name: String,
    // Thumb cache: design id -> (updated_at it was loaded at, thumb).
    pub(crate) thumbs: HashMap<i64, (i64, Thumb)>,
    pub(crate) context_menu: Option<DesignContextMenu>,
    pub(crate) renaming: Option<RenameState>,
    // Focus handle used to capture keystrokes while renaming.
    pub(crate) rename_focus: gpui::FocusHandle,
    pub(crate) canvas_focus: gpui::FocusHandle,
    pub(crate) caret_visible: bool,
    pub(crate) new_design_opacity: f32,
    // Hover/active fade tweens: key -> 0..1 progress.
    pub(crate) fades: HashMap<String, f32>,
    pub(crate) fade_pending: HashMap<String, f32>,
    pub(crate) fade_tween_active: std::collections::HashSet<String>,
    pub(crate) menu_open: bool,
    pub(crate) active_menu: Option<usize>,
    pub(crate) menu_animation: f32,
    pub(crate) icon_animation: f32,
    pub(crate) cursor_trail: Vec<(Point<gpui::Pixels>, Instant)>,
    pub(crate) hovered_entry: Option<usize>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<ThemeState>(|_, cx| cx.notify())
            .detach();
        Self {
            view: View::Home,
            editor: None,
            design_name: String::new(),
            thumbs: HashMap::new(),
            context_menu: None,
            renaming: None,
            rename_focus: cx.focus_handle(),
            canvas_focus: cx.focus_handle(),
            caret_visible: true,
            new_design_opacity: 1.0,
            fades: HashMap::new(),
            fade_pending: HashMap::new(),
            fade_tween_active: std::collections::HashSet::new(),
            menu_open: false,
            active_menu: None,
            menu_animation: 0.0,
            icon_animation: 0.0,
            cursor_trail: Vec::new(),
            hovered_entry: None,
        }
    }

    // -- navigation --

    pub(crate) fn designs(&self, cx: &App) -> Vec<DesignMeta> {
        cx.try_global::<Registry>()
            .and_then(|r| r.list_designs().ok())
            .unwrap_or_default()
    }

    pub(crate) fn create_design(&mut self, cx: &mut Context<Self>) {
        let Some(reg) = cx.try_global::<Registry>() else {
            return;
        };
        let name = format!("Untitled {}", self.designs(cx).len() + 1);
        let Ok(path) = paths::new_document_path(&name) else {
            return;
        };
        // Create the document file with a default layer.
        let Ok(db) = Database::open(path.to_string_lossy().as_ref()) else {
            return;
        };
        let mut doc = Document::new();
        doc.layers.push(Layer {
            id: 1,
            name: "Layer 1".into(),
            elements: Vec::new(),
        });
        let _ = db.save_document(&doc);
        drop(db);

        let Ok(id) = reg.create_design(&name, &path, now_secs()) else {
            return;
        };
        self.open_design(id, cx);
    }

    pub(crate) fn open_design(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(meta) = self.designs(cx).into_iter().find(|d| d.id == id) else {
            return;
        };
        let Ok(db) = Database::open(meta.path.to_string_lossy().as_ref()) else {
            return;
        };
        let Ok(mut doc) = db.load_document() else {
            return;
        };
        drop(db);
        if doc.layers.is_empty() {
            doc.layers.push(Layer {
                id: 1,
                name: "Layer 1".into(),
                elements: Vec::new(),
            });
        }
        self.design_name = meta.name.clone();
        self.view = View::Design(id);
        self.editor = Some(cx.new(|_| Editor::from_document(doc)));
        cx.notify();
    }

    // Saves the open design back to its file and returns home.
    pub(crate) fn go_home(&mut self, cx: &mut Context<Self>) {
        if let (Some(editor), View::Design(id)) = (&self.editor, self.view) {
            if let Some(reg) = cx.try_global::<Registry>() {
                if let Ok(metas) = reg.list_designs()
                    && let Some(meta) = metas.iter().find(|m| m.id == id)
                    && let Ok(db) = Database::open(meta.path.to_string_lossy().as_ref())
                {
                    let _ = db.save_document(&editor.read(cx).doc);
                    drop(db);
                    let _ = reg.touch_design(id, now_secs());
                    // The cached thumbnail predates this save — drop it so
                    // the gallery re-loads the file.
                    self.invalidate_thumb(id);
                }
            }
        }
        self.editor = None;
        self.view = View::Home;
        cx.notify();
    }

    // -- gallery context menu + rename --

    pub(crate) fn open_context_menu(&mut self, id: i64, position: Point<gpui::Pixels>) -> bool {
        // The menu is positioned relative to the home container, which sits
        // below the title bar — convert from window coords + hug the cursor.
        let pos = Point::new(
            px(f32::from(position.x) + 2.),
            px(f32::from(position.y) - crate::ui::shell::title_bar::TITLE_BAR_HEIGHT + 2.),
        );
        self.context_menu = Some(DesignContextMenu { id, position: pos });
        true
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.is_none() {
            return;
        }
        self.context_menu = None;
        cx.notify();
    }

    pub(crate) fn start_rename(&mut self, id: i64, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let name = self
            .designs(cx)
            .into_iter()
            .find(|d| d.id == id)
            .map(|d| d.name)
            .unwrap_or_default();
        self.renaming = Some(RenameState { id, value: name });
        self.context_menu = None;
        self.caret_visible = true;
        window.focus(&self.rename_focus, cx);

        // Blink the caret while the rename input is active.
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(530))
                .await;
            let mut active = false;
            let _ = this.update(cx, |shell, cx| {
                if shell.renaming.is_some() {
                    active = true;
                    shell.caret_visible = !shell.caret_visible;
                    cx.notify();
                }
            });
            if !active {
                break;
            }
        })
        .detach();

        cx.notify();
    }

    pub(crate) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rn) = self.renaming.take() else {
            return;
        };
        let name = rn.value.trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        if let Some(reg) = cx.try_global::<Registry>() {
            let _ = reg.rename_design(rn.id, &name);
            // Keep the document file's name in sync with the design name.
            if let Some(old_path) = reg.design_path(rn.id)
                && old_path.file_stem().map(|s| s.to_string_lossy().to_string())
                    != Some(name.clone())
            {
                let new_path = old_path.with_file_name(format!("{name}.parametric"));
                if !new_path.exists()
                    && std::fs::rename(&old_path, &new_path).is_ok()
                {
                    // WAL sidecars move with the DB.
                    for ext in ["-wal", "-shm"] {
                        let from = PathBuf::from(format!("{}{ext}", old_path.display()));
                        let to = PathBuf::from(format!("{}{ext}", new_path.display()));
                        let _ = std::fs::rename(from, to);
                    }
                    let _ = reg.set_design_path(rn.id, &new_path);
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            cx.notify();
        }
    }

    // -- thumbnails --

    pub(crate) fn ensure_thumb(&mut self, meta: &DesignMeta, viewport: (f32, f32)) {
        // Reload whenever the design's updated_at moves past what we cached.
        if self.thumbs.get(&meta.id).map(|(at, _)| *at) != Some(meta.updated_at) {
            if let Ok(db) = Database::open(meta.path.to_string_lossy().as_ref())
                && let Ok(doc) = db.load_document()
            {
                drop(db);
                let camera = fit_camera(&doc, viewport);
                self.thumbs
                    .insert(meta.id, (meta.updated_at, Thumb { doc, camera }));
            }
        }
    }

    pub(crate) fn thumb_snapshot(&self, id: i64) -> Option<(Document, Camera)> {
        self.thumbs.get(&id).map(|(_, t)| (t.doc.clone(), t.camera))
    }

    pub(crate) fn invalidate_thumb(&mut self, id: i64) {
        self.thumbs.remove(&id);
    }

    // -- menu state --

    pub(crate) fn record_cursor(&mut self, pos: Point<gpui::Pixels>) {
        let now = Instant::now();
        self.cursor_trail.push((pos, now));
        self.cursor_trail
            .retain(|(_, t)| now.duration_since(*t).as_millis() <= 120);
    }

    // Velocity in px/ms over the recent trail.
    pub(crate) fn cursor_velocity(&self) -> (f32, f32) {
        let Some((first, t0)) = self.cursor_trail.first() else {
            return (0., 0.);
        };
        let Some((last, t1)) = self.cursor_trail.last() else {
            return (0., 0.);
        };
        let dt = t1.duration_since(*t0).as_secs_f32();
        if dt < f32::EPSILON {
            return (0., 0.);
        }
        (
            (last.x - first.x).as_f32() / dt,
            (last.y - first.y).as_f32() / dt,
        )
    }

    pub(crate) fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        self.active_menu = None;
        self.menu_animation = 0.0;
        self.animate_icon(self.menu_open, cx);
        if self.menu_open {
            self.start_menu_animation(cx);
        }
    }

    // Fades the menu icon between its idle and active styling.
    pub(crate) fn animate_icon(&mut self, opening: bool, cx: &mut Context<Self>) {
        let start = self.icon_animation;
        let end = if opening { 1.0 } else { 0.0 };
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let steps = 6;
            for i in 1..=steps {
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    shell.icon_animation = start + (end - start) * (i as f32 / steps as f32);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // -- hover fades --

    pub(crate) fn fade(&self, key: &str) -> f32 {
        self.fades.get(key).copied().unwrap_or(0.0)
    }

    // Tweens a named fade toward the target (0 or 1). A single ticker per
    // key eases exponentially toward whatever the pending target is, so
    // rapid hover in/out never fights itself.
    pub(crate) fn animate_fade(&mut self, key: &str, target: f32, cx: &mut Context<Self>) {
        self.fade_pending.insert(key.to_string(), target);
        if !self.fade_tween_active.insert(key.to_string()) {
            return;
        }
        let key_owned = key.to_string();
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
                let mut done = false;
                let _ = this.update(cx, |shell, cx| {
                    if let Some(&target) = shell.fade_pending.get(&key_owned) {
                        let cur = shell.fade(&key_owned);
                        let next = cur + (target - cur) * 0.4;
                        if (next - target).abs() < 0.01 {
                            shell.fades.insert(key_owned.clone(), target);
                            done = true;
                        } else {
                            shell.fades.insert(key_owned.clone(), next);
                        }
                        cx.notify();
                    } else {
                        done = true;
                    }
                });
                if !done {
                    continue;
                }
                let _ = this.update(cx, |shell, _| {
                    shell.fade_pending.remove(&key_owned);
                    shell.fade_tween_active.remove(&key_owned);
                });
                break;
            }
        })
        .detach();
    }
    pub(crate) fn animate_new_design(&mut self, target: f32, cx: &mut Context<Self>) {
        let start = self.new_design_opacity;
        if (start - target).abs() < f32::EPSILON {
            return;
        }
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let steps = 6;
            for i in 1..=steps {
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    shell.new_design_opacity = start + (target - start) * (i as f32 / steps as f32);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // -- edit operations (copy / cut / paste / delete) --

    fn clipboard_data(&self, cx: &Context<Self>) -> Option<String> {
        let ed = self.editor.as_ref()?;
        let e = ed.read(cx);
        if e.selection.len() != 1 {
            return None;
        }
        let sel = e.selection[0];
        let pts = e.doc.element_points(sel);
        let b = e.doc.bounds_of_points(&pts)?;
        Some(format!(
            "parametric/bounds:{},{},{},{}",
            b.origin.x,
            b.origin.y,
            b.size.w,
            b.size.h
        ))
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(data) = self.clipboard_data(cx) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(data));
        }
    }

    pub(crate) fn cut_selection(&mut self, cx: &mut Context<Self>) {
        self.copy_selection(cx);
        self.delete_selection(cx);
    }

    pub(crate) fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else {
            return;
        };
        let Some(rest) = text.strip_prefix("parametric/bounds:") else {
            return;
        };
        let nums: Vec<f64> = rest.split(',').filter_map(|s| s.parse().ok()).collect();
        if nums.len() != 4 {
            return;
        }
        let Some(ed) = self.editor.as_ref() else {
            return;
        };
        ed.update(cx, |ed, cx| {
            // Paste slightly offset so it doesn't land exactly on the copy.
            const OFFSET: f64 = 12.;
            let a = Point2::new(nums[0] + OFFSET, nums[1] + OFFSET);
            let b = Point2::new(nums[0] + OFFSET + nums[2], nums[1] + OFFSET + nums[3]);
            let layer_id = ed.doc.layers[0].id;
            let fill = ed.create_rectangle(layer_id, a, b);
            ed.selection = vec![ElementRef::Fill(fill)];
            ed.update_dim_geom();
            cx.notify();
        });
    }

    pub(crate) fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.editor.as_ref() else {
            return;
        };
        let removed = ed.update(cx, |ed, _| {
            // Selected constraint chips die first: deleting one just drops
            // the constraint, freeing its points.
            let mut removed_any = false;
            if !ed.selected_constraints.is_empty() {
                let dead = std::mem::take(&mut ed.selected_constraints);
                ed.doc.constraints.retain(|c| !dead.contains(c));
                removed_any = true;
            }
            if ed.selection.is_empty() {
                return removed_any;
            }
            let sels = std::mem::take(&mut ed.selection);
            for el in &sels {
                ed.delete_element(*el);
            }
            true
        });
        if removed {
            self.invalidate_thumbs_all();
            cx.notify();
        }
    }

    pub(crate) fn invalidate_thumbs_all(&mut self) {
        self.thumbs.clear();
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if !self.menu_open {
            return;
        }
        self.menu_open = false;
        self.active_menu = None;
        self.menu_animation = 0.0;
        self.animate_icon(false, cx);
        cx.notify();
    }

    pub(crate) fn set_active_menu(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.active_menu == index {
            return;
        }
        self.active_menu = index;
        cx.notify();
    }

    pub(crate) fn start_menu_animation(&mut self, cx: &mut Context<Self>) {
        self.menu_animation = 0.0;
        let this = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let steps = 6;
            for i in 1..=steps {
                cx.background_executor()
                    .timer(Duration::from_millis(12))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    shell.menu_animation = i as f32 / steps as f32;
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

// Camera that fits the document's content into a viewport with padding.
fn fit_camera(doc: &Document, viewport: (f32, f32)) -> Camera {
    let mut bounds: Option<crate::core::geometry::Rect> = None;
    for layer in &doc.layers {
        for &el in &layer.elements {
            let pts = doc.element_points(el);
            if let Some(b) = doc.bounds_of_points(&pts) {
                bounds = Some(match bounds {
                    Some(acc) => acc.union(&b),
                    None => b,
                });
            }
        }
    }
    let mut cam = Camera::new();
    let Some(b) = bounds else {
        return cam;
    };
    let pad = 16.;
    let zw = (viewport.0 - pad * 2.).max(1.) as f64 / b.size.w.max(1.);
    let zh = (viewport.1 - pad * 2.).max(1.) as f64 / b.size.h.max(1.);
    cam.set_zoom(zw.min(zh));
    let c = b.center();
    cam.pan = crate::core::geometry::Point2::new(
        c.x - viewport.0 as f64 / (2. * cam.zoom),
        c.y - viewport.1 as f64 / (2. * cam.zoom),
    );
    cam
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let shell_keys = cx.entity().downgrade();

        div()
            .track_focus(&self.rename_focus)
            .on_action(cx.listener(|shell, _: &crate::ui::actions::Copy, _, cx| {
                shell.copy_selection(cx);
            }))
            .on_action(cx.listener(|shell, _: &crate::ui::actions::Cut, _, cx| {
                shell.cut_selection(cx);
            }))
            .on_action(cx.listener(|shell, _: &crate::ui::actions::Paste, _, cx| {
                shell.paste_clipboard(cx);
            }))
            .on_action(cx.listener(|shell, _: &crate::ui::actions::DeleteSelection, _, cx| {
                shell.delete_selection(cx);
            }))
            .on_key_down(move |e: &gpui::KeyDownEvent, _, cx| {
                // Only capture keys while the inline rename input is active.
                if shell_keys.upgrade().and_then(|s| Some(s.read(cx).renaming.is_some())) != Some(true) {
                    return;
                }
                let handled = match e.keystroke.key.as_str() {
                    "enter" | "escape" | "backspace" | "space" => true,
                    k => k.len() == 1 && !e.keystroke.modifiers.modified(),
                };
                if !handled {
                    return;
                }
                cx.stop_propagation();
                let key = e.keystroke.key.clone();
                let _ = shell_keys.update(cx, |shell, cx| match key.as_str() {
                    "enter" => shell.commit_rename(cx),
                    "escape" => shell.cancel_rename(cx),
                    "backspace" => {
                        if let Some(rn) = &mut shell.renaming {
                            rn.value.pop();
                            cx.notify();
                        }
                    }
                    "space" => {
                        if let Some(rn) = &mut shell.renaming {
                            rn.value.push(' ');
                            shell.caret_visible = true;
                            cx.notify();
                        }
                    }
                    _ => {
                        if let Some(rn) = &mut shell.renaming {
                            rn.value.push_str(&key);
                            shell.caret_visible = true;
                            cx.notify();
                        }
                    }
                });
            })
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            .text_color(rgb(t.text_primary))
            .font_family(theme::FONT_UI)
            .child(TitleBar {
                menu_open: self.menu_open,
                icon_animation: self.icon_animation,
                home_active: self.view == View::Home,
                shell: cx.entity().downgrade(),
            })
            .child(match self.view {
                View::Home => {
                    let mut home = div()
                        .flex_1()
                        .relative()
                        .overflow_hidden()
                        .child(HomeView {
                            shell: cx.entity().downgrade(),
                        });
                    // Click-away catcher for the context menu and the
                    // inline rename input.
                    if self.context_menu.is_some() || self.renaming.is_some() {
                        let shell_l = cx.entity().downgrade();
                        let shell_r = cx.entity().downgrade();
                        home = home.child(
                            div()
                                .absolute()
                                .inset_0()
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    let _ = shell_l.update(cx, |shell, cx| {
                                        shell.close_context_menu(cx);
                                        shell.cancel_rename(cx);
                                    });
                                })
                                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                                    let _ = shell_r.update(cx, |shell, cx| {
                                        shell.close_context_menu(cx);
                                        shell.cancel_rename(cx);
                                    });
                                }),
                        );
                    }
                    if let Some(cm) = self.context_menu.clone() {
                        home = home.child(crate::ui::home::render_context_menu(
                            &cm,
                            cx.entity().downgrade(),
                            *t,
                            cx,
                        ));
                    }
                    home.into_any_element()
                }
                View::Design(_) => {
                    let editor = self.editor.clone().expect("design view without editor");
                    div()
                        .flex_1()
                        .relative()
                        .overflow_hidden()
                        .child(CanvasView {
                            editor: editor.downgrade(),
                            shell: cx.entity().downgrade(),
                            focus: self.canvas_focus.clone(),
                        })
                        .child(Toolbar {
                            editor: editor.downgrade(),
                            shell: cx.entity().downgrade(),
                        })
                        .into_any_element()
                }
            })
            .when(self.menu_open, |d| {
                let shell = cx.entity().downgrade();
                d.child(
                    // Click-away catcher below the title bar.
                    div()
                        .absolute()
                        .inset_0()
                        .top(px(TITLE_BAR_HEIGHT))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            let _ = shell.update(cx, |shell, cx| shell.close_menu(cx));
                        }),
                )
                .child(AppMenu {
                    shell: cx.entity().downgrade(),
                    active: self.active_menu,
                    animation: self.menu_animation,
                })
            })
    }
}

