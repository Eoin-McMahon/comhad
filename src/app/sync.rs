//! One-way, non-destructive sync between the local pane's directory and the remote pane's
//! prefix, the equivalent of `aws s3 sync`: transfer files that are missing or out of date on
//! the destination, and *never* delete anything the destination has extra.
//!
//! Provider-agnostic: it only uses the [`StorageProvider`](crate::provider::StorageProvider)
//! listing already exposed to the browser, so it works for any future backend for free.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::Result;

use super::App;
use crate::jobs::{self, Job, JobKind, JobStatus};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    LocalToRemote,
    RemoteToLocal,
}

impl SyncDirection {
    fn flipped(self) -> Self {
        match self {
            Self::LocalToRemote => Self::RemoteToLocal,
            Self::RemoteToLocal => Self::LocalToRemote,
        }
    }

    pub fn arrow(self) -> &'static str {
        match self {
            Self::LocalToRemote => "→",
            Self::RemoteToLocal => "←",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LocalToRemote => "local → remote (upload)",
            Self::RemoteToLocal => "remote → local (download)",
        }
    }
}

/// What would happen to one file if the sync ran, from the destination's point of view.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    /// Missing on the destination, will be transferred (git-diff green).
    Add,
    /// Present on both but the source is newer or a different size, will overwrite (amber).
    Update,
    /// Identical on both sides, skipped (muted).
    Unchanged,
    /// Present only on the destination, shown for awareness but *never* deleted (red).
    ExtraSkipped,
}

pub struct SyncEntry {
    pub rel: String,
    pub action: SyncAction,
    pub local_size: Option<u64>,
    pub remote_size: Option<u64>,
    pub local_mtime: Option<i64>,
    pub remote_mtime: Option<i64>,
}

pub struct SyncState {
    pub direction: SyncDirection,
    pub entries: Vec<SyncEntry>,
    pub cursor: usize,
    /// First visible row, clamped against the panel height in the renderer so the two panels
    /// scroll in lockstep and the cursor stays on screen.
    pub offset: usize,
}

impl SyncState {
    /// How many entries would actually be transferred (add + update).
    pub fn actionable(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.action, SyncAction::Add | SyncAction::Update))
            .count()
    }
}

impl App {
    /// Opens the sync dialog, scanning both sides for the default local→remote direction.
    pub async fn open_sync(&mut self) {
        if self.client.is_none() {
            return;
        }
        self.load_sync_plan(SyncDirection::LocalToRemote).await;
    }

    /// Rescans in the opposite direction, keeping the dialog open.
    pub async fn flip_sync_direction(&mut self) {
        let Some(state) = &self.sync else { return };
        self.load_sync_plan(state.direction.flipped()).await;
    }

    async fn load_sync_plan(&mut self, direction: SyncDirection) {
        match self.build_sync_plan(direction).await {
            Ok(entries) => {
                self.sync = Some(SyncState { direction, entries, cursor: 0, offset: 0 });
            }
            Err(err) => {
                self.sync = None;
                self.set_error("sync scan failed", &err);
            }
        }
    }

    pub fn close_sync(&mut self) {
        self.sync = None;
    }

    pub fn move_sync_cursor(&mut self, delta: i32) {
        if let Some(state) = &mut self.sync {
            let len = state.entries.len();
            if len == 0 {
                return;
            }
            let next = state.cursor as i32 + delta;
            state.cursor = next.clamp(0, len as i32 - 1) as usize;
        }
    }

