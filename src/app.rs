use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::config::Connection;
use crate::jobs::{self, Job, JobId, JobKind, JobStatus, ProgressMsg};
use crate::local::{self, LocalEntry};
use crate::s3::{Entry, S3Client};

/// Bytes read for a preview snippet — small enough to be cheap even over the network for a
/// remote object, large enough to show a meaningful chunk of a text/JSON/YAML file.
const PREVIEW_BYTES: u64 = 4096;

/// Above this, skip fetching a preview entirely (even the bounded read above isn't free once
/// you count network/head latency) and just say the file's too large.
const MAX_PREVIEW_SIZE: u64 = 5 * 1024 * 1024;

/// `(label, secret, optional)` for each field collected by the add/edit bookmark wizard, in
/// the order they're asked.
pub const BOOKMARK_FIELDS: [(&str, bool, bool); 8] = [
    ("protocol (s3 / s3_private_link, default: s3)", false, true),
    ("name", false, false),
    ("server", false, false),
    ("access_key_id", false, false),
    ("secret_access_key", true, false),
    ("path (bucket or bucket/prefix)", false, false),
    ("web_url (optional)", false, true),
    ("region (optional)", false, true),
];

pub struct BookmarkWizard {
    /// `Some(path)` when editing an existing bookmark file (overwrite it); `None` when
    /// creating a new one (a filename is derived from the name field).
    pub editing_path: Option<String>,
    pub values: Vec<String>,
    pub field_index: usize,
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

#[derive(Clone)]
pub enum Preview {
    Empty,
    Loading,
    Directory,
    Text { text: String, size: u64, truncated: bool },
    Binary { size: u64 },
    TooLarge { size: u64 },
    Error(String),
}

pub struct App {
    pub screen: Screen,
    pub connections: Vec<(String, Connection)>,
    pub conn_selected: usize,

    pub client: Option<S3Client>,
    pub connection: Option<Connection>,
    pub bucket: String,
    pub buckets: Vec<String>,
    pub bucket_selected: usize,

    // Remote (S3) pane.
    pub prefix: String,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub marked: HashSet<String>,
    pub filter: Option<String>,
    /// Scroll/selection state for the remote list widget — kept on `App` (rather than
    /// recreated per frame) so the viewport offset persists across redraws instead of
    /// snapping back to the top every time.
    pub list_state: ListState,

    // Local filesystem pane.
    pub local_cwd: PathBuf,
    pub local_entries: Vec<LocalEntry>,
    pub local_cursor: usize,
    pub local_marked: HashSet<PathBuf>,
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
    pub show_diagnostics: bool,
    pub loading: bool,
    pub status: Option<(String, bool)>,
    /// Full multi-line detail behind the last error, shown in the diagnostics popup — the
    /// one-line `status` message alone is often not enough to debug a connection problem.
    pub last_error: Option<String>,

    pub bookmark_wizard: Option<BookmarkWizard>,
    /// File path of the bookmark pending a delete confirmation.
    pub confirm_bookmark_delete: Option<String>,

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
            list_state: ListState::default(),
            local_cwd: local::default_download_dir(),
            local_entries: Vec::new(),
            local_cursor: 0,
            local_marked: HashSet::new(),
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
            show_diagnostics: false,
            loading: false,
            status: None,
            last_error: None,
            bookmark_wizard: None,
            confirm_bookmark_delete: None,
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
    /// chain (shown in the `e` diagnostics popup), plus whatever connection context we have.
    pub fn set_error(&mut self, context: &str, err: &anyhow::Error) {
        self.status = Some((format!("{context}: {err}"), true));
        let mut detail = format!("{context}:\n");
        for (i, cause) in err.chain().enumerate() {
            detail.push_str(&format!("  {i}: {cause}\n"));
        }
        if let Some(client) = &self.client {
            detail.push_str("\nconnection:\n");
            for line in &client.diagnostics {
                detail.push_str(&format!("  {line}\n"));
            }
        }
        self.last_error = Some(detail);
    }

