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
mod composition;
mod confirm;
mod deep;
mod preview;
mod sort;
mod structure;
mod sync;

pub use bookmarks::{
    is_credential_field, BookmarkEditor, BookmarkWizard, EditorFocus, BOOKMARK_FIELDS, EDITOR_FIELDS, EDITOR_LABELS,
};
pub use browser::EditorTarget;
pub use clipboard::{Clipboard, ClipMode, PendingDelete};
pub use composition::Composition;
pub use confirm::{ConfirmAction, ConfirmKind};
pub use deep::{DeepLocalMatches, DeepRemoteMatches};
pub use preview::{InfoDetails, Preview, PreviewMode};
pub use sort::{Sort, SortDir, SortKey};
pub use structure::StructureMap;
pub use sync::{SyncAction, SyncDirection, SyncState};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedReceiver;

use crossterm::event::KeyCode;

use crate::config::Connection;
use crate::keys::BrowserAction;
use crate::jobs::{self, Job, JobId, JobKind, JobSpec, JobStatus, ProgressMsg};
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

/// Max gap between two clicks on the same row for the second to count as a double-click.
pub const DOUBLE_CLICK_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

/// How long an `O`-edited row stays highlighted, fading from `good` back to its normal colour.
/// See [`App::recent_write`].
pub const RECENT_WRITE_FADE: std::time::Duration = std::time::Duration::from_secs(3);

/// How long the footer echoes the key that was just pressed. Long enough to register at
/// reading speed, short enough that a quick sequence of keys doesn't smear into one blob.
pub const KEY_FLASH_LIFETIME: std::time::Duration = std::time::Duration::from_secs(1);

/// The key press the footer is currently echoing, see [`App::flash_key`].
pub struct KeyFlash {
    /// The key as typed, e.g. `"D"`, `"/"`, `"enter"`.
    pub key: String,
    /// What the press did, for the chip beside the keybind bar.
    pub label: &'static str,
    /// The bar entry to light up, if this action has one. `None` for a jobs-pane action, which
    /// never appears on the bar and so always gets the chip instead.
    pub action: Option<BrowserAction>,
    pub at: Instant,
}

/// How a listing row shows the running job that touches it, see [`App::local_row_transfer`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RowTransfer {
    /// This side is being written: fill the row to the ratio.
    Progress(f64),
    /// This side is being read, or its size isn't known yet: spin, don't fill.
    Active,
}

