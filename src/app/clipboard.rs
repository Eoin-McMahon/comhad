//! Clipboard-based move/copy. Mark items (or just hover one) in the focused pane, stage them
//! with `y` (copy) or `x` (cut), navigate anywhere — same pane, other pane, a different
//! directory/prefix — then `P` to paste. The destination pane's live listing is already the
//! "preview" of where things will land, so pasting reuses the same "are you sure?" confirm
//! popup every other write action goes through rather than a separate preview dialog.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::confirm::ConfirmKind;
use super::{App, Focus};
use crate::jobs::{self, Job, JobKind, JobStatus};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipMode {
    Copy,
    Move,
}

impl ClipMode {
    pub fn verb(self) -> &'static str {
        match self {
            ClipMode::Copy => "Copy",
            ClipMode::Move => "Move",
        }
    }
}

pub enum ClipItems {
    Local(Vec<PathBuf>),
    /// Keys (files) or prefixes (directories, trailing `/`) in the current bucket.
    Remote(Vec<String>),
}

pub struct Clipboard {
    pub mode: ClipMode,
    pub items: ClipItems,
}

/// Where to remove the source from once a cross-backend paste's transfer job reports `Done` —
/// tracked separately from the job itself since the transfer and the cleanup are two different
/// operations that can each fail independently.
pub enum PendingDelete {
    Local(PathBuf),
    /// `(key_or_prefix, is_prefix)`.
    Remote(String, bool),
}

impl App {
    fn stage(&mut self, mode: ClipMode) {
        let items = match self.focus {
            Focus::Local => {
                let paths: Vec<PathBuf> = if !self.local_marked.is_empty() {
                    self.local_marked.iter().cloned().collect()
                } else if let Some(entry) = self.current_local_entry() {
                    vec![entry.path.clone()]
                } else {
                    Vec::new()
                };
                if paths.is_empty() {
                    self.set_status("nothing selected to copy/cut", true);
                    return;
                }
                ClipItems::Local(paths)
            }
            Focus::Remote => {
                let keys: Vec<String> = if !self.marked.is_empty() {
                    self.marked.iter().cloned().collect()
                } else if let Some(entry) = self.current_entry() {
                    vec![entry.key.clone()]
                } else {
                    Vec::new()
                };
                if keys.is_empty() {
                    self.set_status("nothing selected to copy/cut", true);
                    return;
                }
                ClipItems::Remote(keys)
            }
            Focus::Preview | Focus::Transfers => {
                self.set_status("switch focus to a browsing pane first", true);
                return;
            }
        };
        let verb = mode.verb();
        self.clip = Some(Clipboard { mode, items });
        self.set_status(format!("{verb} staged {0} item(s) — navigate then P to paste", self.clip_len()), false);
    }

    fn clip_len(&self) -> usize {
        match self.clip.as_ref().map(|c| &c.items) {
            Some(ClipItems::Local(v)) => v.len(),
            Some(ClipItems::Remote(v)) => v.len(),
            None => 0,
        }
    }

    /// The clipboard's mode if `path` is one of the staged local items — for the local pane
    /// to render a distinct color/glyph on staged rows.
    pub fn clip_mode_for_local(&self, path: &Path) -> Option<ClipMode> {
        let clip = self.clip.as_ref()?;
        match &clip.items {
            ClipItems::Local(paths) if paths.iter().any(|p| p == path) => Some(clip.mode),
            _ => None,
        }
    }

    /// The clipboard's mode if `key` is one of the staged remote items — for the remote pane
    /// to render a distinct color/glyph on staged rows.
    pub fn clip_mode_for_remote(&self, key: &str) -> Option<ClipMode> {
        let clip = self.clip.as_ref()?;
        match &clip.items {
            ClipItems::Remote(keys) if keys.iter().any(|k| k == key) => Some(clip.mode),
            _ => None,
        }
    }

    /// Stages the marked/hovered items in the focused pane for a copy — bound to `y`.
    pub fn stage_copy(&mut self) {
        self.stage(ClipMode::Copy);
    }

    /// Stages the marked/hovered items in the focused pane for a move — bound to `x`.
    pub fn stage_cut(&mut self) {
        self.stage(ClipMode::Move);
    }

    /// Drops whatever's staged — folded into the existing `esc` "clear marks" handler.
    pub fn clear_clip(&mut self) {
        self.clip = None;
    }

    /// Asks before pasting the staged clipboard into the focused pane's current location.
    pub fn request_confirm_paste(&mut self) {
        let Some(clip) = &self.clip else {
            self.set_status("nothing staged — y to copy or x to cut first", true);
            return;
        };
        let count = match &clip.items {
            ClipItems::Local(v) => v.len(),
            ClipItems::Remote(v) => v.len(),
        };
        let dest = match self.focus {
            Focus::Local => self.local_cwd.display().to_string(),
            Focus::Remote => format!("s3://{}/{}", self.bucket, self.prefix),
            Focus::Preview | Focus::Transfers => {
                self.set_status("switch focus to the destination pane first", true);
                return;
            }
        };
        let noun = if count == 1 { "item" } else { "items" };
        let prompt = format!("{} {count} {noun} to:", clip.mode.verb());
        self.request_confirm_with_destination(prompt, dest, ConfirmKind::Paste, true);
    }

