//! Application state and behavior.
//!
//! [`App`] is the single coordinator that the render loop (`main`) and input layer (`input`)
//! drive. Its behavior is split across sibling modules by concern, [`browser`] (navigation +
//! transfers), [`bookmarks`] (the add/edit/delete wizard), and [`preview`] (the preview pane), 
//! each of which adds `impl App` blocks. This module holds the struct itself plus the small,
//! cross-cutting core: construction, status/error plumbing, focus, and job-progress draining.

mod bookmarks;
mod browser;
mod clipboard;
mod confirm;
mod deep;
mod preview;
mod sort;
mod sync;

pub use bookmarks::{BookmarkWizard, BOOKMARK_FIELDS};
pub use clipboard::{Clipboard, ClipMode, PendingDelete};
pub use confirm::{ConfirmAction, ConfirmKind};
pub use deep::{DeepLocalMatches, DeepRemoteMatches};
pub use preview::{InfoDetails, Preview, PreviewMode};
pub use sort::{Sort, SortDir, SortKey};
pub use sync::{SyncAction, SyncDirection, SyncState};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::config::Connection;
use crate::jobs::{Job, JobId, JobKind, JobStatus, ProgressMsg};
use crate::local::{self, LocalEntry};
use crate::provider::{RemoteEntry, StorageProvider};

/// A single logged event, every `set_status`/`set_error` call becomes one of these, not
/// just the current footer toast. Kept so the `E` events log can show a real history
/// ("uploaded x", "transfer hasn't finished yet", a failed list, ...) instead of only ever
/// the single most recent error.
pub struct Event {
    pub message: String,
    pub is_error: bool,
    pub at: Instant,
    /// Full multi-line detail (error chain + connection diagnostics), for errors that have
    /// more to say than the one-line message. `None` for plain status events.
    pub detail: Option<String>,
}

/// Cap on how many events are kept, a long session shouldn't grow this unboundedly.
const MAX_EVENTS: usize = 200;

pub enum Screen {
    ConnectionPicker,
    BucketPicker,
    Browser,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Local,
    Remote,
    Preview,
    Transfers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Rename,
    UploadPath,
    Filter,
    BookmarkField,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub buffer: String,
    pub cursor: usize,
    /// Mask typed characters as `•`, used for the secret_access_key wizard field.
    pub mask: bool,
}

pub struct App {
    pub screen: Screen,
    pub connections: Vec<(String, Connection)>,
    pub conn_selected: usize,

    pub client: Option<Arc<dyn StorageProvider>>,
    pub connection: Option<Connection>,
    pub bucket: String,
    pub buckets: Vec<String>,
    pub bucket_selected: usize,

    // Remote (S3) pane.
    pub prefix: String,
    pub entries: Vec<RemoteEntry>,
    pub cursor: usize,
    pub marked: HashSet<String>,
    /// Row index `v` anchored visual mode at in the focused pane, if active, cleared on
    /// exit, submit, or whenever marks are otherwise touched directly.
    pub visual_anchor: Option<usize>,
    pub filter: Option<String>,
    pub remote_sort: Sort,
    /// Scroll/selection state for the remote list widget, kept on `App` (rather than
    /// recreated per frame) so the viewport offset persists across redraws instead of
    /// snapping back to the top every time.
    pub list_state: ListState,

    // Local filesystem pane.
    pub local_cwd: PathBuf,
    /// `[defaults] local_path` as configured, kept so reconnecting to a bookmark without its
    /// own `local_path` falls back to it rather than to `~/Downloads`.
    pub local_path_config: Option<String>,
    pub local_entries: Vec<LocalEntry>,
    pub local_cursor: usize,
    pub local_marked: HashSet<PathBuf>,
    pub local_filter: Option<String>,
    pub local_sort: Sort,
    pub local_list_state: ListState,

