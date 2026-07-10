//! Application state and behavior.
//!
//! [`App`] is the single coordinator that the render loop (`main`) and input layer (`input`)
//! drive. Its behavior is split across sibling modules by concern — [`browser`] (navigation +
//! transfers), [`bookmarks`] (the add/edit/delete wizard), and [`preview`] (the preview pane) —
//! each of which adds `impl App` blocks. This module holds the struct itself plus the small,
//! cross-cutting core: construction, status/error plumbing, focus, and job-progress draining.

mod bookmarks;
mod browser;
mod confirm;
mod preview;
mod sort;
mod sync;

pub use bookmarks::{BookmarkWizard, BOOKMARK_FIELDS};
pub use confirm::ConfirmKind;
pub use preview::Preview;
pub use sort::{Sort, SortDir, SortKey};
pub use sync::{SyncAction, SyncDirection, SyncState};

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::config::Connection;
use crate::jobs::{Job, JobId, JobKind, JobStatus, ProgressMsg};
use crate::local::{self, LocalEntry};
use crate::provider::{RemoteEntry, StorageProvider};

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
    /// Mask typed characters as `•` — used for the secret_access_key wizard field.
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
    pub filter: Option<String>,
    pub remote_sort: Sort,
    /// Scroll/selection state for the remote list widget — kept on `App` (rather than
    /// recreated per frame) so the viewport offset persists across redraws instead of
    /// snapping back to the top every time.
    pub list_state: ListState,

    // Local filesystem pane.
    pub local_cwd: PathBuf,
    pub local_entries: Vec<LocalEntry>,
    pub local_cursor: usize,
    pub local_marked: HashSet<PathBuf>,
    pub local_filter: Option<String>,
    pub local_sort: Sort,
    pub local_list_state: ListState,

    pub focus: Focus,
    pub preview: Preview,
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
    /// Off by default — most sessions never need to browse the local filesystem; it exists
    /// for uploading without drag-and-drop or typing a path. Toggle with `L`.
    pub show_local: bool,

    pub prompt: Option<Prompt>,
    pub show_help: bool,
    /// Line offset scrolled into the (taller-than-screen) help dialog.
    pub help_scroll: u16,
    pub show_diagnostics: bool,
    pub loading: bool,
    pub status: Option<(String, bool)>,
    /// Full multi-line detail behind the last error, shown in the diagnostics popup — the
    /// one-line `status` message alone is often not enough to debug a connection problem.
    pub last_error: Option<String>,

    pub bookmark_wizard: Option<BookmarkWizard>,
    /// File path of the bookmark pending a delete confirmation.
    pub confirm_bookmark_delete: Option<String>,

    /// Active sync dialog (local⇄remote diff), if open.
    pub sync: Option<SyncState>,

    /// Pending "are you sure?" confirmation for a write action, if any.
    pub confirm_action: Option<confirm::ConfirmAction>,

    pub jobs: Vec<Job>,
    /// Index into the transfers pane's display order (newest first, i.e. `jobs.iter().rev()`).
    pub jobs_cursor: usize,
    pub jobs_list_state: ListState,
    pub next_job_id: JobId,
    pub job_tx: tokio::sync::mpsc::UnboundedSender<ProgressMsg>,
    pub job_rx: UnboundedReceiver<ProgressMsg>,
    pub needs_remote_refresh: bool,
    pub needs_local_refresh: bool,

    pub spinner_frame: usize,
    pub should_quit: bool,
    pub theme: crate::ui::theme::Mode,
}

impl App {
    pub fn new(connections: Vec<(String, Connection)>) -> Self {
        let (job_tx, job_rx) = tokio::sync::mpsc::unbounded_channel();
        let (preview_tx, preview_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
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
            filter: None,
            // Remote: natural provider order (dirs-first, alphabetical) until the user sorts.
            remote_sort: Sort::default(),
            list_state: ListState::default(),
            local_cwd: local::default_download_dir(),
            local_entries: Vec::new(),
            local_cursor: 0,
            local_marked: HashSet::new(),
            local_filter: None,
            // Local: newest-first by default — the just-downloaded/edited file is what you want.
            local_sort: Sort { key: SortKey::Modified, dir: SortDir::Desc },
            local_list_state: ListState::default(),
            focus: Focus::Remote,
            preview: Preview::Empty,
            show_preview: true,
            preview_scroll: 0,
            preview_generation: 0,
            preview_tx,
            preview_rx,
            show_local: false,
            prompt: None,
            show_help: false,
            help_scroll: 0,
            show_diagnostics: false,
            loading: false,
            status: None,
            last_error: None,
            bookmark_wizard: None,
            confirm_bookmark_delete: None,
            sync: None,
            confirm_action: None,
            jobs: Vec::new(),
            jobs_cursor: 0,
            jobs_list_state: ListState::default(),
            next_job_id: 1,
            job_tx,
            job_rx,
            needs_remote_refresh: false,
            needs_local_refresh: false,
            spinner_frame: 0,
            should_quit: false,
            theme: crate::ui::theme::Mode::default(),
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = Some((msg.into(), is_error));
        if !is_error {
            self.last_error = None;
        }
    }

    /// Records both the short one-line status (shown in the footer) and the full error
    /// chain (shown in the `E` diagnostics popup), plus whatever connection context we have.
    pub fn set_error(&mut self, context: &str, err: &anyhow::Error) {
        self.status = Some((format!("{context}: {err}"), true));
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
        self.last_error = Some(detail);
    }

    pub fn current_entry(&self) -> Option<&RemoteEntry> {
        self.visible_entries().get(self.cursor).copied()
    }

    pub fn current_local_entry(&self) -> Option<&LocalEntry> {
        self.visible_local_entries().get(self.local_cursor).copied()
    }

    /// The job under the transfers pane cursor, in the same newest-first order it's rendered.
    pub fn current_job(&self) -> Option<&Job> {
        self.jobs.iter().rev().nth(self.jobs_cursor)
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

    /// Cycles focus forward through visible panes — bound to `tab`.
    pub fn toggle_focus(&mut self) {
        self.step_focus(1);
    }

    /// Cycles focus backward through visible panes — bound to `shift+tab`.
    pub fn toggle_focus_back(&mut self) {
        self.step_focus(-1);
    }

    /// Jumps focus directly to `focus` if that pane is currently visible — bound to number
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
                        JobKind::Download | JobKind::Zip => self.needs_local_refresh = true,
                        JobKind::Upload => self.needs_remote_refresh = true,
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
