//! Browsing, navigation, and transfer orchestration for [`App`] — everything that moves the
//! cursor around the two panes or kicks off a download/upload job.

use std::path::PathBuf;

use anyhow::Result;

use super::{App, Focus, Screen};
use crate::jobs::{self, Job, JobId, JobKind, JobStatus};
use crate::local::{self, LocalEntry};
use crate::provider::{self, RemoteEntry};

impl App {
    /// The remote entries currently shown, after applying the active filter and sort.
    pub fn visible_entries(&self) -> Vec<&RemoteEntry> {
        let mut out: Vec<&RemoteEntry> = match &self.filter {
            Some(f) if !f.is_empty() => {
                let needle = f.to_lowercase();
                self.entries.iter().filter(|e| e.name.to_lowercase().contains(&needle)).collect()
            }
            _ => self.entries.iter().collect(),
        };
        super::sort::sort_entries(&mut out, self.remote_sort);
        out
    }

    /// The local entries currently shown, after applying the active local filter and sort.
    pub fn visible_local_entries(&self) -> Vec<&LocalEntry> {
        let mut out: Vec<&LocalEntry> = match &self.local_filter {
            Some(f) if !f.is_empty() => {
                let needle = f.to_lowercase();
                self.local_entries.iter().filter(|e| e.name.to_lowercase().contains(&needle)).collect()
            }
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

    /// Sets (or clears) the filter for the focused pane.
    pub fn set_filter(&mut self, value: Option<String>) {
        match self.focus {
            Focus::Local => self.local_filter = value,
            _ => self.filter = value,
        }
    }

    pub async fn connect(&mut self, index: usize) -> Result<()> {
        let conn = self.connections[index].1.clone();
        self.loading = true;
        let client = match provider::connect(&conn).await {
            Ok(c) => c,
            Err(err) => {
                self.loading = false;
                self.set_error("connect failed", &err);
                return Ok(());
            }
        };
        self.connection = Some(conn.clone());
        let (bookmark_bucket, bookmark_prefix) = conn.bucket_and_prefix();

        match client.list_containers().await {
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
                    self.local_filter = None;
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
                    self.local_filter = None;
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
                let len = self.visible_local_entries().len();
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
}
