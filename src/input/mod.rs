//! Keyboard and paste event handling, translation layer from crossterm events to [`App`]
//! method calls, kept separate from the render loop and app state.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::{App, ConfirmKind, Focus, Prompt, PromptKind, Screen, DOUBLE_CLICK_WINDOW};
use crate::keys::{
    BookmarkDeleteAction, BucketPickerAction, ConfirmDialogAction, ConnPickerAction, ScrollAction, SyncDialogAction,
};

/// Carries out a confirmed `ConfirmAction`, shared by the `y` shortcut and `enter`-while-Yes.
async fn run_confirmed(app: &mut App, kind: ConfirmKind) -> Result<()> {
    match kind {
        ConfirmKind::Download => app.start_download_selected().await?,
        ConfirmKind::Upload => app.start_upload_selected(),
        ConfirmKind::Rename(name) => app.rename_to(name).await?,
        ConfirmKind::Sync => app.run_sync(),
        ConfirmKind::Delete => app.delete_selected().await?,
        ConfirmKind::Paste => app.run_paste().await?,
    }
    Ok(())
}

pub async fn handle_event(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            // The inline bookmark editor needs the modifier flags (ctrl+r reveals credentials)
            // that the plain KeyCode tables drop, and it captures every character key for text
            // entry, so it's routed here with the full event before anything else.
            if app.bookmark_editor.is_some() {
                handle_bookmark_editor_key(app, &key);
                return Ok(());
            }
            // Vim-style jumps need the modifier flags and the `gg` chord state that the plain
            // KeyCode→action tables can't express, so they're handled first; anything else falls
            // through to the configurable per-screen bindings.
            if !handle_nav_key(app, &key) {
                handle_key(app, key.code).await?;
            }
        }
        Event::Paste(text) => handle_paste(app, text).await?,
        Event::Mouse(mouse) => handle_mouse(app, mouse).await?,
        _ => {}
    }
    Ok(())
}

/// Whether any keyboard-owning modal is open. Mouse input is a no-op then, same gate
/// [`handle_nav_key`] uses. Wiring up clicks on modal buttons (confirm Yes/No, editor Save/
/// Discard) is a natural follow-up but isn't done here yet.
fn modal_active(app: &App) -> bool {
    app.prompt.is_some()
        || app.bookmark_editor.is_some()
        || app.confirm_action.is_some()
        || app.confirm_bookmark_delete.is_some()
        || app.sync.is_some()
        || app.show_help
        || app.show_events
}

