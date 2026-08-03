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
    RenameProfile,
    ToggleTheme,
    Help,
    Quit,
}

const CONN_PICKER_DEFAULTS: &[(ConnPickerAction, &[KeyCode])] = &[
    (ConnPickerAction::Up, &[KeyCode::Up, KeyCode::Char('k')]),
    (ConnPickerAction::Down, &[KeyCode::Down, KeyCode::Char('j')]),
    (ConnPickerAction::Select, &[KeyCode::Enter, KeyCode::Char('l'), KeyCode::Right]),
    (ConnPickerAction::AddBookmark, &[KeyCode::Char('a')]),
    (ConnPickerAction::EditBookmark, &[KeyCode::Char('e')]),
    (ConnPickerAction::DeleteBookmark, &[KeyCode::Char('x'), KeyCode::Delete]),
    (ConnPickerAction::RenameProfile, &[KeyCode::Char('r')]),
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
        "rename_profile" => RenameProfile,
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
    (BucketPickerAction::Select, &[KeyCode::Enter, KeyCode::Char('l'), KeyCode::Right]),
    (BucketPickerAction::ToggleTheme, &[KeyCode::Char('t')]),
    (BucketPickerAction::Back, &[KeyCode::Char('q'), KeyCode::Esc, KeyCode::Char('h'), KeyCode::Left]),
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
    FocusJobs,
    SortName,
    SortSize,
    SortModified,
    OpenWebUrl,
    OpenEditor,
    RevealInFinder,
    Up,
    Down,
    GoUp,
    EnterSelected,
    ToggleMark,
    MarkRange,
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

impl BrowserAction {
    /// Whether pressing this should light the footer up (see `App::flash_key`). Actions do,
    /// getting around doesn't: moving the cursor, changing directory and switching pane focus
    /// are all already visible on screen from the thing that moved, and they fire often enough
    /// that echoing them would leave the footer permanently lit and stop meaning anything.
    pub fn flashes(self) -> bool {
        use BrowserAction::*;
        !matches!(
            self,
            Up | Down
                | GoUp
                | EnterSelected
                | FocusNext
                | FocusPrev
                | FocusLocal
                | FocusRemote
                | FocusPreview
                | FocusJobs
        )
    }

    /// Short human name for the action, shown in the footer's key-press flash. Kept to one or
    /// two words so the chip stays narrow enough to sit on the end of the keybind bar.
    pub fn label(self) -> &'static str {
        use BrowserAction::*;
        match self {
            Quit => "quit",
            SwitchConnection => "connections",
            Help => "help",
            Events => "events",
            ToggleTheme => "theme",
            PreviewTab => "preview",
            InfoTab => "info",
            ToggleLocal => "local",
            FocusNext | FocusPrev => "focus",
            FocusLocal => "focus local",
            FocusRemote => "focus remote",
            FocusPreview => "focus preview",
            FocusJobs => "focus jobs",
            SortName => "sort name",
            SortSize => "sort size",
            SortModified => "sort modified",
            OpenWebUrl => "open in browser",
            OpenEditor => "open in editor",
            RevealInFinder => "reveal",
            Up => "up",
            Down => "down",
            GoUp => "parent",
            EnterSelected => "open",
            ToggleMark => "mark",
            MarkRange => "mark range",
            ToggleVisual => "visual",
            Download => "download",
            Upload => "upload",
            OpenSync => "sync",
            Delete => "delete",
            StageCopy => "copy",
            StageCut => "cut",
            Paste => "paste",
            CopyLocation => "copy path",
            ShareUrl => "share url",
            Rename => "rename",
            Filter => "filter",
            Cancel => "cancel",
        }
    }
}

/// Renders a pressed key the way the help dialog writes it, for the footer flash.
pub fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "shift+tab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "del".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

