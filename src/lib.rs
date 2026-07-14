//! comhad's library crate. `main.rs` is a two-line shell around [`run_app`]; everything else
//! lives here so it's reachable from integration tests as well as co-located unit tests.

pub mod config;
pub mod keys;

mod app;
mod fuzzy;
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

/// comhad's entire runtime: load config, take over the terminal, run the event loop, hand it back.
pub async fn run_app() -> Result<()> {
    let connections = config::load_connections()?;
    let app_config = config::load_app_config()?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, connections, app_config).await;
    restore_terminal(&mut terminal)?;
    result
}

/// `TERM_PROGRAM` doesn't survive through tmux, but `ITERM_SESSION_ID` (also set by iTerm2)
/// does — same signal `ratatui_image`'s own tmux detection uses.
fn is_iterm2_via_env() -> bool {
    std::env::var("ITERM_SESSION_ID").is_ok_and(|v| !v.is_empty())
        || std::env::var("TERM_PROGRAM").is_ok_and(|v| v.contains("iTerm"))
        || std::env::var("LC_TERMINAL").is_ok_and(|v| v.contains("iTerm"))
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

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    connections: Vec<(String, config::Connection)>,
    app_config: config::AppConfig,
) -> Result<()> {
    // Must run after entering the alternate screen but before reading terminal events — queries
    // the terminal via escape sequences for graphics protocol support and font size, falling
    // back to halfblocks if unsupported.
    let mut picker = ratatui_image::picker::Picker::from_query_stdio().unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());

    // tmux passthrough can answer the Sixel capability query even when the outer terminal
    // (e.g. iTerm2) renders it as garbled raw data — trust ITERM_SESSION_ID over Sixel when set.
    if picker.protocol_type() == ratatui_image::picker::ProtocolType::Sixel && is_iterm2_via_env() {
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Iterm2);
    }

    let mut app = App::new(connections, picker, app_config);
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
