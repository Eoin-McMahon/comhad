mod app;
mod config;
mod input;
mod jobs;
mod local;
mod provider;
mod ui;

use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, EventStream};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;

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
    // Must run after entering the alternate screen but before reading any terminal events —
    // it queries the terminal directly via escape sequences to detect graphics protocol
    // support (kitty/iTerm2/sixel) and font size, falling back to halfblocks if that fails
    // or the terminal doesn't support any of them.
    let picker = ratatui_image::picker::Picker::from_query_stdio().unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
    let mut app = App::new(connections, picker);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));

    loop {
        app.drain_job_messages();
        app.drain_preview_messages();
        app.drain_deep_scan_messages();
        app.expire_status();
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
                    Some(Ok(event)) => input::handle_event(&mut app, event).await?,
                    Some(Err(_)) | None => return Ok(()),
                }
            }
        }
    }
}
