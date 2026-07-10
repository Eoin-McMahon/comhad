//! Keyboard and paste event handling — the translation layer from crossterm events to [`App`]
//! method calls. Kept separate from `main` (the render loop) and `app` (the state) so the
//! key-binding surface lives in one place.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};

use crate::app::{App, ConfirmKind, Focus, Prompt, PromptKind, Screen};

/// Carries out whatever a confirmed `ConfirmAction` was for — shared by the `y` shortcut and
/// `enter`-while-Yes-is-selected.
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
            // Filter's own results are already visible in the pane behind it (it renders
            // inline, not as a modal), so let arrows move through them immediately rather
            // than forcing `enter` first just to dismiss the text input.
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
        match code {
            KeyCode::Char('y') | KeyCode::Enter => app.confirm_delete_bookmark_now(),
            _ => app.cancel_delete_bookmark(),
        }
        return Ok(());
    }

    // "Are you sure?" for write actions (download / upload / rename / sync / delete / paste).
    // Two tabbed buttons (Yes/No) rather than just accepting `y`/`enter` unconditionally —
    // `tab`/arrows flip which one's highlighted, `enter` activates it, `y`/`n`/`esc` still
    // work directly as shortcuts.
    if let Some(action) = &mut app.confirm_action {
        match code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                action.yes_selected = !action.yes_selected;
            }
            KeyCode::Char('y') => {
                let kind = app.confirm_action.take().unwrap().kind;
                run_confirmed(app, kind).await?;
            }
            KeyCode::Char('n') | KeyCode::Esc => app.confirm_action = None,
            KeyCode::Enter if action.yes_selected => {
                let kind = app.confirm_action.take().unwrap().kind;
                run_confirmed(app, kind).await?;
            }
            _ => app.confirm_action = None,
        }
        return Ok(());
    }

    if app.show_help {
        match code {
            KeyCode::Up | KeyCode::Char('k') => app.scroll_help(-1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_help(1),
            _ => {
                app.show_help = false;
                app.help_scroll = 0;
            }
        }
        return Ok(());
    }
    if app.show_events {
        match code {
            KeyCode::Up | KeyCode::Char('k') => app.scroll_events(-1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_events(1),
            _ => {
                app.show_events = false;
                app.events_scroll = 0;
            }
        }
        return Ok(());
    }

    if app.sync.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_sync(),
            KeyCode::Up | KeyCode::Char('k') => app.move_sync_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => app.move_sync_cursor(1),
            // Flip the sync direction (local→remote ⇄ remote→local) and rescan.
            KeyCode::Tab | KeyCode::Char('d') => app.flip_sync_direction().await,
            KeyCode::Enter => app.request_confirm_sync(),
            _ => {}
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
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.conn_selected > 0 {
                app.conn_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.conn_selected + 1 < app.connections.len() {
                app.conn_selected += 1;
            }
        }
        KeyCode::Enter => {
            if !app.connections.is_empty() {
                let index = app.conn_selected;
                app.connect(index).await?;
            }
        }
        KeyCode::Char('a') => app.start_add_bookmark(),
        KeyCode::Char('e') => {
            if !app.connections.is_empty() {
                app.start_edit_bookmark(app.conn_selected);
            }
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            if !app.connections.is_empty() {
                app.start_delete_bookmark(app.conn_selected);
            }
        }
        KeyCode::Char('t') => app.theme = app.theme.toggled(),
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        _ => {}
    }
    Ok(())
}

async fn handle_bucket_picker_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.bucket_selected > 0 {
                app.bucket_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.bucket_selected + 1 < app.buckets.len() {
                app.bucket_selected += 1;
            }
        }
        KeyCode::Enter => {
            let index = app.bucket_selected;
            app.pick_bucket(index).await?;
        }
        KeyCode::Char('t') => app.theme = app.theme.toggled(),
        KeyCode::Char('q') | KeyCode::Esc => {
            app.screen = Screen::ConnectionPicker;
            app.client = None;
            app.buckets.clear();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_browser_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('c') => {
            app.screen = Screen::ConnectionPicker;
            app.client = None;
            app.buckets.clear();
        }
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char('E') => app.show_events = true,
        KeyCode::Char('t') => app.theme = app.theme.toggled(),
        KeyCode::Char('p') => app.select_preview_tab(),
        KeyCode::Char('i') => app.select_info_tab(),
        KeyCode::Char('L') => app.toggle_local(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::BackTab => app.toggle_focus_back(),
        KeyCode::Char('1') => app.focus_pane(Focus::Local),
        KeyCode::Char('2') => app.focus_pane(Focus::Remote),
        KeyCode::Char('3') => app.focus_pane(Focus::Preview),
        KeyCode::Char('4') => app.focus_pane(Focus::Transfers),
        KeyCode::F(1) => app.cycle_sort(crate::app::SortKey::Name),
        KeyCode::F(2) => app.cycle_sort(crate::app::SortKey::Size),
        KeyCode::F(3) => app.cycle_sort(crate::app::SortKey::Modified),
        KeyCode::Char('o') => match app.connection.as_ref().and_then(|c| c.web_url.clone()) {
            Some(url) => {
                if let Err(err) = open::that(&url) {
                    app.set_status(format!("failed to open {url}: {err}"), true);
                }
            }
            None => app.set_status("this bookmark has no web_url set", true),
        },
        KeyCode::Char('f') if app.focus == Focus::Transfers => app.reveal_selected_job_in_finder(),
        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus == Focus::Preview {
                app.scroll_preview(-1);
            } else {
                app.move_cursor(-1);
                app.refresh_preview();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus == Focus::Preview {
                app.scroll_preview(1);
            } else {
                app.move_cursor(1);
                app.refresh_preview();
            }
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => app.go_up().await?,
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => app.enter_selected().await?,
        KeyCode::Char(' ') => app.toggle_mark(),
        // Download only makes sense from the remote pane; upload only from the local pane.
        // Both go through an "are you sure?" confirmation first.
        KeyCode::Char('d') if app.focus == Focus::Remote => app.request_confirm_download(),
        KeyCode::Char('u') if app.focus == Focus::Local => app.request_confirm_upload(),
        KeyCode::Char('s') => app.open_sync().await,
        KeyCode::Char('D') => app.request_confirm_delete(),
        KeyCode::Char('y') => app.stage_copy(),
        KeyCode::Char('x') => app.stage_cut(),
        KeyCode::Char('P') => app.request_confirm_paste(),
        KeyCode::Char('Y') => app.copy_location_to_clipboard(),
        KeyCode::Char('r') => {
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
        KeyCode::Char('/') => {
            let existing = app.active_filter().unwrap_or_default();
            app.prompt =
                Some(Prompt { cursor: existing.len(), buffer: existing, kind: PromptKind::Filter, mask: false });
        }
        KeyCode::Esc => {
            if app.active_filter().is_some() {
                app.set_filter(None).await;
            } else {
                app.marked.clear();
                app.local_marked.clear();
                app.clear_clip();
            }
        }
        _ => {}
    }
    Ok(())
}