/// Identifies the item [`App::recent_write`] is currently highlighting.
#[derive(Clone, PartialEq, Eq)]
pub enum RecentWrite {
    Local(PathBuf),
    Remote { bucket: String, key: String },
}

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
    Jobs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Rename,
    UploadPath,
    Filter,
    BookmarkField,
    ProfileRename,
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
    /// Row a shift-range measures from: set by every single-row pick (a plain click, `space`,
    /// ctrl+click), consumed by shift+click and shift+↑/↓. Separate from `visual_anchor`, which
    /// also means "visual mode is live and every cursor move re-sweeps"; this one just records
    /// where the last selection started.
    pub mark_anchor: Option<usize>,
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
    /// Which browser pane (`Remote`/`Local`) the preview/info pane reflects. Held separately from
    /// `focus` so focusing or scrolling the preview pane, and the `p`/`i` tab toggles used there,
    /// keep previewing that pane's selection instead of blanking.
    pub preview_source: Focus,
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
    /// Mirrors `preview_generation`, shared with background preview tasks so they can bail out
    /// *before* doing the expensive work (image decode/resize, a directory walk) once they're
    /// stale, rather than only after. Without this, rapidly arrowing through a folder of large
    /// images queues up a full decode per keystroke that all still run to completion (spending
    /// real CPU time on results that get thrown away), so the one you're actually looking at sits
    /// stuck behind a backlog of already-abandoned ones instead of appearing quickly.
    pub preview_generation_live: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub preview_tx: tokio::sync::mpsc::UnboundedSender<(u64, Preview)>,
    pub preview_rx: UnboundedReceiver<(u64, Preview)>,
    /// Line offset scrolled into the Info tab's metadata body, independent of `preview_scroll`
    /// (the Preview/content tab's own scroll). Reset whenever the previewed entry changes.
    pub info_scroll: u16,
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
    /// The key press the footer is echoing right now, cleared by `expire_status`.
    pub key_flash: Option<KeyFlash>,
    /// Every event logged this session, oldest first, capped at `MAX_EVENTS`, see `Event`.
    pub events: Vec<Event>,

    pub bookmark_wizard: Option<BookmarkWizard>,
    /// Active inline bookmark editor (`e` on the connection picker), if open. Distinct from the
    /// step-by-step `bookmark_wizard` used for adding.
    pub bookmark_editor: Option<BookmarkEditor>,
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
    /// Rolling per-tick transfer-rate samples (bytes/sec), newest at the back, feeding the
    /// jobs-pane sparkline. Drains to empty once transfers stop, so the trace vanishes.
    pub throughput_history: VecDeque<u64>,
    /// Monotonic count of every byte moved this session, summed from job progress deltas. The
    /// sparkline reads its rate from the change in this between ticks, independent of which
    /// individual jobs are alive.
    pub session_bytes: u64,
    /// (time, `session_bytes`) captured at the last [`App::sample_throughput`], to divide the
    /// byte delta by real elapsed time rather than assuming a fixed tick.
    pub last_throughput_sample: (Instant, u64),
    /// Index into the jobs pane's display order (newest first, i.e. `jobs.iter().rev()`).
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
    /// Rows in the browser list viewport (pane height minus borders), refreshed each render so
    /// `ctrl-d`/`ctrl-f`/`PageDown` jump by a screenful that matches what's actually visible.
    pub list_viewport: u16,
    /// Half-entered `gg` chord: set by a lone `g`, consumed (jump to top) by the next `g`.
    pub pending_g: bool,

    // Mouse hit-testing: rendered rects/regions captured each frame by the matching `draw_*`
    // function, consumed by `input::handle_mouse` on the next click/scroll. A frame behind the
    // click by definition, but the layout can't change without a redraw in between, so it's
    // always accurate at click time.
    pub local_pane_area: ratatui::layout::Rect,
    pub remote_pane_area: ratatui::layout::Rect,
    pub preview_pane_area: ratatui::layout::Rect,
    pub jobs_pane_area: ratatui::layout::Rect,
    /// Clickable column ranges on the remote pane's title row: `(start_col, end_col, prefix)` in
    /// absolute screen columns; clicking jumps `self.prefix` to that breadcrumb segment.
    pub breadcrumb_hits: Vec<(u16, u16, String)>,
    /// `(pane, row index, when)` of the last row click, so a second click on the same row within
    /// [`DOUBLE_CLICK_WINDOW`] opens/enters it instead of just (re)selecting it.
    pub last_row_click: Option<(Focus, usize, Instant)>,

    /// Set by `O` ([`App::request_open_editor`]); the main loop (`lib::run`) picks this up each
    /// iteration since it's the only place holding the `Terminal` needed to suspend the screen.
    pub pending_editor: Option<EditorTarget>,
    /// The most recently `O`-edited item and when, so its row fades from `good` back to normal
    /// for [`RECENT_WRITE_FADE`], a quick "this one just changed" cue. Set immediately for a
    /// local edit; for a remote edit, only once the re-upload job actually finishes (via
    /// `pending_write_marks`), since the object isn't updated until then.
    pub recent_write: Option<(RecentWrite, Instant)>,
    /// `JobId -> (bucket, key)` for an in-flight `O`-triggered re-upload, consumed in
    /// `drain_job_messages` on `Done` to set `recent_write` at the right time.
    pub pending_write_marks: HashMap<JobId, (String, String)>,
    /// Loaded once from `[ui] mouse` at startup so the editor suspend/resume can re-toggle
    /// capture around the child process the same way `lib::setup_terminal` set it up.
    pub mouse_enabled: bool,

    pub should_quit: bool,
    pub theme: crate::ui::theme::Mode,
    /// Config-supplied hex overrides applied on top of the built-in palettes.
    pub theme_overrides: crate::app::ThemeOverrides,
    /// Resolved once at startup from `[ui] icons` in config.toml (see
    /// [`crate::ui::theme::resolve_icon_set`]); never `IconSet::Auto` by the time it lands here.
    pub icons: crate::config::IconSet,
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
        let icons = crate::ui::theme::resolve_icon_set(app_config.ui.icons);
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
            mark_anchor: None,
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
            preview_source: Focus::Remote,
            preview: Preview::Empty,
            preview_mode: PreviewMode::default(),
            show_preview: app_config.defaults.show_preview.unwrap_or(true),
            preview_scroll: 0,
            preview_generation: 0,
            preview_generation_live: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            info_scroll: 0,
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
            key_flash: None,
            events: Vec::new(),
            bookmark_wizard: None,
            bookmark_editor: None,
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
            throughput_history: VecDeque::new(),
            session_bytes: 0,
            last_throughput_sample: (Instant::now(), 0),
            jobs_cursor: 0,
            jobs_list_state: ListState::default(),
            next_job_id: 1,
            job_tx,
            job_rx,
            needs_remote_refresh: false,
            needs_local_refresh: false,
            pending_deletes: HashMap::new(),
            spinner_frame: 0,
            list_viewport: 20,
            pending_g: false,
            local_pane_area: ratatui::layout::Rect::default(),
            remote_pane_area: ratatui::layout::Rect::default(),
            preview_pane_area: ratatui::layout::Rect::default(),
            jobs_pane_area: ratatui::layout::Rect::default(),
            breadcrumb_hits: Vec::new(),
            last_row_click: None,
            pending_editor: None,
            recent_write: None,
            pending_write_marks: HashMap::new(),
            mouse_enabled: app_config.ui.mouse,
            should_quit: false,
            theme,
            theme_overrides,
            icons,
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

    /// The job under the jobs pane cursor, in the same newest-first order it's rendered.
    pub fn current_job(&self) -> Option<&Job> {
        self.jobs.iter().rev().nth(self.jobs_cursor)
    }

    /// Registers a network job's row and starts the task behind it from the same [`JobSpec`], so
    /// the two can't be built from different arguments. Used by every download/upload/zip entry
    /// point; the local and remote transfer jobs push their rows directly, having no spec.
    pub fn start_job(
        &mut self,
        label: String,
        kind: JobKind,
        local_path: PathBuf,
        remote_key: Option<String>,
        spec: JobSpec,
    ) -> Option<JobId> {
        let client = self.client.clone()?;
        let id = self.next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.push(Job {
            id,
            label,
            kind,
            total_bytes: 0,
            done_bytes: 0,
            status: JobStatus::Running,
            local_path,
            remote_key,
            cancel: cancel.clone(),
            spec: Some(spec.clone()),
        });
        jobs::spawn(client, id, &spec, cancel, self.job_tx.clone());
        Some(id)
    }

    /// Signals the hovered job to stop. The row stays `Running` until the task itself reports
    /// back, since only it knows when it actually reached a stopping point.
    pub fn cancel_selected_job(&mut self) {
        let Some((cancel, label, running)) = self
            .current_job()
            .map(|j| (j.cancel.clone(), j.label.clone(), matches!(j.status, JobStatus::Running)))
        else {
            return;
        };
        if !running {
            self.set_status("that job has already finished", true);
            return;
        }
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.set_status(format!("cancelling {label}..."), false);
    }

    /// Runs the hovered job again in place, reusing its id so the row restarts rather than a
    /// duplicate appearing, and so anything else keyed by that id (a cross-backend move's
    /// pending source delete) still lines up with it.
    pub fn retry_selected_job(&mut self) {
        let Some((id, label, spec, running)) = self
            .current_job()
            .map(|j| (j.id, j.label.clone(), j.spec.clone(), matches!(j.status, JobStatus::Running)))
        else {
            return;
        };
        if running {
            self.set_status("that job is still running", true);
            return;
        }
        let Some(spec) = spec else {
            self.set_status("moves and deletes can't be retried, run it again from the pane", true);
            return;
        };
        let Some(client) = self.client.clone() else {
            self.set_status("not connected", true);
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.status = JobStatus::Running;
            job.done_bytes = 0;
            job.cancel = cancel.clone();
        }
        jobs::spawn(client, id, &spec, cancel, self.job_tx.clone());
        self.set_status(format!("retrying {label}"), false);
    }

    /// Drops the hovered job's row. Finished jobs only: a running one would keep transferring
    /// with nothing on screen accounting for it.
    pub fn dismiss_selected_job(&mut self) {
        let Some((id, running)) = self.current_job().map(|j| (j.id, matches!(j.status, JobStatus::Running))) else {
            return;
        };
        if running {
            self.set_status("cancel it before dismissing it", true);
            return;
        }
        self.jobs.retain(|j| j.id != id);
        self.clamp_jobs_cursor();
    }

    /// Drops every finished row at once, for clearing out a long session's backlog.
    pub fn dismiss_finished_jobs(&mut self) {
        let before = self.jobs.len();
        self.jobs.retain(|j| matches!(j.status, JobStatus::Running));
        let cleared = before - self.jobs.len();
        self.clamp_jobs_cursor();
        let noun = if cleared == 1 { "job" } else { "jobs" };
        self.set_status(format!("cleared {cleared} finished {noun}"), false);
    }

    /// Keeps the jobs cursor on a real row after rows are removed underneath it.
    fn clamp_jobs_cursor(&mut self) {
        self.jobs_cursor = self.jobs_cursor.min(self.jobs.len().saturating_sub(1));
    }

    const FOCUS_ORDER: [Focus; 4] = [Focus::Local, Focus::Remote, Focus::Preview, Focus::Jobs];

    fn focus_visible(&self, focus: Focus) -> bool {
        match focus {
            Focus::Local => self.show_local,
            Focus::Remote => true,
            Focus::Preview => self.show_preview,
            Focus::Jobs => true,
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
        if self.key_flash.as_ref().is_some_and(|f| f.at.elapsed() >= KEY_FLASH_LIFETIME) {
            self.key_flash = None;
        }
    }

    /// Lights up the footer for a moment with the key just pressed and what it did, so a viewer
    /// (or the person recording a demo) can follow along. Actions already on the keybind bar
    /// highlight in place; anything else appears as a chip on the end of the line. Only for
    /// actions, never navigation, see [`BrowserAction::flashes`].
    pub fn flash_key(&mut self, code: KeyCode, action: BrowserAction) {
        self.key_flash = Some(KeyFlash {
            key: crate::keys::key_label(code),
            label: action.label(),
            action: Some(action),
            at: Instant::now(),
        });
    }

    /// [`flash_key`](Self::flash_key) for a jobs-pane action, which has no keybind-bar entry to
    /// light up and so always shows as a chip.
    pub fn flash_job_key(&mut self, code: KeyCode, action: crate::keys::JobAction) {
        self.key_flash = Some(KeyFlash {
            key: crate::keys::key_label(code),
            label: action.label(),
            action: None,
            at: Instant::now(),
        });
    }

    /// Samples the current aggregate transfer rate for the sparkline, once per tick. Rate is the
    /// change in `session_bytes` divided by real elapsed time, so it's correct regardless of tick
    /// jitter. While anything is transferring the true sample is pushed; once idle, zeros are
    /// pushed so the trace scrolls off and then clears itself, leaving the pane clean.
    pub fn sample_throughput(&mut self) {
        const CAP: usize = 60;
        let now = Instant::now();
        let (last_at, last_bytes) = self.last_throughput_sample;
        let dt = now.duration_since(last_at).as_secs_f64();
        let delta = self.session_bytes.saturating_sub(last_bytes);
        let bytes_per_sec = if dt > 0.0 { (delta as f64 / dt) as u64 } else { 0 };
        self.last_throughput_sample = (now, self.session_bytes);

        let transferring = self.jobs.iter().any(|j| matches!(j.status, JobStatus::Running));
        if transferring {
            self.throughput_history.push_back(bytes_per_sec);
        } else if !self.throughput_history.is_empty() {
            self.throughput_history.push_back(0);
            if self.throughput_history.iter().all(|&s| s == 0) {
                self.throughput_history.clear();
            }
        }
        while self.throughput_history.len() > CAP {
            self.throughput_history.pop_front();
        }
    }

    /// The most recent throughput sample (bytes/sec), for the sparkline's live speed label.
    pub fn current_throughput(&self) -> u64 {
        self.throughput_history.back().copied().unwrap_or(0)
    }

    /// How a listing row should show the job touching it, if one is running. The destination
    /// row fills as the bytes land, so a download reads in the local pane (where the file is
    /// going) and an upload in the remote pane. The source row only spins: it isn't growing,
    /// and a bar on both ends made it ambiguous which way anything was moving.
    pub fn local_row_transfer(&self, path: &Path) -> Option<RowTransfer> {
        self.row_transfer(|job| job.local_path == path, JobKind::writes_local)
    }

    /// Remote counterpart to [`local_row_transfer`](Self::local_row_transfer), matched on the
    /// object key a job reads or writes.
    pub fn remote_row_transfer(&self, key: &str) -> Option<RowTransfer> {
        self.row_transfer(|job| job.remote_key.as_deref() == Some(key), JobKind::writes_remote)
    }

    /// The first running job `is_target` picks out, rendered as a bar if this side is the one
    /// being written and its size is known, and as a spinner otherwise. A job whose total is
    /// still unknown (before the size probe lands) spins too: a bar sitting at zero reads as a
    /// stalled transfer.
    fn row_transfer(&self, is_target: impl Fn(&Job) -> bool, writes_here: impl Fn(JobKind) -> bool) -> Option<RowTransfer> {
        let job = self.jobs.iter().find(|job| matches!(job.status, JobStatus::Running) && is_target(job))?;
        match writes_here(job.kind) {
            true if job.total_bytes > 0 => Some(RowTransfer::Progress(job.progress_ratio())),
            _ => Some(RowTransfer::Active),
        }
    }

    /// Files being downloaded into the local pane's current directory that aren't in its listing
    /// yet, as `(name, progress)`. A download's destination is the whole point of the operation,
    /// but it only joins the listing on the refresh that follows `Done`, so until then it's
    /// rendered as an incoming row rather than not being shown at all.
    pub fn incoming_local_rows(&self) -> Vec<(String, Option<f64>)> {
        self.jobs
            .iter()
            .filter(|job| matches!(job.status, JobStatus::Running) && job.kind.writes_local())
            .filter(|job| job.local_path.parent() == Some(self.local_cwd.as_path()))
            .filter_map(|job| {
                let name = job.local_path.file_name()?.to_string_lossy().to_string();
                // Once the real file joins the listing it renders (and fills) as itself.
                if self.local_entries.iter().any(|e| e.path == job.local_path) {
                    return None;
                }
                Some((name, (job.total_bytes > 0).then(|| job.progress_ratio())))
            })
            .collect()
    }

    /// Points the local pane at the hovered job's destination directory and puts the cursor on
    /// the file itself. The pane is the natural place to watch a download land, but a job
    /// started from elsewhere leaves it looking at some other directory entirely.
    pub fn locate_selected_job(&mut self) {
        let Some(path) = self.current_job().map(|j| j.local_path.clone()) else {
            return;
        };
        let Some(dir) = path.parent().filter(|d| d.is_dir()) else {
            self.set_status("this job has no local destination", true);
            return;
        };
        if dir != self.local_cwd {
            self.local_cwd = dir.to_path_buf();
            self.local_cursor = 0;
            self.local_filter = None;
            self.clear_deep_matches();
        }
        self.refresh_local();
        self.show_local = true;
        if let Some(index) = self.visible_local_entries().iter().position(|e| e.path == path) {
            self.local_cursor = index;
        }
        self.focus = Focus::Local;
        self.refresh_preview();
    }

    /// Applies any queued progress messages from background download/upload tasks.
    pub fn drain_job_messages(&mut self) {
        while let Ok(msg) = self.job_rx.try_recv() {
            match msg {
                ProgressMsg::New { id, label, kind, total_bytes } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.label = label.clone();
                        job.kind = kind;
                        job.total_bytes = total_bytes;
                    }
                    self.set_status(format!("{} {label}", job_verb(kind, false)), false);
                }
                ProgressMsg::Advance { id, delta } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.done_bytes += delta;
                        self.session_bytes += delta; // feeds the throughput sparkline
                    }
                }
                ProgressMsg::Done { id, kind } => {
                    let label = self.jobs.iter().find(|j| j.id == id).map(|j| j.label.clone()).unwrap_or_default();
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = JobStatus::Done;
                        job.done_bytes = job.total_bytes;
                    }
                    self.set_status(format!("{} {label}", job_verb(kind, true)), false);
                    match kind {
                        JobKind::Download | JobKind::Zip | JobKind::LocalCopy | JobKind::LocalMove => {
                            self.needs_local_refresh = true
                        }
                        JobKind::Upload | JobKind::RemoteCopy | JobKind::RemoteMove | JobKind::RemoteDelete => {
                            self.needs_remote_refresh = true
                        }
                    }
                    if let Some((bucket, key)) = self.pending_write_marks.remove(&id) {
                        self.recent_write = Some((RecentWrite::Remote { bucket, key }, Instant::now()));
                    }
                    // Whatever just finished, the currently-hovered item's cached preview may now
                    // be stale (most notably: an `O`-edited remote file re-uploaded in the
                    // background). Cheap no-op if it wasn't the item involved.
                    self.refresh_preview();
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
                    let label = self.jobs.iter().find(|j| j.id == id).map(|j| j.label.clone()).unwrap_or_default();
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = JobStatus::Cancelled;
                    }
                    self.set_status(format!("cancelled {label}"), false);
                }
                ProgressMsg::Failed { id, error } => {
                    let label = self.jobs.iter().find(|j| j.id == id).map(|j| j.label.clone()).unwrap_or_default();
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = JobStatus::Failed(error.clone());
                    }
                    self.set_status(format!("{label}: {error}"), true);
                }
            }
        }
    }
}