    pub focus: Focus,
    pub preview: Preview,
    pub preview_mode: PreviewMode,
    pub show_preview: bool,
    /// Line offset scrolled into the current preview text; reset whenever the previewed
    /// entry changes.
    pub preview_scroll: u16,
    /// Bumped every time a new preview is requested; a background fetch tags its result with
    /// the generation it was started for, so a slow response for an object you've since
    /// scrolled past gets silently dropped instead of overwriting a newer preview.
    pub preview_generation: u64,
    pub preview_tx: tokio::sync::mpsc::UnboundedSender<(u64, Preview)>,
    pub preview_rx: UnboundedReceiver<(u64, Preview)>,
    /// Off by default, most sessions never need to browse the local filesystem; it exists
    /// for uploading without drag-and-drop or typing a path. Toggle with `L`.
    pub show_local: bool,

    pub prompt: Option<Prompt>,
    pub show_help: bool,
    /// Line offset scrolled into the (taller-than-screen) help dialog.
    pub help_scroll: u16,
    pub show_events: bool,
    /// Line offset scrolled into the events log.
    pub events_scroll: u16,
    pub loading: bool,
    pub status: Option<(String, bool)>,
    /// When `status` was last set, the footer toast clears itself a few seconds after this
    /// rather than sitting there indefinitely until the next status message overwrites it.
    pub status_at: Option<Instant>,
    /// Every event logged this session, oldest first, capped at `MAX_EVENTS`, see `Event`.
    pub events: Vec<Event>,

    pub bookmark_wizard: Option<BookmarkWizard>,
    /// File path of the bookmark pending a delete confirmation.
    pub confirm_bookmark_delete: Option<String>,

    /// Active sync dialog (local⇄remote diff), if open.
    pub sync: Option<SyncState>,

    /// Cached recursive scan + extra matches for `/`'s deep fallback (see `app/deep.rs`), if
    /// the remote/local filter is non-empty. `None` whenever the filter is empty.
    pub deep_remote: Option<DeepRemoteMatches>,
    pub deep_local: Option<DeepLocalMatches>,
    /// Bumped each time a new remote deep-scan starts; a background scan tags its result with
    /// the generation it was started for, so a stale scan for a filter/prefix you've since
    /// changed gets dropped instead of clobbering a newer one.
    pub deep_scan_generation: u64,
    pub deep_scan_tx: tokio::sync::mpsc::UnboundedSender<(u64, Result<Vec<RemoteEntry>, String>)>,
    pub deep_scan_rx: UnboundedReceiver<(u64, Result<Vec<RemoteEntry>, String>)>,

    /// Pending "are you sure?" confirmation for a write action, if any.
    pub confirm_action: Option<confirm::ConfirmAction>,

    /// Items staged by `y`/`x` for the next `P` paste, if any.
    pub clip: Option<Clipboard>,

    pub jobs: Vec<Job>,
    /// Index into the transfers pane's display order (newest first, i.e. `jobs.iter().rev()`).
    pub jobs_cursor: usize,
    pub jobs_list_state: ListState,
    pub next_job_id: JobId,
    pub job_tx: tokio::sync::mpsc::UnboundedSender<ProgressMsg>,
    pub job_rx: UnboundedReceiver<ProgressMsg>,
    pub needs_remote_refresh: bool,
    pub needs_local_refresh: bool,
    /// Source to remove once a cross-backend `P` move's transfer job (keyed here by `JobId`)
    /// reports `Done`, see the cleanup check in `drain_job_messages`.
    pub pending_deletes: HashMap<JobId, PendingDelete>,

    pub spinner_frame: usize,
    pub should_quit: bool,
    pub theme: crate::ui::theme::Mode,
    /// Config-supplied hex overrides applied on top of the built-in palettes.
    pub theme_overrides: crate::app::ThemeOverrides,
    /// Keybind table built at startup from config, falling back to built-in defaults.
    pub keybinds: std::sync::Arc<crate::keys::Keybinds>,
    /// Terminal graphics capabilities (protocol + font size), queried once at startup, 
    /// shared so every image preview encodes for the same terminal.
    pub picker: ratatui_image::picker::Picker,
}