    pub fn visible_entries(&self) -> Vec<&Entry> {
        match &self.filter {
            Some(f) if !f.is_empty() => self
                .entries
                .iter()
                .filter(|e| e.name.to_lowercase().contains(&f.to_lowercase()))
                .collect(),
            _ => self.entries.iter().collect(),
        }
    }

    pub async fn connect(&mut self, index: usize) -> Result<()> {
        let conn = self.connections[index].1.clone();
        self.loading = true;
        let client = match S3Client::connect(&conn).await {
            Ok(c) => c,
            Err(err) => {
                self.loading = false;
                self.set_error("connect failed", &err);
                return Ok(());
            }
        };
        self.connection = Some(conn.clone());
        let (bookmark_bucket, bookmark_prefix) = conn.bucket_and_prefix();

        match client.list_buckets().await {
            Ok(buckets) if !buckets.is_empty() => {
                self.bucket_selected = buckets.iter().position(|b| b == &bookmark_bucket).unwrap_or(0);
                self.buckets = buckets;
                self.client = Some(client);
                self.screen = Screen::BucketPicker;
            }
            _ => {
                // No `s3:ListAllMyBuckets` permission (common with a scoped-down policy) or
                // the account genuinely has none visible — fall back to the bucket pinned in
                // the bookmark's `path`, exactly like before bucket-browsing existed.
                self.bucket = bookmark_bucket;
                self.prefix = bookmark_prefix;
                self.client = Some(client);
                self.screen = Screen::Browser;
                self.enter_browser().await?;
            }
        }
        self.loading = false;
        Ok(())
    }

    pub async fn pick_bucket(&mut self, index: usize) -> Result<()> {
        let Some(bucket) = self.buckets.get(index).cloned() else {
            return Ok(());
        };
        let bookmark_prefix = self.connection.as_ref().map(|c| c.bucket_and_prefix().1).unwrap_or_default();
        let bookmark_bucket = self.connection.as_ref().map(|c| c.bucket_and_prefix().0).unwrap_or_default();
        self.prefix = if bucket == bookmark_bucket { bookmark_prefix } else { String::new() };
        self.bucket = bucket;
        self.screen = Screen::Browser;
        self.enter_browser().await
    }

    async fn enter_browser(&mut self) -> Result<()> {
        self.cursor = 0;
        self.marked.clear();
        self.filter = None;
        self.refresh_local();
        self.refresh().await?;
        self.refresh_preview();
        Ok(())
    }

    pub async fn refresh(&mut self) -> Result<()> {
        let Some(client) = &self.client else {
            return Ok(());
        };
        self.loading = true;
        let result = client.list(&self.bucket, &self.prefix).await;
        self.loading = false;
        match result {
            Ok(entries) => {
                self.entries = entries;
                if self.cursor >= self.entries.len() {
                    self.cursor = self.entries.len().saturating_sub(1);
                }
            }
            Err(err) => self.set_error("list failed", &err),
        }
        Ok(())
    }

    pub fn refresh_local(&mut self) {
        // `~/Downloads` not existing (rare, but possible on a minimal Linux setup) shouldn't
        // surface a scary error the moment you connect — silently fall back to `$HOME` instead.
        if !self.local_cwd.is_dir() {
            self.local_cwd = local::home_dir();
        }
        match local::list_local(&self.local_cwd) {
            Ok(entries) => {
                self.local_entries = entries;
                if self.local_cursor >= self.local_entries.len() {
                    self.local_cursor = self.local_entries.len().saturating_sub(1);
                }
            }
            Err(err) => self.set_status(format!("local list failed: {err}"), true),
        }
    }

    pub fn current_entry(&self) -> Option<&Entry> {
        self.visible_entries().get(self.cursor).copied()
    }

    pub fn current_local_entry(&self) -> Option<&LocalEntry> {
        self.local_entries.get(self.local_cursor)
    }

    /// The job under the transfers pane cursor, in the same newest-first order it's rendered.
    pub fn current_job(&self) -> Option<&Job> {
        self.jobs.iter().rev().nth(self.jobs_cursor)
    }