    /// Carries out the staged clipboard operation into the focused pane's current location,
    /// then clears the clipboard (and, for same-pane moves, the source marks).
    pub async fn run_paste(&mut self) -> Result<()> {
        let Some(Clipboard { mode, items }) = self.clip.take() else { return Ok(()) };

        match (items, self.focus) {
            (ClipItems::Local(paths), Focus::Local) => {
                for path in &paths {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let dest = self.local_cwd.join(&name);
                    let result = match mode {
                        ClipMode::Copy => copy_local(path, &dest),
                        ClipMode::Move => move_local(path, &dest),
                    };
                    if let Err(err) = result {
                        self.set_status(format!("paste failed for {name}: {err}"), true);
                    }
                }
                self.local_marked.clear();
                self.refresh_local();
            }
            (ClipItems::Remote(keys), Focus::Remote) => {
                let Some(client) = self.client.clone() else { return Ok(()) };
                for key in &keys {
                    let is_dir = key.ends_with('/');
                    let name = key.trim_end_matches('/').rsplit('/').next().unwrap_or(key);
                    let new_key = if is_dir { format!("{}{name}/", self.prefix) } else { format!("{}{name}", self.prefix) };
                    let result = match mode {
                        ClipMode::Copy if is_dir => client.copy_prefix(&self.bucket, key, &new_key).await,
                        ClipMode::Copy => client.copy_object(&self.bucket, key, &new_key).await,
                        ClipMode::Move if is_dir => client.rename_prefix(&self.bucket, key, &new_key).await,
                        ClipMode::Move => client.rename_object(&self.bucket, key, &new_key).await,
                    };
                    if let Err(err) = result {
                        self.set_error("paste failed", &err);
                    }
                }
                self.marked.clear();
                self.refresh().await?;
            }
            (ClipItems::Local(paths), Focus::Remote) => {
                // local → remote: copy is a plain upload; move uploads then, once each
                // transfer job reports Done, deletes the local source (see
                // `drain_job_messages`'s cleanup check).
                let Some(client) = self.client.clone() else { return Ok(()) };
                for path in paths {
                    let id = self.next_id();
                    let label = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string());
                    self.jobs.push(Job {
                        id,
                        label,
                        kind: JobKind::Upload,
                        total_bytes: 0,
                        done_bytes: 0,
                        status: JobStatus::Running,
                        local_path: path.clone(),
                    });
                    if mode == ClipMode::Move {
                        self.pending_deletes.insert(id, PendingDelete::Local(path.clone()));
                    }
                    jobs::spawn_upload(client.clone(), id, self.bucket.clone(), path, self.prefix.clone(), self.job_tx.clone());
                }
                self.set_status(format!("{} started", mode.verb().to_lowercase()), false);
            }
            (ClipItems::Remote(keys), Focus::Local) => {
                // remote → local: copy/move both stream every object under each key into the
                // local pane's directory, preserving structure (unlike `d`'s zip-on-download).
                let Some(client) = self.client.clone() else { return Ok(()) };
                let dest_dir = self.local_cwd.clone();
                for key in keys {
                    let is_dir = key.ends_with('/');
                    let files = if is_dir {
                        client.list_all_under(&self.bucket, &key).await?
                    } else if let Ok(size) = client.stat_size(&self.bucket, &key).await {
                        vec![crate::provider::RemoteEntry {
                            key: key.clone(),
                            name: key.rsplit('/').next().unwrap_or(&key).to_string(),
                            is_dir: false,
                            size,
                            last_modified: None,
                            modified_unix: None,
                        }]
                    } else {
                        Vec::new()
                    };
                    for entry in files {
                        // `rel` may contain nested slashes for a deep object — `download`
                        // (src/provider/s3.rs) creates any missing parent directories under
                        // `dest_dir` for us, so this mirrors the S3 structure locally as-is.
                        let rel = entry.key.strip_prefix(&key).unwrap_or(&entry.key).to_string();
                        let id = self.next_id();
                        self.jobs.push(Job {
                            id,
                            label: rel.clone(),
                            kind: JobKind::Download,
                            total_bytes: 0,
                            done_bytes: 0,
                            status: JobStatus::Running,
                            local_path: dest_dir.join(&rel),
                        });
                        if mode == ClipMode::Move {
                            self.pending_deletes.insert(id, PendingDelete::Remote(entry.key.clone(), false));
                        }
                        jobs::spawn_download_object(
                            client.clone(),
                            id,
                            self.bucket.clone(),
                            entry.key.clone(),
                            rel,
                            dest_dir.clone(),
                            self.job_tx.clone(),
                        );
                    }
                }
                self.set_status(format!("{} started", mode.verb().to_lowercase()), false);
            }
            _ => {}
        }
        self.refresh_preview();
        Ok(())
    }
}

fn copy_local(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        copy_dir_recursive(src, dst)
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn move_local(src: &Path, dst: &Path) -> std::io::Result<()> {
    // `rename` is atomic and cheap when it works, but fails across filesystems/devices —
    // fall back to a full copy-then-remove in that case.
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_local(src, dst)?;
            if src.is_dir() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.filter_map(|e| e.ok()) {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
