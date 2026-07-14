use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::provider::{RemoteEntry, StorageProvider};

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Download,
    Upload,
    Zip,
    LocalCopy,
    LocalMove,
    RemoteCopy,
    RemoteMove,
    RemoteDelete,
}

#[derive(Debug, Clone)]
pub enum JobStatus {
    Running,
    Done,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub label: String,
    pub kind: JobKind,
    /// 0 means "unknown", render an indeterminate spinner instead of a percentage.
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub status: JobStatus,
    /// Path the transfers pane opens/reveals once done; empty for remote-to-remote copy/move.
    pub local_path: PathBuf,
    /// Checked between units of work by cancellable jobs; `None` for jobs that don't support it.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// Jobs poll this at safe stopping points rather than being killed mid-write, so a cancelled move never leaves a half-written file.
fn is_cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

impl Job {
    pub fn progress_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.done_bytes as f64 / self.total_bytes as f64).min(1.0)
        }
    }
}

#[derive(Debug)]
pub enum ProgressMsg {
    New {
        id: JobId,
        label: String,
        kind: JobKind,
        total_bytes: u64,
    },
    Advance {
        id: JobId,
        delta: u64,
    },
    Done {
        id: JobId,
        kind: JobKind,
    },
    Cancelled {
        id: JobId,
    },
    Failed {
        id: JobId,
        error: String,
    },
}

/// Downloads a single object into `dest_dir`, reporting progress as it streams.
pub fn spawn_download_object(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    key: String,
    label: String,
    dest_dir: PathBuf,
    tx: UnboundedSender<ProgressMsg>,
) {
    tokio::spawn(async move {
        let total = client.stat_size(&bucket, &key).await.unwrap_or(0).max(0) as u64;
        let _ = tx.send(ProgressMsg::New {
            id,
            label: label.clone(),
            kind: JobKind::Download,
            total_bytes: total,
        });

        let dest = dest_dir.join(&label);
        let tx_chunk = tx.clone();
        let on_chunk = move |n: u64| {
            let _ = tx_chunk.send(ProgressMsg::Advance { id, delta: n });
        };
        let result = client.download(&bucket, &key, &dest, &on_chunk).await;

        match result {
            Ok(()) => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Download });
            }
            Err(err) => {
                let _ = tx.send(ProgressMsg::Failed {
                    id,
                    error: err.to_string(),
                });
            }
        }
    });
}

/// Downloads every object under the given entries into a single zip archive in `dest_dir`.
/// `entries` must already be files (expanded via `list_all_under`); `strip_prefix` is removed
/// from each key to compute the path stored inside the archive.
#[allow(clippy::too_many_arguments)] // a cohesive spawn descriptor; a struct wrapper wouldn't earn its keep
pub fn spawn_zip_download(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    entries: Vec<RemoteEntry>,
    strip_prefix: String,
    zip_name: String,
    dest_dir: PathBuf,
    tx: UnboundedSender<ProgressMsg>,
) {
    tokio::spawn(async move {
        let total: u64 = entries.iter().map(|e| e.size.max(0) as u64).sum();
        let _ = tx.send(ProgressMsg::New {
            id,
            label: zip_name.clone(),
            kind: JobKind::Zip,
            total_bytes: total,
        });

        let dest = dest_dir.join(&zip_name);
        let result = run_zip_download(&client, &bucket, &entries, &strip_prefix, &dest, id, &tx).await;

        match result {
            Ok(()) => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Zip });
            }
            Err(err) => {
                let _ = tx.send(ProgressMsg::Failed {
                    id,
                    error: err.to_string(),
                });
            }
        }
    });
}

async fn run_zip_download(
    client: &Arc<dyn StorageProvider>,
    bucket: &str,
    entries: &[RemoteEntry],
    strip_prefix: &str,
    dest: &Path,
    id: JobId,
    tx: &UnboundedSender<ProgressMsg>,
) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for entry in entries {
        let archive_path = entry.key.strip_prefix(strip_prefix).unwrap_or(&entry.key);
        zip.start_file(archive_path, options)?;
        let tx_chunk = tx.clone();
        let on_chunk = move |n: u64| {
            let _ = tx_chunk.send(ProgressMsg::Advance { id, delta: n });
        };
        client.download_to_writer(bucket, &entry.key, &mut zip, &on_chunk).await?;
    }
    zip.finish()?;
    Ok(())
}