fn point_in(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

/// Mouse support for the browser screen: click a breadcrumb segment to jump there, click a row
/// to select it (a second click within [`DOUBLE_CLICK_WINDOW`] opens/enters it, mirroring
/// `enter_selected`), and scroll the pane the cursor is hovering.
async fn handle_mouse(app: &mut App, mouse: MouseEvent) -> Result<()> {
    if modal_active(app) || !matches!(app.screen, Screen::Browser) {
        return Ok(());
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => handle_browser_click(app, mouse.column, mouse.row).await?,
        MouseEventKind::ScrollUp => handle_browser_scroll(app, mouse.column, mouse.row, -2),
        MouseEventKind::ScrollDown => handle_browser_scroll(app, mouse.column, mouse.row, 2),
        _ => {}
    }
    Ok(())
}

async fn handle_browser_click(app: &mut App, col: u16, row: u16) -> Result<()> {
    if row == app.remote_pane_area.y
        && let Some((_, _, prefix)) = app.breadcrumb_hits.iter().find(|(s, e, _)| col >= *s && col < *e).cloned()
    {
        if prefix != app.prefix {
            app.prefix = prefix;
            app.marked.clear();
            app.cursor = 0;
            app.filter = None;
            app.clear_deep_matches();
            app.refresh().await?;
            app.refresh_preview();
        }
        return Ok(());
    }

    if point_in(app.local_pane_area, col, row) {
        click_row(app, Focus::Local, app.local_pane_area, app.local_list_state.offset(), row).await?;
    } else if point_in(app.remote_pane_area, col, row) {
        click_row(app, Focus::Remote, app.remote_pane_area, app.list_state.offset(), row).await?;
    } else if point_in(app.preview_pane_area, col, row) {
        app.focus_pane(Focus::Preview);
    } else if point_in(app.transfers_pane_area, col, row) {
        click_row(app, Focus::Transfers, app.transfers_pane_area, app.jobs_list_state.offset(), row).await?;
    }
    Ok(())
}

/// Focuses `pane`, selects the row under `row` (accounting for the list's scroll `offset`), and
/// on a same-row double-click within the window, opens/enters it via the normal `enter_selected`
/// path, so a click behaves exactly like moving the cursor there with the keyboard and hitting
/// enter, just batched into one gesture on the second click.
async fn click_row(app: &mut App, pane: Focus, area: Rect, offset: usize, row: u16) -> Result<()> {
    app.focus_pane(pane);
    let first_row = area.y + 1; // inside the top border
    if row < first_row {
        return Ok(());
    }
    let index = offset + (row - first_row) as usize;
    let len = match pane {
        Focus::Local => app.visible_local_entries().len(),
        Focus::Remote => app.visible_entries().len(),
        Focus::Transfers => app.jobs.len(),
        Focus::Preview => 0,
    };
    if index >= len {
        return Ok(());
    }

    let now = Instant::now();
    let is_double = app
        .last_row_click
        .as_ref()
        .is_some_and(|(p, i, at)| *p == pane && *i == index && now.duration_since(*at) < DOUBLE_CLICK_WINDOW);
    app.last_row_click = Some((pane, index, now));

    match pane {
        Focus::Local => app.local_cursor = index,
        Focus::Remote => app.cursor = index,
        Focus::Transfers => app.jobs_cursor = index,
        Focus::Preview => {}
    }
    app.refresh_preview();

    if is_double {
        app.last_row_click = None; // a third click on the same row starts a fresh pair, not a triple
        app.enter_selected().await?;
    }
    Ok(())
}

fn handle_browser_scroll(app: &mut App, col: u16, row: u16, delta: i32) {
    if point_in(app.local_pane_area, col, row) {
        app.focus_pane(Focus::Local);
        app.move_cursor(delta);
        app.refresh_preview();
    } else if point_in(app.remote_pane_area, col, row) {
        app.focus_pane(Focus::Remote);
        app.move_cursor(delta);
        app.refresh_preview();
    } else if point_in(app.preview_pane_area, col, row) {
        app.scroll_preview(delta);
    } else if point_in(app.transfers_pane_area, col, row) {
        app.focus_pane(Focus::Transfers);
        app.move_cursor(delta);
    }
}

/// Universal vim navigation shared by every list screen: `gg`/`G` and `Home`/`End` for the edges,
/// `ctrl-d`/`ctrl-u` for half a screen, `ctrl-f`/`ctrl-b`/`PageDown`/`PageUp` for a full one.
/// Returns whether the key was consumed. Deliberately does nothing while a modal (prompt, confirm,
/// sync, help, events) owns the keyboard, so it can never trigger a stray read/write.
fn handle_nav_key(app: &mut App, key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    let modal = app.prompt.is_some()
        || app.bookmark_editor.is_some()
        || app.confirm_action.is_some()
        || app.confirm_bookmark_delete.is_some()
        || app.sync.is_some()
        || app.show_help
        || app.show_events;
    if modal {
        app.pending_g = false;
        return false;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // `gg` chord: a lone `g` arms it, the next `g` jumps to the top. Any other key breaks it.
    if key.code == KeyCode::Char('g') && !ctrl {
        if app.pending_g {
            app.pending_g = false;
            app.nav_to_edge(false);
        } else {
            app.pending_g = true;
        }
        return true;
    }
    app.pending_g = false;

    match key.code {
        KeyCode::Char('d') if ctrl => app.nav_half_page(1),
        KeyCode::Char('u') if ctrl => app.nav_half_page(-1),
        KeyCode::Char('f') if ctrl => app.nav_page(1),
        KeyCode::Char('b') if ctrl => app.nav_page(-1),
        KeyCode::PageDown => app.nav_page(1),
        KeyCode::PageUp => app.nav_page(-1),
        KeyCode::Char('G') => app.nav_to_edge(true),
        KeyCode::Home => app.nav_to_edge(false),
        KeyCode::End => app.nav_to_edge(true),
        _ => return false,
    }
    true
}

async fn handle_paste(app: &mut App, text: String) -> Result<()> {
    if let Some(prompt) = &mut app.prompt {
        prompt.buffer.push_str(text.trim());
        prompt.cursor = prompt.buffer.len();
        return Ok(());
    }

    if matches!(app.screen, Screen::Browser) && !app.show_help {
        let path = unescape_dropped_path(&text);
        if PathBuf::from(&path).exists() {
            app.prompt = Some(Prompt {
                kind: PromptKind::UploadPath,
                cursor: path.len(),
                buffer: path,
                mask: false,
            });
        } else {
            app.set_status(format!("dropped path not found: {path}"), true);
        }
    }
    Ok(())
}

fn unescape_dropped_path(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = if trimmed.len() >= 2
        && ((trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let mut unescaped = String::with_capacity(unquoted.len());
    let mut chars = unquoted.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek() {
                unescaped.push(next);
                chars.next();
                continue;
            }
        unescaped.push(c);
    }

    if let Some(rest) = unescaped.strip_prefix('~')
        && let Ok(home) = std::env::var("HOME") {
            return format!("{home}{rest}");
        }
    unescaped
}

async fn handle_key(app: &mut App, code: KeyCode) -> Result<()> {
    if let Some(prompt) = &mut app.prompt {
        match code {
            KeyCode::Esc => {
                app.prompt = None;
                app.bookmark_wizard = None;
            }
            KeyCode::Enter => {
                let kind = prompt.kind;
                let buffer = prompt.buffer.clone();
                app.prompt = None;
                match kind {
                    PromptKind::Rename => app.request_confirm_rename(buffer),
                    PromptKind::UploadPath => {
                        let path = PathBuf::from(&buffer);
                        if path.exists() {
                            app.start_upload_path(path);
                        } else {
                            app.set_status(format!("path not found: {buffer}"), true);
                        }
                    }
                    PromptKind::Filter => {
                        app.set_filter(if buffer.is_empty() { None } else { Some(buffer) }).await;
                    }
                    PromptKind::BookmarkField => {
                        app.submit_bookmark_field(buffer);
                    }
                    PromptKind::ProfileRename => {
                        app.rename_profile(buffer);
                    }
                }
            }
            KeyCode::Backspace => {
                prompt.buffer.pop();
            }
            // Filter renders inline, not as a modal, so let arrows move through results
            // immediately rather than forcing `enter` first.
            KeyCode::Up if prompt.kind == PromptKind::Filter => {
                app.move_cursor(-1);
                app.refresh_preview();
            }
            KeyCode::Down if prompt.kind == PromptKind::Filter => {
                app.move_cursor(1);
                app.refresh_preview();
            }
            KeyCode::Char(c) => prompt.buffer.push(c),
            _ => {}
        }
        if matches!(app.prompt.as_ref().map(|p| p.kind), Some(PromptKind::Filter)) {
            let live = app.prompt.as_ref().map(|p| p.buffer.clone());
            app.set_filter(live).await;
        }
        return Ok(());
    }

    if app.confirm_bookmark_delete.is_some() {
        match app.keybinds.bookmark_delete.get(&code).copied() {
            Some(BookmarkDeleteAction::Confirm) => app.confirm_delete_bookmark_now(),
            None => app.cancel_delete_bookmark(),
        }
        return Ok(());
    }

    // "Are you sure?" for write actions: `tab`/arrows flip the highlighted Yes/No button,
    // `enter` activates it, `y`/`n`/`esc` still work directly as shortcuts.
    if let Some(action) = &mut app.confirm_action {
        match app.keybinds.confirm.get(&code).copied() {
            Some(ConfirmDialogAction::ToggleSelection) => {
                action.yes_selected = !action.yes_selected;
            }
            Some(ConfirmDialogAction::Yes) => {
                let kind = app.confirm_action.take().unwrap().kind;
                run_confirmed(app, kind).await?;
            }
            Some(ConfirmDialogAction::No) => app.confirm_action = None,
            Some(ConfirmDialogAction::Confirm) if action.yes_selected => {
                let kind = app.confirm_action.take().unwrap().kind;
                run_confirmed(app, kind).await?;
            }
            _ => app.confirm_action = None,
        }
        return Ok(());
    }

    if app.show_help {
        match app.keybinds.help.get(&code).copied() {
            Some(ScrollAction::Up) => app.scroll_help(-1),
            Some(ScrollAction::Down) => app.scroll_help(1),
            None => {
                app.show_help = false;
                app.help_scroll = 0;
            }
        }
        return Ok(());
    }
    if app.show_events {
        match app.keybinds.events.get(&code).copied() {
            Some(ScrollAction::Up) => app.scroll_events(-1),
            Some(ScrollAction::Down) => app.scroll_events(1),
            None => {
                app.show_events = false;
                app.events_scroll = 0;
            }
        }
        return Ok(());
    }

    if app.sync.is_some() {
        match app.keybinds.sync.get(&code).copied() {
            Some(SyncDialogAction::Close) => app.close_sync(),
            Some(SyncDialogAction::Up) => app.move_sync_cursor(-1),
            Some(SyncDialogAction::Down) => app.move_sync_cursor(1),
            Some(SyncDialogAction::FlipDirection) => app.flip_sync_direction().await,
            Some(SyncDialogAction::Confirm) => app.request_confirm_sync(),
            None => {}
        }
        return Ok(());
    }

    match app.screen {
        Screen::ConnectionPicker => handle_connection_picker_key(app, code).await?,
        Screen::BucketPicker => handle_bucket_picker_key(app, code).await?,
        Screen::Browser => handle_browser_key(app, code).await?,
    }

    Ok(())
}

/// Handles a key while the inline bookmark editor is open. Takes the whole event (not just the
/// KeyCode) so `ctrl+r` can toggle credential reveal; every unmodified character key is text input.
fn handle_bookmark_editor_key(app: &mut App, key: &crossterm::event::KeyEvent) {
    use crossterm::event::KeyModifiers;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('r') if ctrl => app.toggle_editor_reveal(),
        KeyCode::Esc => app.cancel_edit_bookmark(),
        KeyCode::Enter => app.editor_activate(),
        KeyCode::Up | KeyCode::BackTab => app.editor_move(-1),
        KeyCode::Down | KeyCode::Tab => app.editor_move(1),
        KeyCode::Left => app.editor_horizontal(-1),
        KeyCode::Right => app.editor_horizontal(1),
        KeyCode::Home => app.editor_cursor_home(),
        KeyCode::End => app.editor_cursor_end(),
        KeyCode::Backspace => app.editor_backspace(),
        KeyCode::Delete => app.editor_delete(),
        // Ignore other ctrl-chords so they don't get typed as literal characters.
        KeyCode::Char(c) if !ctrl => app.editor_insert(c),
        _ => {}
    }
}

async fn handle_connection_picker_key(app: &mut App, code: KeyCode) -> Result<()> {
    let Some(action) = app.keybinds.conn_picker.get(&code).copied() else { return Ok(()) };
    match action {
        ConnPickerAction::Up => {
            if app.conn_selected > 0 {
                app.conn_selected -= 1;
            }
        }
        ConnPickerAction::Down => {
            if app.conn_selected + 1 < app.connections.len() {
                app.conn_selected += 1;
            }
        }
        ConnPickerAction::Select => {
            if !app.connections.is_empty() {
                let index = app.conn_selected;
                app.connect(index).await?;
            }
        }
        ConnPickerAction::AddBookmark => app.start_add_bookmark(),
        ConnPickerAction::EditBookmark => {
            if !app.connections.is_empty() {
                app.start_edit_bookmark(app.conn_selected);
            }
        }
        ConnPickerAction::DeleteBookmark => {
            if !app.connections.is_empty() {
                app.start_delete_bookmark(app.conn_selected);
            }
        }
        ConnPickerAction::RenameProfile => {
            if !app.connections.is_empty() {
                let current = app.selected_profile();
                app.prompt = Some(Prompt {
                    kind: PromptKind::ProfileRename,
                    cursor: current.len(),
                    buffer: current,
                    mask: false,
                });
            }
        }
        ConnPickerAction::ToggleTheme => app.theme = app.theme.toggled(),
        ConnPickerAction::Help => app.show_help = true,
        ConnPickerAction::Quit => app.should_quit = true,
    }
    Ok(())
}

async fn handle_bucket_picker_key(app: &mut App, code: KeyCode) -> Result<()> {
    let Some(action) = app.keybinds.bucket_picker.get(&code).copied() else { return Ok(()) };
    match action {
        BucketPickerAction::Up => {
            if app.bucket_selected > 0 {
                app.bucket_selected -= 1;
            }
        }
        BucketPickerAction::Down => {
            if app.bucket_selected + 1 < app.buckets.len() {
                app.bucket_selected += 1;
            }
        }
        BucketPickerAction::Select => {
            let index = app.bucket_selected;
            app.pick_bucket(index).await?;
        }
        BucketPickerAction::ToggleTheme => app.theme = app.theme.toggled(),
        BucketPickerAction::Back => {
            app.screen = Screen::ConnectionPicker;
            app.client = None;
            app.buckets.clear();
        }
    }
    Ok(())
}

async fn handle_browser_key(app: &mut App, code: KeyCode) -> Result<()> {
    use crate::keys::BrowserAction as A;
    let Some(action) = app.keybinds.browser.get(&code).copied() else { return Ok(()) };
    match action {
        A::Quit => app.should_quit = true,
        A::SwitchConnection => {
            app.screen = Screen::ConnectionPicker;
            app.client = None;
            app.buckets.clear();
        }
        A::Help => app.show_help = true,
        A::Events => app.show_events = true,
        A::ToggleTheme => app.theme = app.theme.toggled(),
        A::PreviewTab => app.select_preview_tab(),
        A::InfoTab => app.select_info_tab(),
        A::ToggleLocal => app.toggle_local(),
        A::FocusNext => app.toggle_focus(),
        A::FocusPrev => app.toggle_focus_back(),
        A::FocusLocal => app.focus_pane(Focus::Local),
        A::FocusRemote => app.focus_pane(Focus::Remote),
        A::FocusPreview => app.focus_pane(Focus::Preview),
        A::FocusTransfers => app.focus_pane(Focus::Transfers),
        A::SortName => app.cycle_sort(crate::app::SortKey::Name),
        A::SortSize => app.cycle_sort(crate::app::SortKey::Size),
        A::SortModified => app.cycle_sort(crate::app::SortKey::Modified),
        A::OpenWebUrl => match app.connection.as_ref().and_then(|c| c.web_url.clone()) {
            Some(url) => {
                if let Err(err) = open::that(&url) {
                    app.set_status(format!("failed to open {url}: {err}"), true);
                }
            }
            None => app.set_status("this bookmark has no web_url set", true),
        },
        A::OpenEditor if matches!(app.focus, Focus::Local | Focus::Remote) => app.request_open_editor(),
        A::RevealInFinder if app.focus == Focus::Transfers => app.reveal_selected_job_in_finder(),
        A::Up => {
            if app.focus == Focus::Preview {
                app.scroll_preview(-1);
            } else {
                app.move_cursor(-1);
                app.refresh_preview();
            }
        }
        A::Down => {
            if app.focus == Focus::Preview {
                app.scroll_preview(1);
            } else {
                app.move_cursor(1);
                app.refresh_preview();
            }
        }
        A::GoUp => app.go_up().await?,
        A::EnterSelected => app.enter_selected().await?,
        A::ToggleMark => app.toggle_mark(),
        A::ToggleVisual => app.toggle_visual_mode(),
        // Download only makes sense from the remote pane; upload only from the local pane.
        // Both go through an "are you sure?" confirmation first.
        A::Download if app.focus == Focus::Remote => app.request_confirm_download(),
        A::Upload if app.focus == Focus::Local => app.request_confirm_upload(),
        A::OpenSync => app.open_sync().await,
        A::Delete => app.request_confirm_delete(),
        A::StageCopy => app.stage_copy(),
        A::StageCut => app.stage_cut(),
        A::Paste => app.request_confirm_paste(),
        A::CopyLocation => app.copy_location_to_clipboard(),
        A::ShareUrl => app.generate_share_url().await,
        A::Rename => {
            let name = match app.focus {
                Focus::Remote => app.current_entry().map(|e| e.name.clone()),
                Focus::Local => app.current_local_entry().map(|e| e.name.clone()),
                Focus::Preview | Focus::Transfers => None,
            };
            if let Some(name) = name {
                app.prompt = Some(Prompt {
                    kind: PromptKind::Rename,
                    cursor: name.len(),
                    buffer: name,
                    mask: false,
                });
            }
        }
        A::Filter => {
            let existing = app.active_filter().unwrap_or_default();
            app.prompt =
                Some(Prompt { cursor: existing.len(), buffer: existing, kind: PromptKind::Filter, mask: false });
        }
        A::Cancel => {
            if app.cancel_running_jobs() {
                app.set_status("cancelling...", false);
            } else if app.active_filter().is_some() {
                app.set_filter(None).await;
            } else {
                app.marked.clear();
                app.local_marked.clear();
                app.visual_anchor = None;
                app.clear_clip();
            }
        }
        _ => {}
    }
    Ok(())
}
