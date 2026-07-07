mod app;
mod config;
mod jobs;
mod local;
mod s3;
mod ui;

use std::io::Stdout;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, Focus, Prompt, PromptKind, Screen};

#[tokio::main]
async fn main() -> Result<()> {
    let connections = config::load_connections()?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, connections).await;
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

async fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, connections: Vec<(String, config::Connection)>) -> Result<()> {
    let mut app = App::new(connections);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));

    loop {
        app.drain_job_messages();
        app.drain_preview_messages();
        if app.needs_local_refresh {
            app.refresh_local();
            app.needs_local_refresh = false;
        }
        if app.needs_remote_refresh {
            app.refresh().await?;
            app.needs_remote_refresh = false;
        }
        terminal.draw(|f| ui::draw(f, &mut app))?;

        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            _ = tick.tick() => {
                app.spinner_frame = app.spinner_frame.wrapping_add(1);
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => handle_event(&mut app, event).await?,
                    Some(Err(_)) | None => return Ok(()),
                }
            }
        }
    }
}

async fn handle_event(app: &mut App, event: Event) -> Result<()> {
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
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                unescaped.push(next);
                chars.next();
                continue;
            }
        }
        unescaped.push(c);
    }

    if let Some(rest) = unescaped.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}{rest}");
        }
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
                    PromptKind::Rename => {
                        if !buffer.is_empty() {
                            app.rename_to(buffer).await?;
                        }
                    }
                    PromptKind::UploadPath => {
                        let path = PathBuf::from(&buffer);
                        if path.exists() {
                            app.start_upload_path(path);
                        } else {
                            app.set_status(format!("path not found: {buffer}"), true);
                        }
                    }
                    PromptKind::Filter => {
                        app.filter = if buffer.is_empty() { None } else { Some(buffer) };
                    }
                    PromptKind::BookmarkField => {
                        app.submit_bookmark_field(buffer);
                    }
                }
            }
            KeyCode::Backspace => {
                prompt.buffer.pop();
            }
            KeyCode::Char(c) => prompt.buffer.push(c),
            _ => {}
        }
        if matches!(app.prompt.as_ref().map(|p| p.kind), Some(PromptKind::Filter)) {
            app.filter = app.prompt.as_ref().map(|p| p.buffer.clone());
        }
        return Ok(());
    }

    if let Some(path) = app.confirm_bookmark_delete.clone() {
        match code {
            KeyCode::Char('y') | KeyCode::Enter => app.confirm_delete_bookmark_now(),
            _ => app.cancel_delete_bookmark(),
        }
        let _ = path;
        return Ok(());
    }

    if app.show_help {
        app.show_help = false;
        return Ok(());
    }
    if app.show_diagnostics {
        app.show_diagnostics = false;
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
        KeyCode::Char('E') => {
            if app.last_error.is_some() {
                app.show_diagnostics = true;
            }
        }
        KeyCode::Char('t') => app.theme = app.theme.toggled(),
        KeyCode::Char('p') => app.toggle_preview(),
        KeyCode::Char('L') => app.toggle_local(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::BackTab => app.toggle_focus_back(),
        KeyCode::Char('1') => app.focus_pane(Focus::Local),
        KeyCode::Char('2') => app.focus_pane(Focus::Remote),
        KeyCode::Char('3') => app.focus_pane(Focus::Preview),
        KeyCode::Char('4') => app.focus_pane(Focus::Transfers),
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
        KeyCode::Char('d') => app.start_download_selected().await?,
        KeyCode::Char('u') => app.start_upload_selected(),
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
            let existing = app.filter.clone().unwrap_or_default();
            app.prompt =
                Some(Prompt { cursor: existing.len(), buffer: existing, kind: PromptKind::Filter, mask: false });
        }
        KeyCode::Esc => {
            if app.filter.is_some() {
                app.filter = None;
            } else {
                app.marked.clear();
                app.local_marked.clear();
            }
        }
        _ => {}
    }
    Ok(())
}
