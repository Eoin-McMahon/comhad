//! Keyboard and paste event handling, translation layer from crossterm events to [`App`]
//! method calls, kept separate from the render loop and app state.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};

use crate::app::{App, ConfirmKind, Focus, Prompt, PromptKind, Screen};
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
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(app, key.code).await?,
        Event::Paste(text) => handle_paste(app, text).await?,
        _ => {}
    }
    Ok(())
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