    /// Opens the selected transfer's local file/directory with the OS default app — the
    /// download destination for a download/zip job, or the source file for an upload.
    pub fn open_selected_job(&mut self) {
        let Some((path, done)) =
            self.current_job().map(|j| (j.local_path.clone(), matches!(j.status, JobStatus::Done)))
        else {
            return;
        };
        if !done {
            self.set_status("transfer hasn't finished yet", true);
            return;
        }
        if let Err(err) = open::that(&path) {
            self.set_status(format!("failed to open {}: {err}", path.display()), true);
        }
    }

    /// Reveals the selected transfer's local file in Finder, highlighted — the usual "where
    /// did that end up" fix without having to hunt for it in the local pane.
    pub fn reveal_selected_job_in_finder(&mut self) {
        let Some((path, done)) =
            self.current_job().map(|j| (j.local_path.clone(), matches!(j.status, JobStatus::Done)))
        else {
            return;
        };
        if !done {
            self.set_status("transfer hasn't finished yet", true);
            return;
        }
        match std::process::Command::new("open").arg("-R").arg(&path).status() {
            Ok(status) if status.success() => {}
            Ok(status) => self.set_status(format!("finder reveal exited with {status}"), true),
            Err(err) => self.set_status(format!("failed to reveal {}: {err}", path.display()), true),
        }
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

    pub async fn enter_selected(&mut self) -> Result<()> {
        match self.focus {
            Focus::Remote => {
                let Some(entry) = self.current_entry().cloned() else { return Ok(()) };
                if entry.is_dir {
                    self.prefix = entry.key.clone();
                    self.cursor = 0;
                    self.filter = None;
                    self.refresh().await?;
                }
            }
            Focus::Local => {
                let Some(entry) = self.current_local_entry().cloned() else { return Ok(()) };
                if entry.is_dir {
                    self.local_cwd = entry.path.clone();
                    self.local_cursor = 0;
                    self.refresh_local();
                }
            }
            Focus::Preview => {}
            Focus::Transfers => self.open_selected_job(),
        }
        self.refresh_preview();
        Ok(())
    }

    pub async fn go_up(&mut self) -> Result<()> {
        match self.focus {
            Focus::Remote => {
                if self.prefix.is_empty() {
                    return Ok(());
                }
                let trimmed = self.prefix.trim_end_matches('/');
                let parent = match trimmed.rsplit_once('/') {
                    Some((p, _)) => format!("{p}/"),
                    None => String::new(),
                };
                self.prefix = parent;
                self.cursor = 0;
                self.filter = None;
                self.refresh().await?;
            }
            Focus::Local => {
                if let Some(parent) = self.local_cwd.parent() {
                    self.local_cwd = parent.to_path_buf();
                    self.local_cursor = 0;
                    self.refresh_local();
                }
            }
            Focus::Preview => {}
            Focus::Transfers => {}
        }
        self.refresh_preview();
        Ok(())
    }

    pub fn move_cursor(&mut self, delta: i32) {
        match self.focus {
            Focus::Remote => {
                let len = self.visible_entries().len();
                if len == 0 {
                    return;
                }
                let next = self.cursor as i32 + delta;
                self.cursor = next.clamp(0, len as i32 - 1) as usize;
            }
            Focus::Preview => {}
            Focus::Transfers => {
                let len = self.jobs.len();
                if len == 0 {
                    return;
                }
                let next = self.jobs_cursor as i32 + delta;
                self.jobs_cursor = next.clamp(0, len as i32 - 1) as usize;
            }
            Focus::Local => {
                let len = self.local_entries.len();
                if len == 0 {
                    return;
                }
                let next = self.local_cursor as i32 + delta;
                self.local_cursor = next.clamp(0, len as i32 - 1) as usize;
            }
        }
    }

    pub fn toggle_mark(&mut self) {
        match self.focus {
            Focus::Remote => {
                if let Some(entry) = self.current_entry().cloned() {
                    if !self.marked.remove(&entry.key) {
                        self.marked.insert(entry.key);
                    }
                }
            }
            Focus::Local => {
                if let Some(entry) = self.current_local_entry().cloned() {
                    if !self.local_marked.remove(&entry.path) {
                        self.local_marked.insert(entry.path);
                    }
                }
            }
            Focus::Preview => {}
            Focus::Transfers => {}
        }
    }

    fn next_id(&mut self) -> JobId {
        let id = self.next_job_id;
        self.next_job_id += 1;
        id
    }

    /// Downloads marked (or the currently hovered) remote objects into the local pane's
    /// current directory — the whole point of browsing both sides at once.
    pub async fn start_download_selected(&mut self) -> Result<()> {
        let Some(client) = self.client.clone() else {
            return Ok(());
        };
        let dest_dir = self.local_cwd.clone();

        if !self.marked.is_empty() {
            let keys: Vec<String> = self.marked.iter().cloned().collect();
            let mut all_files = Vec::new();
            for key in &keys {
                if key.ends_with('/') {
                    all_files.extend(client.list_all_under(&self.bucket, key).await?);
                } else if let Some(e) = self.entries.iter().find(|e| &e.key == key) {
                    all_files.push(e.clone());
                }
            }
            let id = self.next_id();
            let zip_name = format!("comhad-selection-{id}.zip");
            self.jobs.push(Job {
                id,
                label: zip_name.clone(),
                kind: JobKind::Zip,
                total_bytes: 0,
                done_bytes: 0,
                status: JobStatus::Running,
                local_path: dest_dir.join(&zip_name),
            });
            jobs::spawn_zip_download(
                client,
                id,
                self.bucket.clone(),
                all_files,
                self.prefix.clone(),
                zip_name,
                dest_dir,
                self.job_tx.clone(),
            );
            self.marked.clear();
            self.set_status("zipping selection...", false);
            return Ok(());
        }

        let Some(entry) = self.current_entry().cloned() else {
            return Ok(());
        };

        if entry.is_dir {
            let all_files = client.list_all_under(&self.bucket, &entry.key).await?;
            let id = self.next_id();
            let zip_name = format!("{}.zip", entry.name);
            self.jobs.push(Job {
                id,
                label: zip_name.clone(),
                kind: JobKind::Zip,
                total_bytes: 0,
                done_bytes: 0,
                status: JobStatus::Running,
                local_path: dest_dir.join(&zip_name),
            });
            jobs::spawn_zip_download(
                client,
                id,
                self.bucket.clone(),
                all_files,
                entry.key.clone(),
                zip_name,
                dest_dir,
                self.job_tx.clone(),
            );
        } else {
            let id = self.next_id();
            self.jobs.push(Job {
                id,
                label: entry.name.clone(),
                kind: JobKind::Download,
                total_bytes: 0,
                done_bytes: 0,
                status: JobStatus::Running,
                local_path: dest_dir.join(&entry.name),
            });
            jobs::spawn_download_object(
                client,
                id,
                self.bucket.clone(),
                entry.key.clone(),
                entry.name.clone(),
                dest_dir,
                self.job_tx.clone(),
            );
        }
        self.set_status("download started", false);
        Ok(())
    }

    /// Uploads marked (or the currently hovered) local files/directories into the remote
    /// pane's current prefix.
    pub fn start_upload_selected(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let paths: Vec<PathBuf> = if !self.local_marked.is_empty() {
            self.local_marked.iter().cloned().collect()
        } else if let Some(entry) = self.current_local_entry() {
            vec![entry.path.clone()]
        } else {
            Vec::new()
        };
        if paths.is_empty() {
            self.set_status("nothing selected to upload", true);
            return;
        }
        for path in paths {
            let id = self.next_id();
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            self.jobs.push(Job {
                id,
                label,
                kind: JobKind::Upload,
                total_bytes: 0,
                done_bytes: 0,
                status: JobStatus::Running,
                local_path: path.clone(),
            });
            jobs::spawn_upload(
                client.clone(),
                id,
                self.bucket.clone(),
                path,
                self.prefix.clone(),
                self.job_tx.clone(),
            );
        }
        self.local_marked.clear();
        self.set_status("upload started", false);
    }

    /// Uploads a single dropped-from-Finder path into the remote pane's current prefix.
    pub fn start_upload_path(&mut self, local_path: PathBuf) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let id = self.next_id();
        let label = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.display().to_string());
        self.jobs.push(Job {
            id,
            label,
            kind: JobKind::Upload,
            total_bytes: 0,
            done_bytes: 0,
            status: JobStatus::Running,
            local_path: local_path.clone(),
        });
        jobs::spawn_upload(client, id, self.bucket.clone(), local_path, self.prefix.clone(), self.job_tx.clone());
        self.set_status("upload started", false);
    }