    /// Compares the local tree against the remote prefix and classifies every file. `direction`
    /// picks which side is the source and which is the destination (never deleted from).
    async fn build_sync_plan(&self, direction: SyncDirection) -> Result<Vec<SyncEntry>> {
        let Some(client) = &self.client else {
            return Ok(Vec::new());
        };

        let remote = client.list_all_under(&self.bucket, &self.prefix).await?;
        let mut remote_map: BTreeMap<String, (u64, i64)> = BTreeMap::new();
        for e in &remote {
            let rel = e.key.strip_prefix(&self.prefix).unwrap_or(&e.key).to_string();
            if rel.is_empty() || rel.ends_with('/') {
                continue;
            }
            remote_map.insert(rel, (e.size.max(0) as u64, e.modified_unix.unwrap_or(0)));
        }

        let local_map = collect_local_rel(&self.local_cwd);

        let mut rels: BTreeSet<String> = BTreeSet::new();
        rels.extend(remote_map.keys().cloned());
        rels.extend(local_map.keys().cloned());

        let mut entries = Vec::with_capacity(rels.len());
        for rel in rels {
            let l = local_map.get(&rel).copied();
            let r = remote_map.get(&rel).copied();
            let (src, dst) = match direction {
                SyncDirection::LocalToRemote => (l, r),
                SyncDirection::RemoteToLocal => (r, l),
            };
            let action = match (src, dst) {
                (None, None) => continue,
                (Some(_), None) => SyncAction::Add,
                (None, Some(_)) => SyncAction::ExtraSkipped,
                (Some((s_size, s_mtime)), Some((d_size, d_mtime))) => {
                    // Same size+mtime heuristic `aws s3 sync` uses; can occasionally re-transfer
                    // since filesystem/S3 mtimes are only roughly comparable.
                    if s_size != d_size || s_mtime > d_mtime {
                        SyncAction::Update
                    } else {
                        SyncAction::Unchanged
                    }
                }
            };
            entries.push(SyncEntry {
                rel,
                action,
                local_size: l.map(|(s, _)| s),
                remote_size: r.map(|(s, _)| s),
                local_mtime: l.map(|(_, m)| m),
                remote_mtime: r.map(|(_, m)| m),
            });
        }
        Ok(entries)
    }

    /// Kicks off transfer jobs for every add/update entry, then closes the dialog. Extra files
    /// on the destination are left untouched, sync never deletes.
    pub fn run_sync(&mut self) {
        let Some(state) = self.sync.take() else { return };
        let Some(client) = self.client.clone() else { return };

        let mut count = 0;
        for entry in &state.entries {
            if !matches!(entry.action, SyncAction::Add | SyncAction::Update) {
                continue;
            }
            let id = self.next_id();
            let key = format!("{}{}", self.prefix, entry.rel);
            match state.direction {
                SyncDirection::LocalToRemote => {
                    let local_path = self.local_cwd.join(&entry.rel);
                    self.jobs.push(Job {
                        id,
                        label: entry.rel.clone(),
                        kind: JobKind::Upload,
                        total_bytes: 0,
                        done_bytes: 0,
                        status: JobStatus::Running,
                        cancel: None,
                        local_path: local_path.clone(),
                    });
                    jobs::spawn_upload_file(client.clone(), id, self.bucket.clone(), local_path, key, self.job_tx.clone());
                }
                SyncDirection::RemoteToLocal => {
                    let dest = self.local_cwd.join(&entry.rel);
                    self.jobs.push(Job {
                        id,
                        label: entry.rel.clone(),
                        kind: JobKind::Download,
                        total_bytes: 0,
                        done_bytes: 0,
                        status: JobStatus::Running,
                        cancel: None,
                        local_path: dest,
                    });
                    jobs::spawn_download_object(
                        client.clone(),
                        id,
                        self.bucket.clone(),
                        key,
                        entry.rel.clone(),
                        self.local_cwd.clone(),
                        self.job_tx.clone(),
                    );
                }
            }
            count += 1;
        }

        if count == 0 {
            self.set_status("sync: already up to date", false);
        } else {
            self.set_status(format!("sync: {count} transfer(s) started"), false);
        }
    }
}

/// Recursively collects every file under `base`, keyed by its forward-slash relative path, with
/// `(size, mtime_unix)`. Includes dotfiles, a sync should be complete, unlike the browser pane.
fn collect_local_rel(base: &Path) -> BTreeMap<String, (u64, i64)> {
    let mut out = BTreeMap::new();
    if !base.is_dir() {
        return out;
    }
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(rel) = path.strip_prefix(base) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.insert(rel, (meta.len(), mtime));
        }
    }
    out
}
