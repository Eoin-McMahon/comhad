//! comhad's library crate. `main.rs` is a two-line shell around [`run_app`]; everything else
//! lives here so it's reachable from integration tests as well as co-located unit tests.

pub mod config;
pub use config::keys;

mod app;
mod fuzzy;
mod input;
mod jobs;
mod local;
mod provider;
mod ui;

use std::io::Stdout;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, EventStream};
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
    let mouse = app_config.ui.mouse;

    let mut terminal = setup_terminal(mouse)?;
    let result = run(&mut terminal, connections, app_config).await;
    restore_terminal(&mut terminal, mouse)?;
    result
}

/// `TERM_PROGRAM` doesn't survive through tmux, but `ITERM_SESSION_ID` (also set by iTerm2)
/// does, same signal `ratatui_image`'s own tmux detection uses.
fn is_iterm2_via_env() -> bool {
    std::env::var("ITERM_SESSION_ID").is_ok_and(|v| !v.is_empty())
        || std::env::var("TERM_PROGRAM").is_ok_and(|v| v.contains("iTerm"))
        || std::env::var("LC_TERMINAL").is_ok_and(|v| v.contains("iTerm"))
}

fn setup_terminal(mouse: bool) -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse {
        // Capturing the mouse takes over the terminal's native click-drag text selection, so
        // it's opt-out via `[ui] mouse` rather than unconditional.
        execute!(stdout, EnableMouseCapture)?;
    }
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mouse: bool) -> Result<()> {
    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    execute!(terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Handles `O`: suspends the TUI (same leave/re-enter as `setup_terminal`/`restore_terminal`,
/// just around a single child process instead of the whole app), runs `$EDITOR`/`$VISUAL` (or
/// `vi`) on the target (downloaded to a temp file first for a remote object), and on a changed
/// remote temp file, uploads it back as a real transfer job (visible in the transfers strip and
/// events log, same as any other upload). Follows the `git commit`/`crontab -e` convention: just
/// spawn-and-wait, no per-editor special-casing. A GUI editor (VS Code, Sublime) returns
/// immediately unless configured with its own `--wait`/`-w` flag, in which case this simply sees
/// no change and uploads nothing. Never silently wrong, just a no-op until configured.
async fn run_pending_editor(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let Some(target) = app.pending_editor.take() else { return Ok(()) };

    let (local_path, remote) = match target {
        app::EditorTarget::Local(path) => (path, None),
        app::EditorTarget::Remote { bucket, key, name } => {
            let Some(client) = app.client.clone() else { return Ok(()) };
            // A dedicated per-process subdirectory (rather than a prefixed filename) keeps the
            // temp file's own name clean, so it shows up in the upload job/events log as
            // "report.txt", not some mangled "comhad-1234-report.txt".
            let dir = std::env::temp_dir().join(format!("comhad-{}", std::process::id()));
            if let Err(err) = std::fs::create_dir_all(&dir) {
                app.set_status(format!("failed to create temp dir for editing: {err}"), true);
                return Ok(());
            }
            let tmp = dir.join(&name);
            if let Err(err) = client.download(&bucket, &key, &tmp, &|_| {}).await {
                app.set_status(format!("failed to download {key} for editing: {err}"), true);
                return Ok(());
            }
            (tmp, Some((client, bucket, key, name)))
        }
    };

    let before = std::fs::metadata(&local_path).and_then(|m| m.modified()).ok();

    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".to_string());
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        app.set_status("$EDITOR/$VISUAL is set but empty", true);
        return Ok(());
    };
    let extra_args: Vec<&str> = parts.collect();

    disable_raw_mode()?;
    if app.mouse_enabled {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let status = tokio::process::Command::new(program).args(&extra_args).arg(&local_path).status().await;

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    if app.mouse_enabled {
        execute!(terminal.backend_mut(), EnableMouseCapture)?;
    }
    terminal.clear()?; // force a full repaint; the alternate-screen round trip left it stale

    let status = match status {
        Ok(status) => status,
        Err(err) => {
            if let Some((_, _, key, name)) = remote {
                let _ = std::fs::remove_file(&local_path);
                let id = app.next_id();
                app.jobs.push(jobs::Job {
                    id,
                    label: name,
                    kind: jobs::JobKind::Upload,
                    total_bytes: 0,
                    done_bytes: 0,
                    status: jobs::JobStatus::Failed(err.to_string()),
                    cancel: Arc::new(AtomicBool::new(false)),
                    local_path: PathBuf::new(),
                    remote_key: Some(key),
                    // A row recording an editor that never launched, with no task behind it.
                    spec: None,
                });
            }
            app.set_status(format!("failed to launch {program}: {err}"), true);
            return Ok(());
        }
    };

    let changed = std::fs::metadata(&local_path).and_then(|m| m.modified()).ok() != before;

    match remote {
        // The client came from `EditorTarget::Remote`; `App::start_job` reaches for the live one
        // on `app` itself, so it isn't needed again here.
        Some((_client, bucket, key, name)) => {
            if changed {
                // A real transfer job (not just a status line): pushed synchronously so it's
                // visible in the transfers strip immediately, then handed to the same
                // `spawn_upload_file` + `job_tx` path as any other upload. `drain_job_messages`
                // logs the started/finished events and refreshes the listing/preview on `Done`.
                // `pending_deletes` cleans up the temp file once the upload actually succeeds
                // (left in place on failure, so a failed upload never loses the edit).
                let id = app.start_job(
                    name,
                    jobs::JobKind::Upload,
                    local_path.clone(),
                    Some(key.clone()),
                    jobs::JobSpec::UploadFile {
                        bucket: bucket.clone(),
                        local_path: local_path.clone(),
                        key: key.clone(),
                    },
                );
                if let Some(id) = id {
                    app.pending_deletes.insert(id, app::PendingDelete::Local(local_path));
                    app.pending_write_marks.insert(id, (bucket, key));
                }
            } else {
                let _ = std::fs::remove_file(&local_path);
                // Non-zero exit with nothing changed reads as an aborted edit (`:cq`, ctrl-c,
                // ...). A normal `:q` with no edits also lands here, but exits 0, so it stays
                // silent instead of spamming a "cancelled" line on every quiet look at a file.
                if !status.success() {
                    let id = app.next_id();
                    app.jobs.push(jobs::Job {
                        id,
                        label: name.clone(),
                        kind: jobs::JobKind::Upload,
                        total_bytes: 0,
                        done_bytes: 0,
                        status: jobs::JobStatus::Cancelled,
                        cancel: Arc::new(AtomicBool::new(false)),
                        local_path: PathBuf::new(),
                        remote_key: Some(key.clone()),
                        // Records an abandoned edit; there was never an upload to retry.
                        spec: None,
                    });
                    app.set_status(format!("edit cancelled: {name}"), false);
                }
            }
        }
        None => {
            if !status.success() {
                app.set_status(format!("{program} exited with {status}"), true);
            }
            if changed {
                app.needs_local_refresh = true;
                app.recent_write = Some((app::RecentWrite::Local(local_path), std::time::Instant::now()));
            }
            app.refresh_preview();
        }
    }
    Ok(())
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    connections: Vec<(String, config::Connection)>,
    app_config: config::AppConfig,
) -> Result<()> {
    // Must run after entering the alternate screen but before reading terminal events, queries
    // the terminal via escape sequences for graphics protocol support and font size, falling
    // back to halfblocks if unsupported.
    let mut picker = ratatui_image::picker::Picker::from_query_stdio().unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());

    // Auto-detection can false-positive on iTerm2: tmux passthrough answers the Sixel capability
    // query even when the outer terminal renders it as garbled raw data, and iTerm2's Kitty
    // protocol support is unreliable enough (version-dependent, and the capability query itself
    // can respond "yes" without actually rendering) that a detected Kitty/Sixel result there is
    // more often wrong than right. iTerm2's own inline-image protocol is the one thing it's
    // always supported natively, so once we know we're in iTerm2 (env vars, not escape-sequence
    // probing, which is reliable), that wins over whatever the graphics probe guessed.
    if is_iterm2_via_env()
        && matches!(
            picker.protocol_type(),
            ratatui_image::picker::ProtocolType::Sixel | ratatui_image::picker::ProtocolType::Kitty
        )
    {
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Iterm2);
    }

    // Surfaced to the events log (`E`) purely as a debugging aid: if images never render, this
    // says whether the terminal was detected as supporting a real graphics protocol at all, or
    // fell back to halfblocks (should still show something, colored block art, so if even that
    // renders nothing, the bug is elsewhere).
    let protocol = format!("{:?}", picker.protocol_type());
    let mut app = App::new(connections, picker, app_config);
    app.set_status(format!("image protocol detected: {protocol}"), false);
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
        if app.pending_editor.is_some() {
            // Draw first so the "opening/downloading..." status set by `request_open_editor` is
            // actually visible before the (possibly slow, for a large remote file) download runs.
            // Otherwise it'd be set and then immediately blown past with nothing ever painted.
            terminal.draw(|f| ui::draw(f, &mut app))?;
            run_pending_editor(terminal, &mut app).await?;
        }
        terminal.draw(|f| ui::draw(f, &mut app))?;

        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            _ = tick.tick() => {
                app.spinner_frame = app.spinner_frame.wrapping_add(1);
                app.sample_throughput();
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
