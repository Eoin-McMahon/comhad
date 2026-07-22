//! Browsing, navigation, and transfer orchestration for [`App`].

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;

use super::{App, Focus, Preview, Screen};
use crate::fuzzy::fuzzy_matches;
use crate::jobs::{self, Job, JobId, JobKind, JobStatus};
use crate::local::{self, LocalEntry};
use crate::provider::{self, RemoteEntry};

impl App {
    /// The remote entries currently shown, after applying the active filter and sort.
    pub fn visible_entries(&self) -> Vec<&RemoteEntry> {
        let mut out: Vec<&RemoteEntry> = match &self.filter {
            Some(f) if !f.is_empty() => self.entries.iter().filter(|e| fuzzy_matches(&e.name, f)).collect(),
            _ => self.entries.iter().collect(),
        };
        super::sort::sort_entries(&mut out, self.remote_sort);
        out
    }

    /// The local entries currently shown, after applying the active local filter and sort.
    pub fn visible_local_entries(&self) -> Vec<&LocalEntry> {
        let mut out: Vec<&LocalEntry> = match &self.local_filter {
            Some(f) if !f.is_empty() => self.local_entries.iter().filter(|e| fuzzy_matches(&e.name, f)).collect(),
            _ => self.local_entries.iter().collect(),
        };
        super::sort::sort_entries(&mut out, self.local_sort);
        out
    }

    /// The active filter string for the focused pane (local vs remote), if any.
    pub fn active_filter(&self) -> Option<String> {
        match self.focus {
            Focus::Local => self.local_filter.clone(),
            _ => self.filter.clone(),
        }
    }

    /// Sets (or clears) the filter for the focused pane, then re-evaluates `/`'s deep
    /// fallback (see [`super::deep`]) against the new value.
    pub async fn set_filter(&mut self, value: Option<String>) {
        match self.focus {
            Focus::Local => self.local_filter = value,
            _ => self.filter = value,
        }
        self.update_deep_matches().await;
    }

