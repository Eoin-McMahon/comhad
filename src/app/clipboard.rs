//! Clipboard-based move/copy: stage marked/hovered items with `y`/`x`, navigate anywhere, `P`
//! to paste. Staged items render a ghost `+` row (`ui::ghost_rows`) in every pane you visit.
//!
//! Same-backend transfers (local→local, S3→S3) run as background jobs like every cross-backend
//! transfer, so a big directory copy shows in the transfers strip and `esc` can cancel it.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;

use super::confirm::ConfirmKind;
use super::{App, Focus};
use crate::jobs::{self, Job, JobKind, JobStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl Clipboard {
    /// The name each staged item would land under — basename of a local path, or last segment
    /// of a remote key/prefix. Used for the ghost `+` row.
    pub fn ghost_names(&self) -> Vec<String> {
        match &self.items {
            ClipItems::Local(paths) => {
                paths.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).collect()
            }
            ClipItems::Remote(keys) => keys.iter().map(|k| k.trim_end_matches('/').rsplit('/').next().unwrap_or(k).to_string()).collect(),
        }
    }
}

/// Where to remove the source from once a cross-backend paste's job reports `Done` — tracked
/// separately since transfer and cleanup can each fail independently.
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

    /// The clipboard's mode if `path` is staged, for rendering a distinct color/glyph.
    pub fn clip_mode_for_local(&self, path: &Path) -> Option<ClipMode> {
        let clip = self.clip.as_ref()?;
        match &clip.items {
            ClipItems::Local(paths) if paths.iter().any(|p| p == path) => Some(clip.mode),
            _ => None,
        }
    }

    /// The clipboard's mode if `key` is staged, for rendering a distinct color/glyph.
    pub fn clip_mode_for_remote(&self, key: &str) -> Option<ClipMode> {
        let clip = self.clip.as_ref()?;
        match &clip.items {
            ClipItems::Remote(keys) if keys.iter().any(|k| k == key) => Some(clip.mode),
            _ => None,
        }
    }

    pub fn stage_copy(&mut self) {
        self.stage(ClipMode::Copy);
    }

    pub fn stage_cut(&mut self) {
        self.stage(ClipMode::Move);
    }

    /// Drops whatever's staged — folded into the `esc` "clear marks" handler.
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
                // Runs as a background job like every other transfer, so a big directory
                // doesn't freeze the UI and `esc` can cancel it partway through.
                let delete_source = mode == ClipMode::Move;
                for path in paths {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let dest = self.local_cwd.join(&name);
                    let id = self.next_id();
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.jobs.push(Job {
                        id,
                        label: name,
                        kind: if delete_source { JobKind::LocalMove } else { JobKind::LocalCopy },
                        total_bytes: 0,
                        done_bytes: 0,
                        status: JobStatus::Running,
                        local_path: dest.clone(),
                        cancel: Some(cancel.clone()),
                    });
                    jobs::spawn_local_transfer(id, path, dest, delete_source, cancel, self.job_tx.clone());
                }
                self.local_marked.clear();
                self.visual_anchor = None;
                self.set_status(format!("{} started", mode.verb().to_lowercase()), false);
            }
            (ClipItems::Remote(keys), Focus::Remote) => {
                let Some(client) = self.client.clone() else { return Ok(()) };
                let delete_source = mode == ClipMode::Move;
                for key in keys {
                    let is_dir = key.ends_with('/');
                    let name = key.trim_end_matches('/').rsplit('/').next().unwrap_or(&key).to_string();
                    let new_key = if is_dir { format!("{}{name}/", self.prefix) } else { format!("{}{name}", self.prefix) };
                    let id = self.next_id();
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.jobs.push(Job {
                        id,
                        label: name,
                        kind: if delete_source { JobKind::RemoteMove } else { JobKind::RemoteCopy },
                        total_bytes: 0,
                        done_bytes: 0,
                        status: JobStatus::Running,
                        local_path: PathBuf::new(),
                        cancel: Some(cancel.clone()),
                    });
                    jobs::spawn_remote_transfer(client.clone(), id, self.bucket.clone(), key, new_key, is_dir, delete_source, cancel, self.job_tx.clone());
                }
                self.marked.clear();
                self.visual_anchor = None;
                self.set_status(format!("{} started", mode.verb().to_lowercase()), false);
            }
            (ClipItems::Local(paths), Focus::Remote) => {
                // local → remote: copy is a plain upload; move uploads then deletes the local
                // source once the job reports Done (see `drain_job_messages`).
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
                        cancel: None,
                    });
                    if mode == ClipMode::Move {
                        self.pending_deletes.insert(id, PendingDelete::Local(path.clone()));
                    }
                    jobs::spawn_upload(client.clone(), id, self.bucket.clone(), path, self.prefix.clone(), self.job_tx.clone());
                }
                self.set_status(format!("{} started", mode.verb().to_lowercase()), false);
            }
            (ClipItems::Remote(keys), Focus::Local) => {
                // remote → local: stream every object under each key into the local directory,
                // preserving structure (unlike `d`'s zip-on-download).
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
                        // `download` (src/provider/s3.rs) creates missing parent dirs, so
                        // nested `rel` mirrors the S3 structure locally as-is.
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
                            cancel: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        App::new(Vec::new(), ratatui_image::picker::Picker::halfblocks(), crate::config::AppConfig::default())
    }

    #[test]
    fn ghost_names_uses_basename_for_local_items() {
        let clip = Clipboard { mode: ClipMode::Copy, items: ClipItems::Local(vec![PathBuf::from("/a/b/file.txt")]) };
        assert_eq!(clip.ghost_names(), vec!["file.txt".to_string()]);
    }

    #[test]
    fn ghost_names_uses_last_segment_for_remote_keys_and_prefixes() {
        let clip = Clipboard {
            mode: ClipMode::Move,
            items: ClipItems::Remote(vec!["a/b/file.txt".to_string(), "a/b/dir/".to_string()]),
        };
        assert_eq!(clip.ghost_names(), vec!["file.txt".to_string(), "dir".to_string()]);
    }

    #[test]
    fn stage_copy_on_local_pane_stages_the_hovered_entry() {
        let mut app = test_app();
        app.focus = Focus::Local;
        app.local_entries = vec![crate::local::LocalEntry {
            path: PathBuf::from("/tmp/foo.txt"),
            name: "foo.txt".to_string(),
            is_dir: false,
            size: 0,
            modified: None,
        }];
        app.stage_copy();
        let clip = app.clip.as_ref().expect("staged");
        assert!(matches!(clip.mode, ClipMode::Copy));
        match &clip.items {
            ClipItems::Local(paths) => assert_eq!(paths, &vec![PathBuf::from("/tmp/foo.txt")]),
            _ => panic!("expected local items"),
        }
    }

    #[test]
    fn stage_cut_with_nothing_selected_sets_an_error_and_stages_nothing() {
        let mut app = test_app();
        app.focus = Focus::Local;
        app.stage_cut();
        assert!(app.clip.is_none());
        assert_eq!(app.status, Some(("nothing selected to copy/cut".to_string(), true)));
    }

    #[test]
    fn clip_mode_for_local_only_matches_staged_paths() {
        let mut app = test_app();
        app.clip = Some(Clipboard { mode: ClipMode::Move, items: ClipItems::Local(vec![PathBuf::from("/a")]) });
        assert_eq!(app.clip_mode_for_local(Path::new("/a")), Some(ClipMode::Move));
        assert_eq!(app.clip_mode_for_local(Path::new("/b")), None);
    }

    #[test]
    fn clip_mode_for_remote_only_matches_staged_keys() {
        let mut app = test_app();
        app.clip = Some(Clipboard { mode: ClipMode::Copy, items: ClipItems::Remote(vec!["a/b".to_string()]) });
        assert_eq!(app.clip_mode_for_remote("a/b"), Some(ClipMode::Copy));
        assert_eq!(app.clip_mode_for_remote("a/c"), None);
    }

    #[test]
    fn clear_clip_drops_the_staged_items() {
        let mut app = test_app();
        app.clip = Some(Clipboard { mode: ClipMode::Copy, items: ClipItems::Local(vec![PathBuf::from("/a")]) });
        app.clear_clip();
        assert!(app.clip.is_none());
    }

    #[test]
    fn request_confirm_paste_with_nothing_staged_sets_an_error() {
        let mut app = test_app();
        app.request_confirm_paste();
        assert_eq!(app.status, Some(("nothing staged — y to copy or x to cut first".to_string(), true)));
    }
}