/// Config-supplied hex overrides for each theme mode.
pub struct ThemeOverrides {
    pub light: crate::config::PaletteOverride,
    pub dark: crate::config::PaletteOverride,
}

impl App {
    pub fn new(
        connections: Vec<(String, Connection)>,
        picker: ratatui_image::picker::Picker,
        app_config: crate::config::AppConfig,
    ) -> Self {
        let (job_tx, job_rx) = tokio::sync::mpsc::unbounded_channel();
        let (preview_tx, preview_rx) = tokio::sync::mpsc::unbounded_channel();
        let (deep_scan_tx, deep_scan_rx) = tokio::sync::mpsc::unbounded_channel();
        let keybinds = std::sync::Arc::new(crate::keys::Keybinds::load(&app_config.keybinds));
        let theme = match app_config.theme.mode.as_deref() {
            Some("dark") => crate::ui::theme::Mode::Dark,
            _ => crate::ui::theme::Mode::Light,
        };
        let theme_overrides = ThemeOverrides { light: app_config.theme.light, dark: app_config.theme.dark };
        let local_path_config = app_config.defaults.local_path;
        // No bookmark is connected yet, so only the global default can apply here; `connect`
        // re-resolves once it knows which bookmark you picked.
        let (local_cwd, local_path_warning) = local::resolve_start_dir(None, local_path_config.as_deref());
        let mut app = Self {
            screen: Screen::ConnectionPicker,
            connections,
            conn_selected: 0,
            client: None,
            connection: None,
            bucket: String::new(),
            buckets: Vec::new(),
            bucket_selected: 0,
            prefix: String::new(),
            entries: Vec::new(),
            cursor: 0,
            marked: HashSet::new(),
            visual_anchor: None,
            filter: None,
            // Remote: natural provider order (dirs-first, alphabetical) until the user sorts.
            remote_sort: Sort::default(),
            list_state: ListState::default(),
            local_cwd,
            local_path_config,
            local_entries: Vec::new(),
            local_cursor: 0,
            local_marked: HashSet::new(),
            local_filter: None,
            // Local: newest-first by default, the just-downloaded/edited file is what you want.
            local_sort: Sort { key: SortKey::Modified, dir: SortDir::Desc },
            local_list_state: ListState::default(),
            focus: Focus::Remote,
            preview: Preview::Empty,
            preview_mode: PreviewMode::default(),
            show_preview: app_config.defaults.show_preview.unwrap_or(true),
            preview_scroll: 0,
            preview_generation: 0,
            preview_tx,
            preview_rx,
            show_local: app_config.defaults.show_local.unwrap_or(false),
            prompt: None,
            show_help: false,
            help_scroll: 0,
            show_events: false,
            events_scroll: 0,
            loading: false,
            status: None,
            status_at: None,
            events: Vec::new(),
            bookmark_wizard: None,
            confirm_bookmark_delete: None,
            sync: None,
            deep_remote: None,
            deep_local: None,
            deep_scan_generation: 0,
            deep_scan_tx,
            deep_scan_rx,
            confirm_action: None,
            clip: None,
            jobs: Vec::new(),
            jobs_cursor: 0,
            jobs_list_state: ListState::default(),
            next_job_id: 1,
            job_tx,
            job_rx,
            needs_remote_refresh: false,
            needs_local_refresh: false,
            pending_deletes: HashMap::new(),
            spinner_frame: 0,
            should_quit: false,
            theme,
            theme_overrides,
            keybinds,
            picker,
        };
        if let Some(warning) = local_path_warning {
            app.log_event(warning, true, None);
        }
        app
    }