    pub async fn connect(&mut self, index: usize) -> Result<()> {
        let conn = self.connections[index].1.clone();
        self.loading = true;
        let client = match provider::connect(&conn).await {
            Ok(c) => c,
            Err(err) => {
                self.loading = false;
                // Don't leave a stale bookmark's session state on screen after a failed connect.
                self.reset_session();
                self.set_error("connect failed", &err);
                return Ok(());
            }
        };
        self.connection = Some(conn.clone());
        self.apply_local_start_dir(&conn);
        let (bookmark_bucket, bookmark_prefix) = conn.bucket_and_prefix();

        match client.list_containers().await {
            Ok(buckets) if !buckets.is_empty() => {
                self.bucket_selected = buckets.iter().position(|b| b == &bookmark_bucket).unwrap_or(0);
                self.buckets = buckets;
                self.client = Some(client);
                self.screen = Screen::BucketPicker;
            }
            _ => {
                // No list-buckets permission or none visible — fall back to the bookmark's pinned bucket.
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

    /// Moves the local pane to the directory this bookmark is paired with. Called on every
    /// connect, so switching bookmarks with `c` re-pairs the local side too — connecting to a
    /// different bucket is a context switch, and leaving the local pane behind on the previous
    /// bookmark's directory would make `s` diff two unrelated trees.
    fn apply_local_start_dir(&mut self, conn: &crate::config::Connection) {
        let (dir, warning) =
            local::resolve_start_dir(conn.local_path.as_deref(), self.local_dir_config.as_deref());

        if dir != self.local_cwd {
            self.local_cwd = dir;
            self.local_cursor = 0;
            self.local_marked.clear();
            self.local_filter = None;
        }
        if let Some(warning) = warning {
            self.set_status(warning, true);
        }
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

    /// Clears every piece of state tied to a specific bookmark/bucket/prefix.
    pub(super) fn reset_session(&mut self) {
        self.client = None;
        self.connection = None;
        self.buckets.clear();
        self.bucket_selected = 0;
        self.bucket.clear();
        self.prefix.clear();
        self.entries.clear();
        self.cursor = 0;
        self.marked.clear();
        self.filter = None;
        self.clear_deep_matches();
        self.sync = None;
        self.clip = None;
        self.preview = Preview::Empty;
    }

    async fn enter_browser(&mut self) -> Result<()> {
        self.cursor = 0;
        self.marked.clear();
        self.filter = None;
        self.clear_deep_matches();
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
            Err(err) => {
                // Show an empty listing rather than a stale one under the new bucket's name.
                self.entries.clear();
                self.cursor = 0;
                self.set_error("list failed", &err);
            }
        }
        Ok(())
    }

    pub fn refresh_local(&mut self) {
        // Fall back to $HOME if the configured local dir doesn't exist.
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

    /// Opens the selected transfer's local file/directory with the OS default app.
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

    /// Reveals the selected transfer's local file in Finder, highlighted.
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

    pub async fn enter_selected(&mut self) -> Result<()> {
        match self.focus {
            Focus::Remote => {
                // Past the listing means the hovered row is one of `/`'s deep extra matches.
                if self.cursor >= self.visible_entries().len() {
                    return self.jump_to_deep_remote().await;
                }
                let Some(entry) = self.current_entry().cloned() else { return Ok(()) };
                if entry.is_dir {
                    self.prefix = entry.key.clone();
                    self.cursor = 0;
                    self.filter = None;
                    self.clear_deep_matches();
                    self.refresh().await?;
                }
            }
            Focus::Local => {
                if self.local_cursor >= self.visible_local_entries().len() {
                    self.jump_to_deep_local();
                    self.refresh_preview();
                    return Ok(());
                }
                let Some(entry) = self.current_local_entry().cloned() else { return Ok(()) };
                if entry.is_dir {
                    self.local_cwd = entry.path.clone();
                    self.local_cursor = 0;
                    self.local_filter = None;
                    self.clear_deep_matches();
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
                self.clear_deep_matches();
                self.refresh().await?;
            }
            Focus::Local => {
                if let Some(parent) = self.local_cwd.parent() {
                    self.local_cwd = parent.to_path_buf();
                    self.local_cursor = 0;
                    self.local_filter = None;
                    self.clear_deep_matches();
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
                let len = self.visible_entries().len() + self.deep_remote.as_ref().map(|d| d.extra.len()).unwrap_or(0);
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
                let len =
                    self.visible_local_entries().len() + self.deep_local.as_ref().map(|d| d.extra.len()).unwrap_or(0);
                if len == 0 {
                    return;
                }
                let next = self.local_cursor as i32 + delta;
                self.local_cursor = next.clamp(0, len as i32 - 1) as usize;
            }
        }
        self.update_visual_selection();
    }

    /// Every key currently on screen in the remote pane, in display order (listing then deep extras).
    fn remote_row_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.visible_entries().iter().map(|e| e.key.clone()).collect();
        if let Some(deep) = &self.deep_remote {
            keys.extend(deep.extra.iter().map(|e| e.key.clone()));
        }
        keys
    }

    fn local_row_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.visible_local_entries().iter().map(|e| e.path.clone()).collect();
        if let Some(deep) = &self.deep_local {
            paths.extend(deep.extra.iter().map(|e| e.path.clone()));
        }
        paths
    }

    /// Vim-style visual mode: `v` anchors the current row; each cursor move re-marks the
    /// range between anchor and cursor, replacing the previous marks.
    pub fn toggle_visual_mode(&mut self) {
        if self.visual_anchor.take().is_some() {
            return;
        }
        let anchor = match self.focus {
            Focus::Remote => self.cursor,
            Focus::Local => self.local_cursor,
            Focus::Preview | Focus::Transfers => return,
        };
        self.visual_anchor = Some(anchor);
        self.update_visual_selection();
    }

    /// Recomputes the focused pane's marked set from `visual_anchor` to the cursor. No-op if
    /// visual mode isn't active.
    fn update_visual_selection(&mut self) {
        let Some(anchor) = self.visual_anchor else { return };
        match self.focus {
            Focus::Remote => {
                let (lo, hi) = (anchor.min(self.cursor), anchor.max(self.cursor));
                self.marked = self.remote_row_keys().into_iter().enumerate().filter(|(i, _)| (lo..=hi).contains(i)).map(|(_, k)| k).collect();
            }
            Focus::Local => {
                let (lo, hi) = (anchor.min(self.local_cursor), anchor.max(self.local_cursor));
                self.local_marked = self.local_row_paths().into_iter().enumerate().filter(|(i, _)| (lo..=hi).contains(i)).map(|(_, p)| p).collect();
            }
            Focus::Preview | Focus::Transfers => {}
        }
    }

    pub fn toggle_mark(&mut self) {
        self.visual_anchor = None;
        match self.focus {
            Focus::Remote => {
                if let Some(entry) = self.current_entry().cloned()
                    && !self.marked.remove(&entry.key) {
                        self.marked.insert(entry.key);
                    }
            }
            Focus::Local => {
                if let Some(entry) = self.current_local_entry().cloned()
                    && !self.local_marked.remove(&entry.path) {
                        self.local_marked.insert(entry.path);
                    }
            }
            Focus::Preview => {}
            Focus::Transfers => {}
        }
    }

    pub(super) fn next_id(&mut self) -> JobId {
        let id = self.next_job_id;
        self.next_job_id += 1;
        id
    }

    /// The single non-directory entry `d` would download directly (no zip): the resolved
    /// selection is exactly one item and it isn't a directory. Shared with
    /// `request_confirm_download` so the confirm prompt matches what actually happens.
    pub fn single_download_target(&self) -> Option<RemoteEntry> {
        let entry = if self.marked.is_empty() {
            self.current_entry().cloned()
        } else if self.marked.len() == 1 {
            let key = self.marked.iter().next().unwrap();
            self.entries
                .iter()
                .find(|e| &e.key == key)
                .or_else(|| self.deep_remote.as_ref().and_then(|d| d.all_entries().iter().find(|e| &e.key == key)))
                .cloned()
        } else {
            None
        }?;
        (!entry.is_dir).then_some(entry)
    }

    /// Downloads marked (or the currently hovered) remote objects into the local pane's
    /// current directory.
    pub async fn start_download_selected(&mut self) -> Result<()> {
        let Some(client) = self.client.clone() else {
            return Ok(());
        };
        let dest_dir = self.local_cwd.clone();

        if let Some(entry) = self.single_download_target() {
            self.marked.clear();
            self.visual_anchor = None;
            let id = self.next_id();
            self.jobs.push(Job {
                id,
                label: entry.name.clone(),
                kind: JobKind::Download,
                total_bytes: 0,
                done_bytes: 0,
                status: JobStatus::Running,
                cancel: None,
                local_path: dest_dir.join(&entry.name),
            });
            jobs::spawn_download_object(client, id, self.bucket.clone(), entry.key.clone(), entry.name.clone(), dest_dir, self.job_tx.clone());
            return Ok(());
        }

        if !self.marked.is_empty() {
            let keys: Vec<String> = self.marked.iter().cloned().collect();
            let mut all_files = Vec::new();
            for key in &keys {
                if key.ends_with('/') {
                    all_files.extend(client.list_all_under(&self.bucket, key).await?);
                } else if let Some(e) = self.entries.iter().find(|e| &e.key == key).or_else(|| {
                    // A marked key might be one of `/`'s deep extra matches instead.
                    self.deep_remote.as_ref().and_then(|d| d.all_entries().iter().find(|e| &e.key == key))
                }) {
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
                cancel: None,
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
            self.visual_anchor = None;
            self.set_status("zipping selection...", false);
            return Ok(());
        }

        // Only reachable with `marked` empty and a directory hovered.
        let Some(entry) = self.current_entry().cloned() else {
            return Ok(());
        };
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
            cancel: None,
            local_path: dest_dir.join(&zip_name),
        });
        jobs::spawn_zip_download(client, id, self.bucket.clone(), all_files, entry.key.clone(), zip_name, dest_dir, self.job_tx.clone());
        self.set_status("zipping directory...", false);
        Ok(())
    }

    /// Uploads marked (or the hovered) local files/directories into the remote pane's current prefix.
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
                cancel: None,
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
        self.visual_anchor = None;
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
            cancel: None,
            local_path: local_path.clone(),
        });
        jobs::spawn_upload(client, id, self.bucket.clone(), local_path, self.prefix.clone(), self.job_tx.clone());
        self.set_status("upload started", false);
    }

    /// Copies the hovered item's location to the OS clipboard — `s3://bucket/key` remote,
    /// absolute path local.
    pub fn copy_location_to_clipboard(&mut self) {
        let text = match self.focus {
            Focus::Remote => self.current_entry().map(|e| format!("s3://{}/{}", self.bucket, e.key)),
            Focus::Local => self.current_local_entry().map(|e| e.path.display().to_string()),
            Focus::Preview | Focus::Transfers => None,
        };
        let Some(text) = text else {
            self.set_status("nothing hovered to copy", true);
            return;
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())) {
            Ok(()) => self.set_status(format!("copied {text}"), false),
            Err(err) => self.set_status(format!("clipboard copy failed: {err}"), true),
        }
    }