    pub async fn rename_to(&mut self, new_name: String) -> Result<()> {
        match self.focus {
            Focus::Remote => {
                let Some(client) = self.client.clone() else { return Ok(()) };
                let Some(entry) = self.current_entry().cloned() else { return Ok(()) };
                let parent = entry
                    .key
                    .strip_suffix(&format!("{}{}", entry.name, if entry.is_dir { "/" } else { "" }))
                    .unwrap_or("")
                    .to_string();
                let result = if entry.is_dir {
                    let new_key = format!("{parent}{new_name}/");
                    client.rename_prefix(&self.bucket, &entry.key, &new_key).await
                } else {
                    let new_key = format!("{parent}{new_name}");
                    client.rename_object(&self.bucket, &entry.key, &new_key).await
                };
                match result {
                    Ok(()) => self.set_status("renamed", false),
                    Err(err) => self.set_error("rename failed", &err),
                }
                self.refresh().await?;
            }
            Focus::Local => {
                let Some(entry) = self.current_local_entry().cloned() else { return Ok(()) };
                let new_path = self.local_cwd.join(&new_name);
                match std::fs::rename(&entry.path, &new_path) {
                    Ok(()) => self.set_status("renamed", false),
                    Err(err) => self.set_status(format!("rename failed: {err}"), true),
                }
                self.refresh_local();
            }
            Focus::Preview => {}
            Focus::Transfers => {}
        }
        self.refresh_preview();
        Ok(())
    }