    /// Sets the footer toast (cleared automatically a few seconds later, see
    /// `main::run`'s tick handling) and logs the event to the `E` events log.
    pub fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        let msg = msg.into();
        self.status = Some((msg.clone(), is_error));
        self.status_at = Some(Instant::now());
        self.log_event(msg, is_error, None);
    }

    /// Like `set_status`, but also records the full error chain plus whatever connection
    /// context we have, the one-line message alone is often not enough to debug a
    /// connection problem, so the events log keeps the detail around for `E`.
    pub fn set_error(&mut self, context: &str, err: &anyhow::Error) {
        let msg = format!("{context}: {err}");
        self.status = Some((msg.clone(), true));
        self.status_at = Some(Instant::now());

        let mut detail = format!("{context}:\n");
        for (i, cause) in err.chain().enumerate() {
            detail.push_str(&format!("  {i}: {cause}\n"));
        }
        if let Some(client) = &self.client {
            detail.push_str("\nconnection:\n");
            for line in client.diagnostics() {
                detail.push_str(&format!("  {line}\n"));
            }
        }
        self.log_event(msg, true, Some(detail));
    }

    fn log_event(&mut self, message: String, is_error: bool, detail: Option<String>) {
        self.events.push(Event { message, is_error, at: Instant::now(), detail });
        if self.events.len() > MAX_EVENTS {
            self.events.remove(0);
        }
    }

    /// The active palette for the current theme mode, with config overrides applied.
    pub fn palette(&self) -> crate::ui::theme::Palette {
        let base = self.theme.palette();
        match self.theme {
            crate::ui::theme::Mode::Light => base.with_overrides(&self.theme_overrides.light),
            crate::ui::theme::Mode::Dark => base.with_overrides(&self.theme_overrides.dark),
        }
    }

    /// The hovered remote row, either a normal listing row, or (once the cursor runs past
    /// the end of the current directory's own listing) one of `/`'s deep extra matches
    /// appended below it. See `app/deep.rs`.
    pub fn current_entry(&self) -> Option<&RemoteEntry> {
        let visible = self.visible_entries();
        if self.cursor < visible.len() {
            return visible.get(self.cursor).copied();
        }
        self.deep_remote.as_ref()?.extra.get(self.cursor - visible.len())
    }

    /// Mirrors `current_entry`, but for the local pane.
    pub fn current_local_entry(&self) -> Option<&LocalEntry> {
        let visible = self.visible_local_entries();
        if self.local_cursor < visible.len() {
            return visible.get(self.local_cursor).copied();
        }
        self.deep_local.as_ref()?.extra.get(self.local_cursor - visible.len())
    }

    /// The job under the transfers pane cursor, in the same newest-first order it's rendered.
    pub fn current_job(&self) -> Option<&Job> {
        self.jobs.iter().rev().nth(self.jobs_cursor)
    }

    /// Signals every currently-running cancellable job to stop. Returns `true` if any were
    /// found (so callers can show a "cancelling..." status), `false` if nothing was running.
    pub fn cancel_running_jobs(&mut self) -> bool {
        let mut any = false;
        for job in self.jobs.iter().filter(|j| matches!(j.status, JobStatus::Running)) {
            if let Some(cancel) = &job.cancel {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                any = true;
            }
        }
        any
    }

    const FOCUS_ORDER: [Focus; 4] = [Focus::Local, Focus::Remote, Focus::Preview, Focus::Transfers];

    fn focus_visible(&self, focus: Focus) -> bool {
        match focus {
            Focus::Local => self.show_local,
            Focus::Remote => true,
            Focus::Preview => self.show_preview,
            Focus::Transfers => true,
        }
    }

    /// Moves focus by `steps` panes through whichever are currently visible (negative steps
    /// go backwards), skipping panes that are toggled off.
    fn step_focus(&mut self, steps: i32) {
        let len = Self::FOCUS_ORDER.len() as i32;
        let current = Self::FOCUS_ORDER.iter().position(|f| *f == self.focus).unwrap_or(0) as i32;
        for step in 1..=Self::FOCUS_ORDER.len() as i32 {
            let idx = (current + steps.signum() * step).rem_euclid(len) as usize;
            let candidate = Self::FOCUS_ORDER[idx];
            if self.focus_visible(candidate) {
                self.focus = candidate;
                return;
            }
        }
    }

    /// Cycles focus forward through visible panes, bound to `tab`.
    pub fn toggle_focus(&mut self) {
        self.step_focus(1);
    }

    /// Cycles focus backward through visible panes, bound to `shift+tab`.
    pub fn toggle_focus_back(&mut self) {
        self.step_focus(-1);
    }

    /// Jumps focus directly to `focus` if that pane is currently visible, bound to number
    /// keys `1`-`4` so a specific pane is always one keystroke away.
    pub fn focus_pane(&mut self, focus: Focus) {
        if self.focus_visible(focus) {
            self.focus = focus;
        }
    }

    /// Scrolls the help dialog by `delta` lines (it's taller than the screen).
    pub fn scroll_help(&mut self, delta: i32) {
        let next = self.help_scroll as i32 + delta;
        self.help_scroll = next.max(0) as u16;
    }

    /// Scrolls the (taller-than-screen, most-recent-first) events log by `delta` lines.
    pub fn scroll_events(&mut self, delta: i32) {
        let next = self.events_scroll as i32 + delta;
        self.events_scroll = next.max(0) as u16;
    }

    /// Clears the footer toast a few seconds after it was set, called every tick from the
    /// main loop, so a status message doesn't linger indefinitely once nothing's overwritten
    /// it. The full event stays in the `E` log regardless.
    pub fn expire_status(&mut self) {
        const STATUS_LIFETIME: std::time::Duration = std::time::Duration::from_secs(5);
        if self.status_at.is_some_and(|at| at.elapsed() >= STATUS_LIFETIME) {
            self.status = None;
            self.status_at = None;
        }
    }

    /// Applies any queued progress messages from background download/upload tasks.
    pub fn drain_job_messages(&mut self) {
        while let Ok(msg) = self.job_rx.try_recv() {
            match msg {
                ProgressMsg::New { id, label, kind, total_bytes } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.label = label;
                        job.kind = kind;
                        job.total_bytes = total_bytes;
                    }
                }
                ProgressMsg::Advance { id, delta } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.done_bytes += delta;
                    }
                }
                ProgressMsg::Done { id, kind } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = JobStatus::Done;
                        job.done_bytes = job.total_bytes;
                    }
                    match kind {
                        JobKind::Download | JobKind::Zip | JobKind::LocalCopy | JobKind::LocalMove => {
                            self.needs_local_refresh = true
                        }
                        JobKind::Upload | JobKind::RemoteCopy | JobKind::RemoteMove | JobKind::RemoteDelete => {
                            self.needs_remote_refresh = true
                        }
                    }
                    // A cross-backend `P` move transferred the file as a copy; now that the
                    // transfer succeeded, remove the source. Tracked separately from the job
                    // itself since the two can fail independently.
                    if let Some(delete) = self.pending_deletes.remove(&id) {
                        match delete {
                            PendingDelete::Local(path) => {
                                let result =
                                    if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                                match result {
                                    Ok(()) => self.needs_local_refresh = true,
                                    Err(err) => {
                                        let _ = self.job_tx.send(ProgressMsg::Failed {
                                            id,
                                            error: format!("moved, but failed to remove source: {err}"),
                                        });
                                    }
                                }
                            }
                            PendingDelete::Remote(key, is_prefix) => {
                                if let Some(client) = self.client.clone() {
                                    let bucket = self.bucket.clone();
                                    let tx = self.job_tx.clone();
                                    tokio::spawn(async move {
                                        let result = if is_prefix {
                                            client.delete_prefix(&bucket, &key).await
                                        } else {
                                            client.delete_object(&bucket, &key).await
                                        };
                                        if let Err(err) = result {
                                            let _ = tx.send(ProgressMsg::Failed {
                                                id,
                                                error: format!("moved, but failed to remove source: {err}"),
                                            });
                                        }
                                    });
                                    self.needs_remote_refresh = true;
                                }
                            }
                        }
                    }
                }
                ProgressMsg::Cancelled { id } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = JobStatus::Cancelled;
                    }
                }
                ProgressMsg::Failed { id, error } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = JobStatus::Failed(error);
                    }
                }
            }
        }
    }
}