/// Verb for a job kind, progressive ("downloading") when `done` is false or past ("downloaded")
/// when true. Used for the started/finished event every read/write logs in `drain_job_messages`.
fn job_verb(kind: JobKind, done: bool) -> &'static str {
    match (kind, done) {
        (JobKind::Download | JobKind::Zip, false) => "downloading",
        (JobKind::Download | JobKind::Zip, true) => "downloaded",
        (JobKind::Upload, false) => "uploading",
        (JobKind::Upload, true) => "uploaded",
        (JobKind::LocalCopy | JobKind::RemoteCopy, false) => "copying",
        (JobKind::LocalCopy | JobKind::RemoteCopy, true) => "copied",
        (JobKind::LocalMove | JobKind::RemoteMove, false) => "moving",
        (JobKind::LocalMove | JobKind::RemoteMove, true) => "moved",
        (JobKind::RemoteDelete, false) => "deleting",
        (JobKind::RemoteDelete, true) => "deleted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        App::new(Vec::new(), ratatui_image::picker::Picker::halfblocks(), crate::config::AppConfig::default())
    }

    /// Pushes a bare job row. No task runs behind it, which is what these tests want: the
    /// actions are about the row and the cancel flag, not the transfer.
    fn push_job(app: &mut App, status: JobStatus, spec: Option<JobSpec>) -> JobId {
        let id = app.next_id();
        app.jobs.push(Job {
            id,
            label: format!("job-{id}"),
            kind: JobKind::Download,
            total_bytes: 100,
            done_bytes: 50,
            status,
            local_path: PathBuf::new(),
            remote_key: None,
            cancel: Arc::new(AtomicBool::new(false)),
            spec,
        });
        id
    }

    #[test]
    fn cancel_sets_the_flag_on_the_hovered_job_only() {
        let mut app = test_app();
        let first = push_job(&mut app, JobStatus::Running, None);
        let second = push_job(&mut app, JobStatus::Running, None);

        // The pane renders newest-first, so cursor 0 is the job pushed last.
        app.jobs_cursor = 0;
        app.cancel_selected_job();

        let flagged = |id: JobId| {
            app.jobs.iter().find(|j| j.id == id).unwrap().cancel.load(std::sync::atomic::Ordering::Relaxed)
        };
        assert!(flagged(second), "the hovered job is the one that gets cancelled");
        assert!(!flagged(first), "the job above it keeps running");
    }

    #[test]
    fn cancelling_a_finished_job_is_refused() {
        let mut app = test_app();
        push_job(&mut app, JobStatus::Done, None);
        app.cancel_selected_job();
        assert!(app.status.as_ref().is_some_and(|(_, is_err)| *is_err));
    }

    #[test]
    fn dismiss_drops_a_finished_row_but_not_a_running_one() {
        let mut app = test_app();
        push_job(&mut app, JobStatus::Running, None);
        app.dismiss_selected_job();
        assert_eq!(app.jobs.len(), 1, "a running job stays until it's cancelled");

        app.jobs[0].status = JobStatus::Failed("boom".into());
        app.dismiss_selected_job();
        assert!(app.jobs.is_empty());
    }

    #[test]
    fn dismiss_finished_clears_every_settled_row_and_keeps_the_running_ones() {
        let mut app = test_app();
        push_job(&mut app, JobStatus::Done, None);
        let running = push_job(&mut app, JobStatus::Running, None);
        push_job(&mut app, JobStatus::Cancelled, None);

        app.dismiss_finished_jobs();

        assert_eq!(app.jobs.len(), 1);
        assert_eq!(app.jobs[0].id, running);
    }

    #[test]
    fn dismissing_the_last_row_leaves_the_cursor_on_a_real_one() {
        let mut app = test_app();
        push_job(&mut app, JobStatus::Done, None);
        push_job(&mut app, JobStatus::Done, None);
        app.jobs_cursor = 1;

        app.dismiss_selected_job();

        assert_eq!(app.jobs.len(), 1);
        assert_eq!(app.jobs_cursor, 0);
    }

    #[test]
    fn retrying_a_job_with_no_spec_explains_itself_rather_than_doing_nothing() {
        let mut app = test_app();
        push_job(&mut app, JobStatus::Failed("boom".into()), None);
        app.retry_selected_job();

        let (msg, is_err) = app.status.clone().expect("a status line");
        assert!(is_err);
        assert!(msg.contains("can't be retried"), "got {msg:?}");
        assert!(matches!(app.jobs[0].status, JobStatus::Failed(_)), "the row is left as it was");
    }

    /// Pushes a job with the source/destination fields a real one of that kind would carry.
    fn push_transfer(app: &mut App, kind: JobKind, local: &str, remote: &str, total: u64) -> JobId {
        let id = app.next_id();
        app.jobs.push(Job {
            id,
            label: "x".into(),
            kind,
            total_bytes: total,
            done_bytes: total / 4,
            status: JobStatus::Running,
            local_path: PathBuf::from(local),
            remote_key: Some(remote.to_string()),
            cancel: Arc::new(AtomicBool::new(false)),
            spec: None,
        });
        id
    }

    #[test]
    fn a_download_fills_the_local_row_and_spins_the_remote_one() {
        let mut app = test_app();
        push_transfer(&mut app, JobKind::Download, "/tmp/dl/report.csv", "data/report.csv", 400);

        assert_eq!(
            app.local_row_transfer(Path::new("/tmp/dl/report.csv")),
            Some(RowTransfer::Progress(0.25)),
            "the bytes are landing locally, so the local row is the one that fills"
        );
        assert_eq!(
            app.remote_row_transfer("data/report.csv"),
            Some(RowTransfer::Active),
            "the object being read isn't growing, so it only spins"
        );
    }

    #[test]
    fn an_upload_is_the_mirror_image() {
        let mut app = test_app();
        push_transfer(&mut app, JobKind::Upload, "/tmp/dl/report.csv", "data/report.csv", 400);

        assert_eq!(app.remote_row_transfer("data/report.csv"), Some(RowTransfer::Progress(0.25)));
        assert_eq!(app.local_row_transfer(Path::new("/tmp/dl/report.csv")), Some(RowTransfer::Active));
    }

    #[test]
    fn a_destination_whose_size_is_not_known_yet_spins_rather_than_showing_an_empty_bar() {
        let mut app = test_app();
        push_transfer(&mut app, JobKind::Download, "/tmp/dl/big.bin", "data/big.bin", 0);
        assert_eq!(app.local_row_transfer(Path::new("/tmp/dl/big.bin")), Some(RowTransfer::Active));
    }

    #[test]
    fn a_delete_spins_its_prefix_and_fills_nothing() {
        let mut app = test_app();
        push_transfer(&mut app, JobKind::RemoteDelete, "", "logs/2026/", 0);
        assert_eq!(app.remote_row_transfer("logs/2026/"), Some(RowTransfer::Active));
    }

    #[test]
    fn rows_untouched_by_any_job_show_nothing() {
        let mut app = test_app();
        push_transfer(&mut app, JobKind::Download, "/tmp/dl/report.csv", "data/report.csv", 400);
        assert_eq!(app.local_row_transfer(Path::new("/tmp/dl/other.csv")), None);
        assert_eq!(app.remote_row_transfer("data/other.csv"), None);
    }

    #[test]
    fn a_download_landing_here_gets_an_incoming_row_until_it_joins_the_listing() {
        let mut app = test_app();
        app.local_cwd = PathBuf::from("/tmp/dl");
        push_transfer(&mut app, JobKind::Download, "/tmp/dl/report.csv", "data/report.csv", 400);

        assert_eq!(app.incoming_local_rows(), vec![("report.csv".to_string(), Some(0.25))]);

        // Once the post-`Done` refresh picks the file up, it renders as itself instead.
        app.local_entries.push(crate::local::LocalEntry {
            path: PathBuf::from("/tmp/dl/report.csv"),
            name: "report.csv".into(),
            is_dir: false,
            size: 400,
            modified: None,
        });
        assert!(app.incoming_local_rows().is_empty());
    }

    #[test]
    fn a_download_landing_elsewhere_gets_no_incoming_row() {
        let mut app = test_app();
        app.local_cwd = PathBuf::from("/tmp/dl");
        push_transfer(&mut app, JobKind::Download, "/tmp/somewhere-else/report.csv", "data/report.csv", 400);
        assert!(app.incoming_local_rows().is_empty(), "only this directory's arrivals belong in this pane");
    }

    #[test]
    fn an_upload_never_gets_an_incoming_row_for_its_own_source() {
        let mut app = test_app();
        app.local_cwd = PathBuf::from("/tmp/dl");
        push_transfer(&mut app, JobKind::Upload, "/tmp/dl/report.csv", "data/report.csv", 400);
        assert!(app.incoming_local_rows().is_empty(), "the source file is already in the listing");
    }

    #[test]
    fn retrying_a_running_job_is_refused() {
        let mut app = test_app();
        let spec = JobSpec::Upload {
            bucket: "b".into(),
            local_path: PathBuf::from("/tmp/x"),
            key_prefix: String::new(),
        };
        push_job(&mut app, JobStatus::Running, Some(spec));
        app.retry_selected_job();
        assert!(app.status.as_ref().is_some_and(|(_, is_err)| *is_err));
    }
}