    pub fn start_add_bookmark(&mut self) {
        self.bookmark_wizard = Some(BookmarkWizard {
            editing_path: None,
            values: vec![String::new(); BOOKMARK_FIELDS.len()],
            field_index: 0,
        });
        self.open_bookmark_field_prompt();
    }

    pub fn start_edit_bookmark(&mut self, index: usize) {
        let Some((path, conn)) = self.connections.get(index).cloned() else {
            return;
        };
        // protocol, name, server, access_key_id, secret_access_key, path, web_url, region
        let values = vec![
            conn.protocol.clone().unwrap_or_default(),
            conn.name.clone(),
            conn.server.clone(),
            conn.access_key_id.clone(),
            String::new(), // secret left blank; blank on save means "keep the existing one"
            conn.path.clone(),
            conn.web_url.clone().unwrap_or_default(),
            conn.region.clone().unwrap_or_default(),
        ];
        self.bookmark_wizard = Some(BookmarkWizard { editing_path: Some(path), values, field_index: 0 });
        self.open_bookmark_field_prompt();
    }

    fn open_bookmark_field_prompt(&mut self) {
        let Some(wizard) = &self.bookmark_wizard else { return };
        let (_, secret, _) = BOOKMARK_FIELDS[wizard.field_index];
        let buffer = wizard.values[wizard.field_index].clone();
        self.prompt = Some(Prompt { kind: PromptKind::BookmarkField, cursor: buffer.len(), buffer, mask: secret });
    }

    /// Called when the user presses enter on the current wizard field's prompt. Advances to
    /// the next field, or writes the bookmark file out once the last field is submitted.
    pub fn submit_bookmark_field(&mut self, value: String) {
        let Some(wizard) = &mut self.bookmark_wizard else { return };
        wizard.values[wizard.field_index] = value;
        wizard.field_index += 1;
        if wizard.field_index < wizard.values.len() {
            self.open_bookmark_field_prompt();
        } else {
            self.save_bookmark();
        }
    }