const BROWSER_DEFAULTS: &[(BrowserAction, &[KeyCode])] = &[
    (BrowserAction::Quit, &[KeyCode::Char('q')]),
    (BrowserAction::SwitchConnection, &[KeyCode::Char('b')]),
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
    (BrowserAction::FocusJobs, &[KeyCode::Char('4')]),
    (BrowserAction::SortName, &[KeyCode::F(1)]),
    (BrowserAction::SortSize, &[KeyCode::F(2)]),
    (BrowserAction::SortModified, &[KeyCode::F(3)]),
    (BrowserAction::OpenWebUrl, &[KeyCode::Char('o')]),
    (BrowserAction::OpenEditor, &[KeyCode::Char('O')]),
    (BrowserAction::RevealInFinder, &[KeyCode::Char('f')]),
    (BrowserAction::Up, &[KeyCode::Up, KeyCode::Char('k')]),
    (BrowserAction::Down, &[KeyCode::Down, KeyCode::Char('j')]),
    (BrowserAction::GoUp, &[KeyCode::Left, KeyCode::Char('h'), KeyCode::Backspace]),
    (BrowserAction::EnterSelected, &[KeyCode::Right, KeyCode::Char('l'), KeyCode::Enter]),
    (BrowserAction::ToggleMark, &[KeyCode::Char(' ')]),
    (BrowserAction::MarkRange, &[KeyCode::Char('V')]),
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
        // `focus_transfers` is the pane's old name, still accepted so an existing config keeps working.
        "focus_jobs" | "focus_transfers" => FocusJobs,
        "sort_name" => SortName,
        "sort_size" => SortSize,
        "sort_modified" => SortModified,
        "open_web_url" => OpenWebUrl,
        "open_editor" => OpenEditor,
        "reveal_in_finder" => RevealInFinder,
        "up" => Up,
        "down" => Down,
        "go_up" => GoUp,
        "enter_selected" => EnterSelected,
        "toggle_mark" => ToggleMark,
        "mark_range" => MarkRange,
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

/// Actions on the row under the cursor in the jobs pane. Consulted before [`BrowserAction`]
/// while that pane holds focus, so these keys can be short verbs (`c`, `r`, `d`) without having
/// to be globally unique: `d` only means dismiss where a job row is selected, and goes back to
/// meaning download everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobAction {
    Cancel,
    Retry,
    Dismiss,
    DismissFinished,
    Locate,
}

impl JobAction {
    /// Short human name for the footer's key-press flash, matching [`BrowserAction::label`].
    pub fn label(self) -> &'static str {
        use JobAction::*;
        match self {
            Cancel => "cancel job",
            Retry => "retry job",
            Dismiss => "dismiss job",
            DismissFinished => "clear finished",
            Locate => "locate",
        }
    }
}

const JOB_DEFAULTS: &[(JobAction, &[KeyCode])] = &[
    (JobAction::Cancel, &[KeyCode::Char('c')]),
    (JobAction::Retry, &[KeyCode::Char('r')]),
    (JobAction::Dismiss, &[KeyCode::Char('d'), KeyCode::Delete]),
    (JobAction::DismissFinished, &[KeyCode::Char('C')]),
    (JobAction::Locate, &[KeyCode::Char('L')]),
];