    /// Generates a temporary public link for the hovered remote object and copies it to the
    /// OS clipboard.
    pub async fn generate_share_url(&mut self) {
        if self.focus != Focus::Remote {
            self.set_status("switch focus to the S3 pane first", true);
            return;
        }
        let Some(entry) = self.current_entry() else {
            self.set_status("nothing hovered to share", true);
            return;
        };
        if entry.is_dir {
            self.set_status("can't generate a link for a directory — hover a file", true);
            return;
        }
        let key = entry.key.clone();
        let Some(client) = self.client.clone() else { return };
        const EXPIRY: std::time::Duration = std::time::Duration::from_secs(3600);
        match client.share_url(&self.bucket, &key, EXPIRY).await {
            Ok(Some(url)) => match arboard::Clipboard::new().and_then(|mut c| c.set_text(url)) {
                Ok(()) => self.set_status("share link (expires in 1h) copied to clipboard", false),
                Err(err) => self.set_status(format!("clipboard copy failed: {err}"), true),
            },
            Ok(None) => self.set_status("this backend doesn't support share links", true),
            Err(err) => self.set_error("failed to generate share link", &err),
        }
    }

    /// Permanently deletes the marked (or hovered) item(s) in the focused pane. No undo.
    pub async fn delete_selected(&mut self) -> Result<()> {
        match self.focus {
            Focus::Remote => {
                let Some(client) = self.client.clone() else { return Ok(()) };
                let keys: Vec<(String, bool)> = if !self.marked.is_empty() {
                    self.marked.iter().map(|k| (k.clone(), k.ends_with('/'))).collect()
                } else if let Some(entry) = self.current_entry() {
                    vec![(entry.key.clone(), entry.is_dir)]
                } else {
                    Vec::new()
                };
                // A directory can hold unboundedly many objects, so it deletes in the
                // background; a single object is one fast call and stays synchronous.
                let mut failed = 0;
                let mut deleted_files = 0;
                let mut queued_dirs = 0;
                for (key, is_dir) in &keys {
                    if *is_dir {
                        let id = self.next_id();
                        let cancel = Arc::new(AtomicBool::new(false));
                        let label = key.trim_end_matches('/').rsplit('/').next().unwrap_or(key).to_string();
                        self.jobs.push(Job {
                            id,
                            label,
                            kind: JobKind::RemoteDelete,
                            total_bytes: 0,
                            done_bytes: 0,
                            status: JobStatus::Running,
                            local_path: PathBuf::new(),
                            cancel: Some(cancel.clone()),
                        });
                        jobs::spawn_remote_delete(client.clone(), id, self.bucket.clone(), key.clone(), cancel, self.job_tx.clone());
                        queued_dirs += 1;
                    } else if let Err(err) = client.delete_object(&self.bucket, key).await {
                        failed += 1;
                        self.set_error("delete failed", &err);
                    } else {
                        deleted_files += 1;
                    }
                }
                if failed == 0 {
                    let mut parts = Vec::new();
                    if deleted_files > 0 {
                        parts.push(format!("deleted {deleted_files} item(s)"));
                    }
                    if queued_dirs > 0 {
                        let noun = if queued_dirs == 1 { "directory" } else { "directories" };
                        parts.push(format!("deleting {queued_dirs} {noun} in the background"));
                    }
                    if !parts.is_empty() {
                        self.set_status(parts.join("; "), false);
                    }
                }
                self.marked.clear();
                self.visual_anchor = None;
                self.refresh().await?;
            }
            Focus::Local => {
                let paths: Vec<PathBuf> = if !self.local_marked.is_empty() {
                    self.local_marked.iter().cloned().collect()
                } else if let Some(entry) = self.current_local_entry() {
                    vec![entry.path.clone()]
                } else {
                    Vec::new()
                };
                let mut failed = 0;
                for path in &paths {
                    let result = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
                    if let Err(err) = result {
                        failed += 1;
                        self.set_status(format!("delete failed: {err}"), true);
                    }
                }
                if failed == 0 {
                    self.set_status(format!("deleted {} item(s)", paths.len()), false);
                }
                self.local_marked.clear();
                self.visual_anchor = None;
                self.refresh_local();
            }
            Focus::Preview | Focus::Transfers => {}
        }
        self.refresh_preview();
        Ok(())
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
                if entry.is_dir {
                    // Same reasoning as `delete_selected`: runs as a cancellable background
                    // job — a directory rename is a same-store move to a new key.
                    let new_key = format!("{parent}{new_name}/");
                    let id = self.next_id();
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.jobs.push(Job {
                        id,
                        label: new_name,
                        kind: JobKind::RemoteMove,
                        total_bytes: 0,
                        done_bytes: 0,
                        status: JobStatus::Running,
                        local_path: PathBuf::new(),
                        cancel: Some(cancel.clone()),
                    });
                    jobs::spawn_remote_transfer(client, id, self.bucket.clone(), entry.key.clone(), new_key, true, true, cancel, self.job_tx.clone());
                    self.set_status("renaming directory in the background...", false);
                } else {
                    let new_key = format!("{parent}{new_name}");
                    match client.rename_object(&self.bucket, &entry.key, &new_key).await {
                        Ok(()) => self.set_status("renamed", false),
                        Err(err) => self.set_error("rename failed", &err),
                    }
                    self.refresh().await?;
                }
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
}