    fn save_bookmark(&mut self) {
        let Some(wizard) = self.bookmark_wizard.take() else { return };
        let [protocol, name, server, access_key_id, secret_access_key, path, web_url, region] =
            match <[String; 8]>::try_from(wizard.values) {
                Ok(v) => v,
                Err(_) => return,
            };

        let secret_access_key = if secret_access_key.is_empty() {
            match &wizard.editing_path {
                Some(path) => self
                    .connections
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, c)| c.secret_access_key.clone())
                    .unwrap_or_default(),
                None => String::new(),
            }
        } else {
            secret_access_key
        };

        let conn = Connection {
            name,
            server,
            access_key_id,
            secret_access_key,
            path,
            web_url: if web_url.is_empty() { None } else { Some(web_url) },
            region: if region.is_empty() { None } else { Some(region) },
            protocol: if protocol.is_empty() { None } else { Some(protocol) },
            force_path_style: None,
        };

        let target_path = match &wizard.editing_path {
            Some(p) => PathBuf::from(p),
            None => match crate::config::config_dir() {
                Ok(dir) => dir.join(format!("{}.json", slugify(&conn.name))),
                Err(err) => {
                    self.set_status(format!("failed to save bookmark: {err}"), true);
                    return;
                }
            },
        };

        match crate::config::write_bookmark(&target_path, &conn) {
            Ok(()) => {
                let path_str = target_path.display().to_string();
                if let Some(existing) = self.connections.iter_mut().find(|(p, _)| p == &path_str) {
                    existing.1 = conn;
                } else {
                    self.connections.push((path_str, conn));
                    self.connections.sort_by(|a, b| a.0.cmp(&b.0));
                }
                self.set_status("bookmark saved", false);
            }
            Err(err) => self.set_status(format!("failed to save bookmark: {err}"), true),
        }
    }

    pub fn start_delete_bookmark(&mut self, index: usize) {
        if let Some((path, _)) = self.connections.get(index) {
            self.confirm_bookmark_delete = Some(path.clone());
        }
    }

    pub fn cancel_delete_bookmark(&mut self) {
        self.confirm_bookmark_delete = None;
    }

    pub fn confirm_delete_bookmark_now(&mut self) {
        let Some(path) = self.confirm_bookmark_delete.take() else { return };
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.connections.retain(|(p, _)| p != &path);
                if self.conn_selected >= self.connections.len() {
                    self.conn_selected = self.connections.len().saturating_sub(1);
                }
                self.set_status("bookmark deleted", false);
            }
            Err(err) => self.set_status(format!("failed to delete bookmark: {err}"), true),
        }
    }

    /// Recomputes the preview pane for whichever pane currently has focus.
    ///
    /// Synchronous and non-blocking: local reads are fast enough to do inline, but a remote
    /// object needs a network round trip, so that case spawns a background task and returns
    /// immediately (showing `Preview::Loading` in the meantime) rather than freezing the UI
    /// on every arrow key press — the previous version awaited the fetch right in the key
    /// handler, so scrolling past objects over a slower connection stalled the whole redraw
    /// loop. `preview_generation` tags each request so a slow response for an object you've
    /// since scrolled past is silently dropped instead of clobbering a newer preview.
    pub fn refresh_preview(&mut self) {
        // Tabbing focus into the preview or transfers pane doesn't change what's being
        // previewed, so leave whatever content and scroll position are already there alone.
        if matches!(self.focus, Focus::Preview | Focus::Transfers) {
            return;
        }

        self.preview_generation += 1;
        let generation = self.preview_generation;
        self.preview_scroll = 0;

        if !self.show_preview {
            self.preview = Preview::Empty;
            return;
        }

        self.preview = match self.focus {
            Focus::Remote => match self.current_entry().cloned() {
                None => Preview::Empty,
                Some(entry) if entry.is_dir => Preview::Directory,
                Some(entry) if entry.size.max(0) as u64 > MAX_PREVIEW_SIZE => {
                    Preview::TooLarge { size: entry.size.max(0) as u64 }
                }
                Some(entry) => match &self.client {
                    None => Preview::Empty,
                    Some(client) => {
                        let client = client.clone();
                        let bucket = self.bucket.clone();
                        let tx = self.preview_tx.clone();
                        tokio::spawn(async move {
                            let preview = match client.read_preview(&bucket, &entry.key, PREVIEW_BYTES).await {
                                Ok(bytes) => classify_bytes(bytes, entry.size.max(0) as u64),
                                Err(err) => Preview::Error(err.to_string()),
                            };
                            let _ = tx.send((generation, preview));
                        });
                        Preview::Loading
                    }
                },
            },
            Focus::Local => match self.current_local_entry().cloned() {
                None => Preview::Empty,
                Some(entry) if entry.is_dir => Preview::Directory,
                Some(entry) if entry.size > MAX_PREVIEW_SIZE => Preview::TooLarge { size: entry.size },
                Some(entry) => match read_local_prefix(&entry.path, PREVIEW_BYTES) {
                    Ok((bytes, truncated)) => {
                        let mut preview = classify_bytes(bytes, entry.size);
                        if let Preview::Text { truncated: t, .. } = &mut preview {
                            *t = truncated;
                        }
                        preview
                    }
                    Err(err) => Preview::Error(err.to_string()),
                },
            },
            // Unreachable: handled by the early return above.
            Focus::Preview | Focus::Transfers => Preview::Empty,
        };
    }

    /// Applies any preview fetched by a background task started by `refresh_preview`,
    /// dropping it if it's for a stale request (the cursor has since moved on).
    pub fn drain_preview_messages(&mut self) {
        while let Ok((generation, preview)) = self.preview_rx.try_recv() {
            if generation == self.preview_generation {
                self.preview = preview;
            }
        }
    }

    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
        if !self.show_preview && self.focus == Focus::Preview {
            self.focus = Focus::Remote;
        }
    }

    pub fn toggle_local(&mut self) {
        self.show_local = !self.show_local;
        if !self.show_local && self.focus == Focus::Local {
            self.focus = Focus::Remote;
        }
    }

    /// Scrolls the preview pane by `delta` lines, clamped to the text's line count. A no-op
    /// for previews with nothing to scroll (directories, binaries, loading, etc).
    pub fn scroll_preview(&mut self, delta: i32) {
        let Preview::Text { text, .. } = &self.preview else {
            return;
        };
        let max_scroll = text.lines().count().saturating_sub(1) as i32;
        let next = self.preview_scroll as i32 + delta;
        self.preview_scroll = next.clamp(0, max_scroll) as u16;
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

/// Turns a bookmark name into a safe filename stem, e.g. `"HELLO world!"` -> `"hello_world"`.
fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = slug.trim_matches('_');
    if trimmed.is_empty() { "bookmark".to_string() } else { trimmed.to_string() }
}