fn job_action_by_name(name: &str) -> Option<JobAction> {
    use JobAction::*;
    Some(match name {
        "cancel" => Cancel,
        "retry" => Retry,
        "dismiss" => Dismiss,
        "dismiss_finished" => DismissFinished,
        "locate" => Locate,
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
    pub jobs: HashMap<KeyCode, JobAction>,
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
            jobs: build_table(JOB_DEFAULTS, &raw.jobs, job_action_by_name),
            help: build_table(SCROLL_DEFAULTS, &raw.help, scroll_action_by_name),
            events: build_table(SCROLL_DEFAULTS, &raw.events, scroll_action_by_name),
            sync: build_table(SYNC_DEFAULTS, &raw.sync, sync_action_by_name),
            confirm: build_table(CONFIRM_DEFAULTS, &raw.confirm, confirm_action_by_name),
            bookmark_delete: build_table(BOOKMARK_DELETE_DEFAULTS, &raw.bookmark_delete, bookmark_delete_action_by_name),
        }
    }

    /// The key to advertise for `action` in the footer's keybind bar, so a remapped bind shows
    /// the key that actually works rather than the built-in default. `None` if the user has
    /// unbound the action entirely.
    ///
    /// An action can hold several keys (`enter`, `l` and `→` all open), and `browser` is a
    /// `HashMap`, so the candidates are ranked rather than taken in iteration order, otherwise
    /// the glyph would flicker between frames. A printable character wins, being the one that
    /// fits a single-glyph bar entry; ties break on the rendered name.
    pub fn browser_key(&self, action: BrowserAction) -> Option<KeyCode> {
        self.browser
            .iter()
            .filter(|(_, bound)| **bound == action)
            .map(|(code, _)| *code)
            .min_by_key(|code| (!matches!(code, KeyCode::Char(c) if !c.is_whitespace()), key_label(*code)))
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
    fn footer_bar_advertises_the_remapped_key_not_the_default() {
        let mut raw = KeybindsRaw::default();
        raw.browser.insert("toggle_local".to_string(), "w".to_string());
        let binds = Keybinds::load(&raw);
        assert_eq!(binds.browser_key(BrowserAction::ToggleLocal), Some(KeyCode::Char('w')));
        assert_eq!(binds.browser_key(BrowserAction::Quit), Some(KeyCode::Char('q')));
    }

    #[test]
    fn footer_bar_key_is_stable_when_an_action_has_several_binds() {
        // `enter_selected` defaults to right/l/enter. The pick has to be the same on every
        // frame despite the table being a HashMap, and should prefer the printable character.
        let binds = Keybinds::default();
        let first = binds.browser_key(BrowserAction::EnterSelected);
        assert_eq!(first, Some(KeyCode::Char('l')));
        for _ in 0..20 {
            assert_eq!(binds.browser_key(BrowserAction::EnterSelected), first);
        }
    }

    #[test]
    fn footer_bar_drops_an_action_the_user_unbound() {
        let mut raw = KeybindsRaw::default();
        raw.browser.insert("toggle_local".to_string(), "nonsense".to_string()); // parses to nothing
        let binds = Keybinds::load(&raw);
        assert_eq!(binds.browser_key(BrowserAction::ToggleLocal), None);
    }

    #[test]
    fn navigation_does_not_flash_the_footer_but_actions_do() {
        use BrowserAction::*;
        for action in [Up, Down, GoUp, EnterSelected, FocusNext, FocusLocal, FocusJobs] {
            assert!(!action.flashes(), "{:?} is navigation and should not flash", action.label());
        }
        for action in [Download, Upload, Delete, Paste, StageCopy, OpenSync, Filter, ToggleLocal] {
            assert!(action.flashes(), "{:?} is an action and should flash", action.label());
        }
    }

    #[test]
    fn jobs_pane_keys_shadow_their_browser_meanings() {
        // The point of a separate table: these three are all bound globally to something else,
        // and only mean cancel/retry/dismiss where `input::handle_job_key` consults this first.
        let binds = Keybinds::default();
        assert_eq!(binds.jobs.get(&KeyCode::Char('c')), Some(&JobAction::Cancel));
        assert_eq!(binds.jobs.get(&KeyCode::Char('r')), Some(&JobAction::Retry));
        assert_eq!(binds.jobs.get(&KeyCode::Char('d')), Some(&JobAction::Dismiss));

        assert_eq!(binds.browser.get(&KeyCode::Char('r')), Some(&BrowserAction::Rename));
        assert_eq!(binds.browser.get(&KeyCode::Char('d')), Some(&BrowserAction::Download));
    }

    #[test]
    fn switching_bookmarks_is_on_b_leaving_c_free_for_the_jobs_pane() {
        let binds = Keybinds::default();
        assert_eq!(binds.browser.get(&KeyCode::Char('b')), Some(&BrowserAction::SwitchConnection));
        assert_eq!(binds.browser.get(&KeyCode::Char('c')), None);
    }

    #[test]
    fn the_panes_old_config_name_still_binds_the_focus_action() {
        let mut raw = KeybindsRaw::default();
        raw.browser.insert("focus_transfers".to_string(), "9".to_string());
        let binds = Keybinds::load(&raw);
        assert_eq!(binds.browser.get(&KeyCode::Char('9')), Some(&BrowserAction::FocusJobs));
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
