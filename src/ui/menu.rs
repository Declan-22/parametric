use gpui::{Action, SharedString};

use crate::ui::actions::*;

pub struct Menu {
    pub label: SharedString,
    pub items: Vec<MenuItem>,
}

pub struct MenuEntry {
    pub label: SharedString,
    pub shortcut: Option<SharedString>,
    pub action: Box<dyn Action>,
}

pub enum MenuItem {
    Separator,
    Entry(MenuEntry),
}

impl MenuEntry {
    pub fn new(label: impl Into<SharedString>, action: impl Action + Clone + 'static) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
            action: Box::new(action),
        }
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

impl MenuItem {
    pub fn entry(label: impl Into<SharedString>, action: impl Action + Clone + 'static) -> Self {
        Self::Entry(MenuEntry::new(label, action))
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        if let Self::Entry(ref mut entry) = self {
            entry.shortcut = Some(shortcut.into());
        }
        self
    }
}

pub fn default_menus() -> Vec<Menu> {
    vec![
        Menu {
            label: "File".into(),
            items: vec![
                MenuItem::entry("New Document", NewDocument).shortcut("Ctrl+N"),
                MenuItem::entry("Open Document…", OpenDocument).shortcut("Ctrl+O"),
                MenuItem::separator(),
                MenuItem::entry("Save", SaveDocument).shortcut("Ctrl+S"),
                MenuItem::entry("Save As…", SaveDocumentAs).shortcut("Ctrl+Shift+S"),
                MenuItem::separator(),
                MenuItem::entry("Export…", ExportDocument).shortcut("Ctrl+E"),
                MenuItem::separator(),
                MenuItem::entry("Quit", Quit).shortcut("Alt+F4"),
            ],
        },
        Menu {
            label: "Edit".into(),
            items: vec![
                MenuItem::entry("Undo", Undo).shortcut("Ctrl+Z"),
                MenuItem::entry("Redo", Redo).shortcut("Ctrl+Shift+Z"),
                MenuItem::separator(),
                MenuItem::entry("Cut", Cut).shortcut("Ctrl+X"),
                MenuItem::entry("Copy", Copy).shortcut("Ctrl+C"),
                MenuItem::entry("Paste", Paste).shortcut("Ctrl+V"),
                MenuItem::entry("Delete", DeleteSelection).shortcut("Del"),
                MenuItem::separator(),
                MenuItem::entry("Select All", SelectAll).shortcut("Ctrl+A"),
            ],
        },
        Menu {
            label: "View".into(),
            items: vec![
                MenuItem::entry("Zoom In", ZoomIn).shortcut("Ctrl+="),
                MenuItem::entry("Zoom Out", ZoomOut).shortcut("Ctrl+-"),
                MenuItem::entry("Zoom to Fit", ZoomToFit).shortcut("Ctrl+0"),
                MenuItem::separator(),
                MenuItem::entry("Toggle Theme", ToggleTheme),
            ],
        },
        Menu {
            label: "Object".into(),
            items: vec![
                MenuItem::entry("Group", GroupObjects).shortcut("Ctrl+G"),
                MenuItem::entry("Ungroup", UngroupObjects).shortcut("Ctrl+Shift+G"),
                MenuItem::separator(),
                MenuItem::entry("Transform…", TransformObjects),
            ],
        },
        Menu {
            label: "Arrange".into(),
            items: vec![
                MenuItem::entry("Bring to Front", BringToFront).shortcut("]"),
                MenuItem::entry("Bring Forward", BringForward).shortcut("["),
                MenuItem::entry("Send Backward", SendBackward).shortcut("Ctrl+["),
                MenuItem::entry("Send to Back", SendToBack).shortcut("Ctrl+]"),
            ],
        },
        Menu {
            label: "Constraints".into(),
            items: vec![
                MenuItem::entry("Add Constraint…", AddConstraint),
                MenuItem::separator(),
                MenuItem::entry("Toggle Constraints Panel", ToggleConstraintsPanel),
            ],
        },
        Menu {
            label: "Help".into(),
            items: vec![
                MenuItem::entry("Keyboard Shortcuts", ShowKeybindings).shortcut("Ctrl+/"),
                MenuItem::separator(),
                MenuItem::entry("About Parametric", About),
            ],
        },
    ]
}
