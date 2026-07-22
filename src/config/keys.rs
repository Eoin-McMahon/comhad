//! Configurable keybinds: parses `[keybinds.*]` tables from `~/.comhad/config.toml` into
//! per-context lookup tables, falling back to built-in defaults for anything unoverridden.
//! Each context has its own small `Action` enum and default key table; `input/mod.rs` looks a
//! pressed [`KeyCode`] up to get an `Action` rather than matching on raw key codes.
//!
//! Key-spec syntax: a single case-sensitive character, a named key (`up`, `down`, `left`,
//! `right`, `enter`, `esc`, `tab`, `backtab`, `backspace`, `delete`, `space`, `f1`..`f12`), or
//! a comma-separated list to bind multiple keys, e.g. `"up,k"`.

use std::collections::HashMap;

use crossterm::event::KeyCode;

use crate::config::KeybindsRaw;

fn parse_single_key(raw: &str) -> Option<KeyCode> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "up" => return Some(KeyCode::Up),
        "down" => return Some(KeyCode::Down),
        "left" => return Some(KeyCode::Left),
        "right" => return Some(KeyCode::Right),
        "enter" | "return" => return Some(KeyCode::Enter),
        "esc" | "escape" => return Some(KeyCode::Esc),
        "tab" => return Some(KeyCode::Tab),
        "backtab" | "shift+tab" => return Some(KeyCode::BackTab),
        "backspace" => return Some(KeyCode::Backspace),
        "delete" | "del" => return Some(KeyCode::Delete),
        "space" => return Some(KeyCode::Char(' ')),
        _ => {}
    }
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
    {
        return Some(KeyCode::F(n));
    }
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(KeyCode::Char(c)),
        _ => None,
    }
}

fn parse_keys(spec: &str) -> Vec<KeyCode> {
    spec.split(',').filter_map(parse_single_key).collect()
}