/// Uploads a local file (or, recursively, every file under a local directory) to `key_prefix`.
pub fn spawn_upload(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    local_path: PathBuf,
    key_prefix: String,
    tx: UnboundedSender<ProgressMsg>,
) {
    tokio::spawn(async move {
        let files = collect_local_files(&local_path);
        let total: u64 = files.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
        let label = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.display().to_string());
        let _ = tx.send(ProgressMsg::New {
            id,
            label: label.clone(),
            kind: JobKind::Upload,
            total_bytes: total,
        });

        let base = local_path.parent().unwrap_or(&local_path);
        let mut failed = None;
        for file in &files {
            let rel = file.strip_prefix(base).unwrap_or(file);
            let key = format!("{key_prefix}{}", rel.to_string_lossy().replace('\\', "/"));
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            if let Err(err) = client.upload_file(&bucket, file, &key).await {
                failed = Some(err.to_string());
                break;
            }
            let _ = tx.send(ProgressMsg::Advance { id, delta: size });
        }

        match failed {
            None => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Upload });
            }
            Some(error) => {
                let _ = tx.send(ProgressMsg::Failed { id, error });
            }
        }
    });
}

/// Uploads a single local file to an exact `key`, unlike [`spawn_upload`] which derives keys
/// from a prefix + directory structure. Used by sync, which precomputes the destination key.
pub fn spawn_upload_file(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    local_path: PathBuf,
    key: String,
    tx: UnboundedSender<ProgressMsg>,
) {
    tokio::spawn(async move {
        let size = local_path.metadata().map(|m| m.len()).unwrap_or(0);
        let label = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.display().to_string());
        let _ = tx.send(ProgressMsg::New { id, label, kind: JobKind::Upload, total_bytes: size });

        match client.upload_file(&bucket, &local_path, &key).await {
            Ok(()) => {
                let _ = tx.send(ProgressMsg::Advance { id, delta: size });
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Upload });
            }
            Err(err) => {
                let _ = tx.send(ProgressMsg::Failed { id, error: err.to_string() });
            }
        }
    });
}

/// Copies (or, if `delete_source`, moves) a local file/directory in the background, checking
/// `cancel` between files so a big directory copy can be stopped partway through. Progress is
/// indeterminate — sizing the tree upfront isn't worth the extra pass for a fast local op.
pub fn spawn_local_transfer(
    id: JobId,
    src: PathBuf,
    dest: PathBuf,
    delete_source: bool,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<ProgressMsg>,
) {
    let label = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| src.display().to_string());
    let kind = if delete_source { JobKind::LocalMove } else { JobKind::LocalCopy };
    let _ = tx.send(ProgressMsg::New { id, label, kind, total_bytes: 0 });

    tokio::task::spawn_blocking(move || {
        let result = if delete_source { move_local_cancellable(&src, &dest, &cancel) } else { copy_local_cancellable(&src, &dest, &cancel) };
        match result {
            Ok(()) => {
                let _ = tx.send(ProgressMsg::Done { id, kind });
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
                let _ = tx.send(ProgressMsg::Cancelled { id });
            }
            Err(err) => {
                let _ = tx.send(ProgressMsg::Failed { id, error: err.to_string() });
            }
        }
    });
}

fn cancelled_err() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")
}

fn copy_local_cancellable(src: &Path, dst: &Path, cancel: &AtomicBool) -> std::io::Result<()> {
    if is_cancelled(cancel) {
        return Err(cancelled_err());
    }
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)?.filter_map(|e| e.ok()) {
            if is_cancelled(cancel) {
                return Err(cancelled_err());
            }
            let from = entry.path();
            let to = dst.join(entry.file_name());
            copy_local_cancellable(&from, &to, cancel)?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn move_local_cancellable(src: &Path, dst: &Path, cancel: &AtomicBool) -> std::io::Result<()> {
    if is_cancelled(cancel) {
        return Err(cancelled_err());
    }
    // `rename` is atomic and cheap when it works, but fails across filesystems/devices —
    // fall back to a full copy-then-remove in that case.
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_local_cancellable(src, dst, cancel)?;
            if src.is_dir() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
    }
}