/// Reads at most `max_bytes` from the start of a local file without loading the whole thing
/// into memory first — important once `MAX_PREVIEW_SIZE` no longer catches every large file
/// on a slow filesystem (network mounts, etc).
fn read_local_prefix(path: &std::path::Path, max_bytes: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; max_bytes as usize];
    let mut total = 0;
    loop {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
        if total == buf.len() {
            break;
        }
    }
    buf.truncate(total);
    let truncated = file.read(&mut [0u8; 1])? > 0;
    Ok((buf, truncated))
}

/// Best-effort text/binary classification of a preview snippet.
fn classify_bytes(bytes: Vec<u8>, total_size: u64) -> Preview {
    if bytes.is_empty() {
        return Preview::Text { text: String::new(), size: total_size, truncated: false };
    }
    let sample_len = bytes.len().min(512);
    let non_printable = bytes[..sample_len]
        .iter()
        .filter(|&&b| b != b'\n' && b != b'\r' && b != b'\t' && (b < 0x20 || b == 0x7f))
        .count();
    if non_printable * 20 > sample_len {
        return Preview::Binary { size: total_size };
    }
    let truncated = (bytes.len() as u64) < total_size;
    Preview::Text { text: String::from_utf8_lossy(&bytes).to_string(), size: total_size, truncated }
}