/// Builds a `KeyCode -> Action` table for one context: an override replaces all default keys
/// for that action; anything unmentioned keeps its built-in default key(s).
fn build_table<A: Copy + PartialEq>(
    defaults: &[(A, &[KeyCode])],
    overrides: &HashMap<String, String>,
    action_by_name: impl Fn(&str) -> Option<A>,
) -> HashMap<KeyCode, A> {
    let mut table = HashMap::new();
    for (name, spec) in overrides {
        if let Some(action) = action_by_name(name) {
            for key in parse_keys(spec) {
                table.insert(key, action);
            }
        }
    }
    for (action, keys) in defaults {
        let user_overrode = overrides.keys().any(|n| action_by_name(n) == Some(*action));
        if user_overrode {
            continue;
        }
        for key in *keys {
            table.entry(*key).or_insert(*action);
        }
    }
    table
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnPickerAction {
    Up,
    Down,
    Select,
    AddBookmark,
    EditBookmark,
    DeleteBookmark,
    ToggleTheme,
    Help,
    Quit,
}

const CONN_PICKER_DEFAULTS: &[(ConnPickerAction, &[KeyCode])] = &[
    (ConnPickerAction::Up, &[KeyCode::Up, KeyCode::Char('k')]),
    (ConnPickerAction::Down, &[KeyCode::Down, KeyCode::Char('j')]),
    (ConnPickerAction::Select, &[KeyCode::Enter]),
    (ConnPickerAction::AddBookmark, &[KeyCode::Char('a')]),
    (ConnPickerAction::EditBookmark, &[KeyCode::Char('e')]),
    (ConnPickerAction::DeleteBookmark, &[KeyCode::Char('x'), KeyCode::Delete]),
    (ConnPickerAction::ToggleTheme, &[KeyCode::Char('t')]),
    (ConnPickerAction::Help, &[KeyCode::Char('?')]),
    (ConnPickerAction::Quit, &[KeyCode::Char('q'), KeyCode::Esc]),
];

fn conn_picker_action_by_name(name: &str) -> Option<ConnPickerAction> {
    use ConnPickerAction::*;
    Some(match name {
        "up" => Up,
        "down" => Down,
        "select" => Select,
        "add_bookmark" => AddBookmark,
        "edit_bookmark" => EditBookmark,
        "delete_bookmark" => DeleteBookmark,
        "toggle_theme" => ToggleTheme,
        "help" => Help,
        "quit" => Quit,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketPickerAction {
    Up,
    Down,
    Select,
    ToggleTheme,
    Back,
}

const BUCKET_PICKER_DEFAULTS: &[(BucketPickerAction, &[KeyCode])] = &[
    (BucketPickerAction::Up, &[KeyCode::Up, KeyCode::Char('k')]),
    (BucketPickerAction::Down, &[KeyCode::Down, KeyCode::Char('j')]),
    (BucketPickerAction::Select, &[KeyCode::Enter]),
    (BucketPickerAction::ToggleTheme, &[KeyCode::Char('t')]),
    (BucketPickerAction::Back, &[KeyCode::Char('q'), KeyCode::Esc]),
];

fn bucket_picker_action_by_name(name: &str) -> Option<BucketPickerAction> {
    use BucketPickerAction::*;
    Some(match name {
        "up" => Up,
        "down" => Down,
        "select" => Select,
        "toggle_theme" => ToggleTheme,
        "back" => Back,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserAction {
    Quit,
    SwitchConnection,
    Help,
    Events,
    ToggleTheme,
    PreviewTab,
    InfoTab,
    ToggleLocal,
    FocusNext,
    FocusPrev,
    FocusLocal,
    FocusRemote,
    FocusPreview,
    FocusTransfers,
    SortName,
    SortSize,
    SortModified,
    OpenWebUrl,
    RevealInFinder,
    Up,
    Down,
    GoUp,
    EnterSelected,
    ToggleMark,
    ToggleVisual,
    Download,
    Upload,
    OpenSync,
    Delete,
    StageCopy,
    StageCut,
    Paste,
    CopyLocation,
    ShareUrl,
    Rename,
    Filter,
    Cancel,
}

const BROWSER_DEFAULTS: &[(BrowserAction, &[KeyCode])] = &[
    (BrowserAction::Quit, &[KeyCode::Char('q')]),
    (BrowserAction::SwitchConnection, &[KeyCode::Char('c')]),
    (BrowserAction::Help, &[KeyCode::Char('?')]),
    (BrowserAction::Events, &[KeyCode::Char('E')]),
    (BrowserAction::ToggleTheme, &[KeyCode::Char('t')]),
    (BrowserAction::PreviewTab, &[KeyCode::Char('p')]),
    (BrowserAction::InfoTab, &[KeyCode::Char('i')]),
    (BrowserAction::ToggleLocal, &[KeyCode::Char('L')]),
    (BrowserAction::FocusNext, &[KeyCode::Tab]),
    (BrowserAction::FocusPrev, &[KeyCode::BackTab]),
    (BrowserAction::FocusLocal, &[KeyCode::Char('1')]),
    (BrowserAction::FocusRemote, &[KeyCode::Char('2')]),
    (BrowserAction::FocusPreview, &[KeyCode::Char('3')]),
    (BrowserAction::FocusTransfers, &[KeyCode::Char('4')]),
    (BrowserAction::SortName, &[KeyCode::F(1)]),
    (BrowserAction::SortSize, &[KeyCode::F(2)]),
    (BrowserAction::SortModified, &[KeyCode::F(3)]),
    (BrowserAction::OpenWebUrl, &[KeyCode::Char('o')]),
    (BrowserAction::RevealInFinder, &[KeyCode::Char('f')]),
    (BrowserAction::Up, &[KeyCode::Up, KeyCode::Char('k')]),
    (BrowserAction::Down, &[KeyCode::Down, KeyCode::Char('j')]),
    (BrowserAction::GoUp, &[KeyCode::Left, KeyCode::Char('h'), KeyCode::Backspace]),
    (BrowserAction::EnterSelected, &[KeyCode::Right, KeyCode::Char('l'), KeyCode::Enter]),
    (BrowserAction::ToggleMark, &[KeyCode::Char(' ')]),
    (BrowserAction::ToggleVisual, &[KeyCode::Char('v')]),
    (BrowserAction::Download, &[KeyCode::Char('d')]),
    (BrowserAction::Upload, &[KeyCode::Char('u')]),
    (BrowserAction::OpenSync, &[KeyCode::Char('s')]),
    (BrowserAction::Delete, &[KeyCode::Char('D')]),
    (BrowserAction::StageCopy, &[KeyCode::Char('y')]),
    (BrowserAction::StageCut, &[KeyCode::Char('x')]),
    (BrowserAction::Paste, &[KeyCode::Char('P')]),
    (BrowserAction::CopyLocation, &[KeyCode::Char('Y')]),
    (BrowserAction::ShareUrl, &[KeyCode::Char('U')]),
    (BrowserAction::Rename, &[KeyCode::Char('r')]),
    (BrowserAction::Filter, &[KeyCode::Char('/')]),
    (BrowserAction::Cancel, &[KeyCode::Esc]),
];

fn browser_action_by_name(name: &str) -> Option<BrowserAction> {
    use BrowserAction::*;
    Some(match name {
        "quit" => Quit,
        "switch_connection" => SwitchConnection,
        "help" => Help,
        "events" => Events,
        "toggle_theme" => ToggleTheme,
        "preview_tab" => PreviewTab,
        "info_tab" => InfoTab,
        "toggle_local" => ToggleLocal,
        "focus_next" => FocusNext,
        "focus_prev" => FocusPrev,
        "focus_local" => FocusLocal,
        "focus_remote" => FocusRemote,
        "focus_preview" => FocusPreview,
        "focus_transfers" => FocusTransfers,
        "sort_name" => SortName,
        "sort_size" => SortSize,
        "sort_modified" => SortModified,
        "open_web_url" => OpenWebUrl,
        "reveal_in_finder" => RevealInFinder,
        "up" => Up,
        "down" => Down,
        "go_up" => GoUp,
        "enter_selected" => EnterSelected,
        "toggle_mark" => ToggleMark,
        "toggle_visual" => ToggleVisual,
        "download" => Download,
        "upload" => Upload,
        "open_sync" => OpenSync,
        "delete" => Delete,
        "stage_copy" => StageCopy,
        "stage_cut" => StageCut,
        "paste" => Paste,
        "copy_location" => CopyLocation,
        "share_url" => ShareUrl,
        "rename" => Rename,
        "filter" => Filter,
        "cancel" => Cancel,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAction {
    Up,
    Down,
}

const SCROLL_DEFAULTS: &[(ScrollAction, &[KeyCode])] =
    &[(ScrollAction::Up, &[KeyCode::Up, KeyCode::Char('k')]), (ScrollAction::Down, &[KeyCode::Down, KeyCode::Char('j')])];

fn scroll_action_by_name(name: &str) -> Option<ScrollAction> {
    use ScrollAction::*;
    Some(match name {
        "up" => Up,
        "down" => Down,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDialogAction {
    Close,
    Up,
    Down,
    FlipDirection,
    Confirm,
}

const SYNC_DEFAULTS: &[(SyncDialogAction, &[KeyCode])] = &[
    (SyncDialogAction::Close, &[KeyCode::Esc, KeyCode::Char('q')]),
    (SyncDialogAction::Up, &[KeyCode::Up, KeyCode::Char('k')]),
    (SyncDialogAction::Down, &[KeyCode::Down, KeyCode::Char('j')]),
    (SyncDialogAction::FlipDirection, &[KeyCode::Tab, KeyCode::Char('d')]),
    (SyncDialogAction::Confirm, &[KeyCode::Enter]),
];

fn sync_action_by_name(name: &str) -> Option<SyncDialogAction> {
    use SyncDialogAction::*;
    Some(match name {
        "close" => Close,
        "up" => Up,
        "down" => Down,
        "flip_direction" => FlipDirection,
        "confirm" => Confirm,
        _ => return None,
    })
}

/// The "are you sure?" yes/no dialog shown before download/upload/rename/sync/delete/paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDialogAction {
    ToggleSelection,
    Yes,
    No,
    Confirm,
}

const CONFIRM_DEFAULTS: &[(ConfirmDialogAction, &[KeyCode])] = &[
    (
        ConfirmDialogAction::ToggleSelection,
        &[KeyCode::Tab, KeyCode::BackTab, KeyCode::Left, KeyCode::Right],
    ),
    (ConfirmDialogAction::Yes, &[KeyCode::Char('y')]),
    (ConfirmDialogAction::No, &[KeyCode::Char('n'), KeyCode::Esc]),
    (ConfirmDialogAction::Confirm, &[KeyCode::Enter]),
];

fn confirm_action_by_name(name: &str) -> Option<ConfirmDialogAction> {
    use ConfirmDialogAction::*;
    Some(match name {
        "toggle_selection" => ToggleSelection,
        "yes" => Yes,
        "no" => No,
        "confirm" => Confirm,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkDeleteAction {
    Confirm,
}

const BOOKMARK_DELETE_DEFAULTS: &[(BookmarkDeleteAction, &[KeyCode])] =
    &[(BookmarkDeleteAction::Confirm, &[KeyCode::Char('y'), KeyCode::Enter])];

fn bookmark_delete_action_by_name(name: &str) -> Option<BookmarkDeleteAction> {
    use BookmarkDeleteAction::*;
    Some(match name {
        "confirm" => Confirm,
        _ => return None,
    })
}

/// Every context's `KeyCode -> Action` table, built once at startup from defaults plus config overrides.
pub struct Keybinds {
    pub conn_picker: HashMap<KeyCode, ConnPickerAction>,
    pub bucket_picker: HashMap<KeyCode, BucketPickerAction>,
    pub browser: HashMap<KeyCode, BrowserAction>,
    pub help: HashMap<KeyCode, ScrollAction>,
    pub events: HashMap<KeyCode, ScrollAction>,
    pub sync: HashMap<KeyCode, SyncDialogAction>,
    pub confirm: HashMap<KeyCode, ConfirmDialogAction>,
    pub bookmark_delete: HashMap<KeyCode, BookmarkDeleteAction>,
}

impl Keybinds {
    pub fn load(raw: &KeybindsRaw) -> Self {
        Self {
            conn_picker: build_table(CONN_PICKER_DEFAULTS, &raw.connection_picker, conn_picker_action_by_name),
            bucket_picker: build_table(BUCKET_PICKER_DEFAULTS, &raw.bucket_picker, bucket_picker_action_by_name),
            browser: build_table(BROWSER_DEFAULTS, &raw.browser, browser_action_by_name),
            help: build_table(SCROLL_DEFAULTS, &raw.help, scroll_action_by_name),
            events: build_table(SCROLL_DEFAULTS, &raw.events, scroll_action_by_name),
            sync: build_table(SYNC_DEFAULTS, &raw.sync, sync_action_by_name),
            confirm: build_table(CONFIRM_DEFAULTS, &raw.confirm, confirm_action_by_name),
            bookmark_delete: build_table(BOOKMARK_DELETE_DEFAULTS, &raw.bookmark_delete, bookmark_delete_action_by_name),
        }
    }
}

impl Default for Keybinds {
    fn default() -> Self {
        Self::load(&KeybindsRaw::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_keys_case_insensitively() {
        assert_eq!(parse_single_key("Up"), Some(KeyCode::Up));
        assert_eq!(parse_single_key("ESC"), Some(KeyCode::Esc));
        assert_eq!(parse_single_key("BackTab"), Some(KeyCode::BackTab));
        assert_eq!(parse_single_key("F3"), Some(KeyCode::F(3)));
    }

    #[test]
    fn parses_single_chars_case_sensitively() {
        assert_eq!(parse_single_key("q"), Some(KeyCode::Char('q')));
        assert_eq!(parse_single_key("Q"), Some(KeyCode::Char('Q')));
        assert_eq!(parse_single_key("?"), Some(KeyCode::Char('?')));
    }

    #[test]
    fn rejects_empty_or_multi_char_garbage() {
        assert_eq!(parse_single_key(""), None);
        assert_eq!(parse_single_key("nope"), None);
        assert_eq!(parse_single_key("xy"), None);
    }

    #[test]
    fn parse_keys_splits_a_comma_list() {
        assert_eq!(parse_keys("up, k"), vec![KeyCode::Up, KeyCode::Char('k')]);
    }

    #[test]
    fn default_conn_picker_table_matches_documented_defaults() {
        let binds = Keybinds::default();
        assert_eq!(binds.conn_picker.get(&KeyCode::Char('q')), Some(&ConnPickerAction::Quit));
        assert_eq!(binds.conn_picker.get(&KeyCode::Char('a')), Some(&ConnPickerAction::AddBookmark));
        assert_eq!(binds.conn_picker.get(&KeyCode::Char('k')), Some(&ConnPickerAction::Up));
    }

    #[test]
    fn override_replaces_default_key_for_that_action_only() {
        let mut raw = KeybindsRaw::default();
        raw.browser.insert("quit".to_string(), "Q".to_string());
        let binds = Keybinds::load(&raw);

        // The new key takes over the action...
        assert_eq!(binds.browser.get(&KeyCode::Char('Q')), Some(&BrowserAction::Quit));
        // ...and the old default key no longer does anything.
        assert_eq!(binds.browser.get(&KeyCode::Char('q')), None);
        // Unrelated actions keep their defaults.
        assert_eq!(binds.browser.get(&KeyCode::Char('t')), Some(&BrowserAction::ToggleTheme));
    }

    #[test]
    fn override_supports_binding_multiple_keys_to_one_action() {
        let mut raw = KeybindsRaw::default();
        raw.browser.insert("quit".to_string(), "q,ctrl".to_string()); // "ctrl" alone is unparseable, ignored
        let binds = Keybinds::load(&raw);
        assert_eq!(binds.browser.get(&KeyCode::Char('q')), Some(&BrowserAction::Quit));
    }

    #[test]
    fn unknown_action_name_in_config_is_ignored() {
        let mut raw = KeybindsRaw::default();
        raw.browser.insert("not_a_real_action".to_string(), "z".to_string());
        let binds = Keybinds::load(&raw);
        assert_eq!(binds.browser.get(&KeyCode::Char('z')), None);
        // Defaults are otherwise untouched.
        assert_eq!(binds.browser.get(&KeyCode::Char('q')), Some(&BrowserAction::Quit));
    }
}
