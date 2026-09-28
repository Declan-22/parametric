// ========================================
// DARK THEME
// ========================================

pub const DARK_BG_PRIMARY: u32 = 0x20211E;
pub const DARK_BG_SECONDARY: u32 = 0x292A27;
pub const DARK_BG_TERTIARY: u32 = 0x2D2E2B;
pub const DARK_BG_DARKER: u32 = 0x1C1D1A;

pub const DARK_BORDER_COLOR: u32 = 0x353632;
pub const DARK_MENU_BORDER_COLOR: u32 = 0x2E2F2B;
pub const DARK_COMPONENT_BORDER_COLOR: u32 = 0x292A27;

// Menu surfaces (ctx menus, app dropdown, mode options): panel/rest tone
// and hover-row tone, resolved per mode so dark menus stay dark without
// touching bg_tertiary (which other surfaces rely on).
pub const DARK_MENU_BG: u32 = DARK_BG_DARKER;
pub const DARK_MENU_HOVER_BG: u32 = 0x24251F;

pub const DARK_TEXT_PRIMARY: u32 = 0xFFFFFF;
pub const DARK_TEXT_SECONDARY: u32 = 0xBFBFBF;

pub const DARK_EMPTY_TEXT_PRIMARY: u32 = 0x838383;
pub const DARK_EMPTY_TEXT_SECONDARY: u32 = 0x535353;

pub const DARK_SHADOW_COLOR: u32 = 0x00000040;
pub const DARK_ITEM_SHADOW_COLOR: u32 = 0x00000024;

// High-contrast action buttons (e.g. "+ New design").
pub const DARK_BUTTON_BACKGROUND: u32 = 0xE8E9E4;
pub const DARK_BUTTON_TEXT: u32 = 0x1F1E1C;
pub const DARK_BUTTON_BORDER_COLOR: u32 = 0xB9BAB5;

pub const DARK_ACCENT: u32 = 0x789DD2;
pub const DARK_ACCENT_BORDER: u32 = 0x86A9DF;

// ========================================
// LIGHT THEME
// ========================================

pub const LIGHT_BG_PRIMARY: u32 = 0xEEEFE9;
pub const LIGHT_BG_SECONDARY: u32 = 0xE3E4DE;
pub const LIGHT_BG_TERTIARY: u32 = 0xF5F6F1;
pub const LIGHT_BG_DARKER: u32 = 0xE6E7E2;

pub const LIGHT_BORDER_COLOR: u32 = 0xD9DAD4;
pub const LIGHT_MENU_BORDER_COLOR: u32 = 0xDCDDD7;
pub const LIGHT_COMPONENT_BORDER_COLOR: u32 = 0xE3E4DE;

// Light values mirror bg_tertiary exactly: zero light-mode delta.
pub const LIGHT_MENU_BG: u32 = LIGHT_BG_TERTIARY;
pub const LIGHT_MENU_HOVER_BG: u32 = 0xEBEBE7;

pub const LIGHT_TEXT_PRIMARY: u32 = 0x1F1E1C;
pub const LIGHT_TEXT_SECONDARY: u32 = 0x55534E;

pub const LIGHT_EMPTY_TEXT_PRIMARY: u32 = 0x7E7D7A;
pub const LIGHT_EMPTY_TEXT_SECONDARY: u32 = 0xB6B5B4;

pub const LIGHT_SHADOW_COLOR: u32 = 0x00000012;
pub const LIGHT_ITEM_SHADOW_COLOR: u32 = 0x00000008;

// High-contrast action buttons (e.g. "+ New design").
pub const LIGHT_BUTTON_BACKGROUND: u32 = 0x232220;
pub const LIGHT_BUTTON_TEXT: u32 = 0xF2F2F2;
pub const LIGHT_BUTTON_BORDER_COLOR: u32 = 0x55534E;

pub const LIGHT_ACCENT: u32 = 0x81ACEC;
pub const LIGHT_ACCENT_BORDER: u32 = 0x7897C7;