/// Copies (or, if `delete_source`, moves) a single S3 object, or every object under a prefix
/// one at a time, so `cancel` can be checked between objects.
#[allow(clippy::too_many_arguments)]
pub fn spawn_remote_transfer(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    old_key: String,
    new_key: String,
    is_dir: bool,
    delete_source: bool,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<ProgressMsg>,
) {
    let label = old_key.trim_end_matches('/').rsplit('/').next().unwrap_or(&old_key).to_string();
    let kind = if delete_source { JobKind::RemoteMove } else { JobKind::RemoteCopy };
    let _ = tx.send(ProgressMsg::New { id, label, kind, total_bytes: 0 });

    tokio::spawn(async move {
        let result = run_remote_transfer(&client, &bucket, &old_key, &new_key, is_dir, delete_source, &cancel).await;
        match result {
            Ok(true) => {
                let _ = tx.send(ProgressMsg::Cancelled { id });
            }
            Ok(false) => {
                let _ = tx.send(ProgressMsg::Done { id, kind });
            }
            Err(err) => {
                let _ = tx.send(ProgressMsg::Failed { id, error: err.to_string() });
            }
        }
    });
}

/// Returns `Ok(true)` if `cancel` was set before finishing, `Ok(false)` if it ran to completion.
async fn run_remote_transfer(
    client: &Arc<dyn StorageProvider>,
    bucket: &str,
    old_key: &str,
    new_key: &str,
    is_dir: bool,
    delete_source: bool,
    cancel: &AtomicBool,
) -> anyhow::Result<bool> {
    if !is_dir {
        if is_cancelled(cancel) {
            return Ok(true);
        }
        if delete_source {
            client.rename_object(bucket, old_key, new_key).await?;
        } else {
            client.copy_object(bucket, old_key, new_key).await?;
        }
        return Ok(false);
    }
    let objects = client.list_all_under(bucket, old_key).await?;
    for obj in objects {
        if is_cancelled(cancel) {
            return Ok(true);
        }
        let suffix = obj.key.strip_prefix(old_key).unwrap_or(&obj.key);
        let dest_key = format!("{new_key}{suffix}");
        if delete_source {
            client.rename_object(bucket, &obj.key, &dest_key).await?;
        } else {
            client.copy_object(bucket, &obj.key, &dest_key).await?;
        }
    }
    Ok(false)
}

/// Permanently deletes every object under `prefix` in the background, checking `cancel`
/// between objects — same shape as [`spawn_remote_transfer`].
pub fn spawn_remote_delete(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    prefix: String,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<ProgressMsg>,
) {
    let label = prefix.trim_end_matches('/').rsplit('/').next().unwrap_or(&prefix).to_string();
    let _ = tx.send(ProgressMsg::New { id, label, kind: JobKind::RemoteDelete, total_bytes: 0 });

    tokio::spawn(async move {
        let result = run_remote_delete(&client, &bucket, &prefix, &cancel).await;
        match result {
            Ok(true) => {
                let _ = tx.send(ProgressMsg::Cancelled { id });
            }
            Ok(false) => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::RemoteDelete });
            }
            Err(err) => {
                let _ = tx.send(ProgressMsg::Failed { id, error: err.to_string() });
            }
        }
    });
}

async fn run_remote_delete(client: &Arc<dyn StorageProvider>, bucket: &str, prefix: &str, cancel: &AtomicBool) -> anyhow::Result<bool> {
    let objects = client.list_all_under(bucket, prefix).await?;
    for obj in objects {
        if is_cancelled(cancel) {
            return Ok(true);
        }
        client.delete_object(bucket, &obj.key).await?;
    }
    Ok(false)
}

fn collect_local_files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut out = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
