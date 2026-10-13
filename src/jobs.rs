use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

impl JobKind {
    /// Whether the bytes land on the local side, i.e. `local_path` is the destination rather
    /// than the source. Decides which listing row carries the progress bar: the side being
    /// written fills, the side being read only shows that it's busy.
    pub fn writes_local(self) -> bool {
        matches!(self, JobKind::Download | JobKind::Zip | JobKind::LocalCopy | JobKind::LocalMove)
    }

    /// The remote counterpart: `remote_key` is the destination key rather than the source.
    /// A delete writes to neither side, so both of these are false for it.
    pub fn writes_remote(self) -> bool {
        matches!(self, JobKind::Upload | JobKind::RemoteCopy | JobKind::RemoteMove)
    }
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
    /// The single remote object this job reads or writes, so the remote pane can find the row a
    /// job belongs to (see [`App::transfer_progress_remote`](crate::app::App::transfer_progress_remote)).
    /// For a write it's the destination key, which is the one that shows up in the listing.
    /// `None` for jobs that touch no remote object at all, like a local copy or move.
    pub remote_key: Option<String>,
    /// Set by the jobs pane's cancel action; every job watches it, either by polling at a safe
    /// stopping point or by racing it against the request in flight (see [`until_cancelled`]).
    pub cancel: Arc<AtomicBool>,
    /// What it takes to run this job again, for the jobs pane's retry action. `None` for the
    /// kinds retry doesn't offer: a half-finished move or delete has already consumed part of
    /// its own source, so re-running it from the top is not the same operation the user asked
    /// for the first time.
    pub spec: Option<JobSpec>,
}

/// Everything needed to start (or restart) one of the network jobs. Held on the [`Job`] so a
/// retry rebuilds the task from the same description the original row was created from.
#[derive(Debug, Clone)]
pub enum JobSpec {
    Download { bucket: String, key: String, label: String, dest_dir: PathBuf },
    Zip { bucket: String, entries: Vec<RemoteEntry>, strip_prefix: String, zip_name: String, dest_dir: PathBuf },
    /// `name` overrides the top-level name the upload lands under (the file's key, or the
    /// directory's prefix); `None` keeps the local name.
    Upload { bucket: String, local_path: PathBuf, key_prefix: String, name: Option<String> },
    UploadFile { bucket: String, local_path: PathBuf, key: String },
}

/// Starts the task for `spec` under `id`, the single entry point for both the first run and a
/// retry so the two can't drift apart.
pub fn spawn(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    spec: &JobSpec,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<ProgressMsg>,
) {
    match spec.clone() {
        JobSpec::Download { bucket, key, label, dest_dir } => {
            spawn_download_object(client, id, bucket, key, label, dest_dir, cancel, tx)
        }
        JobSpec::Zip { bucket, entries, strip_prefix, zip_name, dest_dir } => {
            spawn_zip_download(client, id, bucket, entries, strip_prefix, zip_name, dest_dir, cancel, tx)
        }
        JobSpec::Upload { bucket, local_path, key_prefix, name } => {
            spawn_upload(client, id, bucket, local_path, key_prefix, name, cancel, tx)
        }
        JobSpec::UploadFile { bucket, local_path, key } => {
            spawn_upload_file(client, id, bucket, local_path, key, cancel, tx)
        }
    }
}

/// Jobs poll this at safe stopping points rather than being killed mid-write, so a cancelled move never leaves a half-written file.
fn is_cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

/// Resolves once `cancel` is set. Polls rather than waiting on a notify so the flag stays a
/// plain `AtomicBool`, shared with the jobs that check it between units of work instead.
async fn cancelled(cancel: &AtomicBool) {
    while !is_cancelled(cancel) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Runs `fut` unless `cancel` fires first, in which case the future is dropped and `None` comes
/// back. For the network jobs: a request already in flight has no stopping point to poll at, so
/// it gets aborted mid-stream and the caller cleans up whatever partial file the drop left.
async fn until_cancelled<T>(fut: impl Future<Output = T>, cancel: &AtomicBool) -> Option<T> {
    tokio::select! {
        out = fut => Some(out),
        () = cancelled(cancel) => None,
    }
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
#[allow(clippy::too_many_arguments)] // a cohesive spawn descriptor; see `JobSpec::Download`
pub fn spawn_download_object(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    key: String,
    label: String,
    dest_dir: PathBuf,
    cancel: Arc<AtomicBool>,
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
        let result = until_cancelled(client.download(&bucket, &key, &dest, &on_chunk), &cancel).await;

        match result {
            Some(Ok(())) => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Download });
            }
            Some(Err(err)) => {
                let _ = tx.send(ProgressMsg::Failed {
                    id,
                    error: err.to_string(),
                });
            }
            // The stream was dropped part-way through a write, so the stub it left behind goes
            // with it rather than sitting in the pane looking like a complete file.
            None => {
                let _ = tokio::fs::remove_file(&dest).await;
                let _ = tx.send(ProgressMsg::Cancelled { id });
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
    cancel: Arc<AtomicBool>,
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
        let result = run_zip_download(&client, &bucket, &entries, &strip_prefix, &dest, id, &cancel, &tx).await;

        match result {
            Ok(false) => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Zip });
            }
            // A half-written archive isn't a usable file, so it doesn't outlive the cancel.
            Ok(true) => {
                let _ = tokio::fs::remove_file(&dest).await;
                let _ = tx.send(ProgressMsg::Cancelled { id });
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

/// Returns `Ok(true)` if `cancel` was set before the archive was finished, `Ok(false)` if it ran
/// to completion.
#[allow(clippy::too_many_arguments)] // mirrors the spawn descriptor above
async fn run_zip_download(
    client: &Arc<dyn StorageProvider>,
    bucket: &str,
    entries: &[RemoteEntry],
    strip_prefix: &str,
    dest: &Path,
    id: JobId,
    cancel: &AtomicBool,
    tx: &UnboundedSender<ProgressMsg>,
) -> anyhow::Result<bool> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for entry in entries {
        if is_cancelled(cancel) {
            return Ok(true);
        }
        let archive_path = entry.key.strip_prefix(strip_prefix).unwrap_or(&entry.key);
        zip.start_file(archive_path, options)?;
        let tx_chunk = tx.clone();
        let on_chunk = move |n: u64| {
            let _ = tx_chunk.send(ProgressMsg::Advance { id, delta: n });
        };
        match until_cancelled(client.download_to_writer(bucket, &entry.key, &mut zip, &on_chunk), cancel).await {
            Some(result) => result?,
            None => return Ok(true),
        }
    }
    zip.finish()?;
    Ok(false)
}

/// The key an upload of `local_path` into `key_prefix` lands on, as it appears in a listing of
/// that prefix: the object itself for a file, or the directory prefix a recursive upload fills in.
/// Lives next to [`spawn_upload`] so the two stay in step about how a destination is derived.
pub fn upload_destination_key(local_path: &Path, key_prefix: &str) -> String {
    upload_destination_key_named(local_path, key_prefix, None)
}

/// Like [`upload_destination_key`], with `name` standing in for the local file or directory name.
pub fn upload_destination_key_named(local_path: &Path, key_prefix: &str, name: Option<&str>) -> String {
    let name = match name {
        Some(name) => name.to_string(),
        None => local_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
    };
    let suffix = if local_path.is_dir() { "/" } else { "" };
    format!("{key_prefix}{name}{suffix}")
}

/// Uploads a local file (or, recursively, every file under a local directory) to `key_prefix`,
/// under `name` instead of the local name when one is given.
#[allow(clippy::too_many_arguments)] // a cohesive spawn descriptor; see `JobSpec::Upload`
pub fn spawn_upload(
    client: Arc<dyn StorageProvider>,
    id: JobId,
    bucket: String,
    local_path: PathBuf,
    key_prefix: String,
    name: Option<String>,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<ProgressMsg>,
) {
    tokio::spawn(async move {
        let files = collect_local_files(&local_path);
        let total: u64 = files.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
        let label = name.clone().unwrap_or_else(|| {
            local_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| local_path.display().to_string())
        });
        let _ = tx.send(ProgressMsg::New {
            id,
            label: label.clone(),
            kind: JobKind::Upload,
            total_bytes: total,
        });

        let base = local_path.parent().unwrap_or(&local_path);
        // Ok(false) ran to completion, Ok(true) stopped on `cancel`, Err carries the failure.
        let mut outcome: Result<bool, String> = Ok(false);
        for file in &files {
            if is_cancelled(&cancel) {
                outcome = Ok(true);
                break;
            }
            let key = match &name {
                // Renamed: the path inside `local_path` hangs off the new name instead.
                Some(name) => match file.strip_prefix(&local_path) {
                    Ok(inner) if !inner.as_os_str().is_empty() => {
                        format!("{key_prefix}{name}/{}", inner.to_string_lossy().replace('\\', "/"))
                    }
                    _ => format!("{key_prefix}{name}"),
                },
                None => {
                    let rel = file.strip_prefix(base).unwrap_or(file);
                    format!("{key_prefix}{}", rel.to_string_lossy().replace('\\', "/"))
                }
            };
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            match until_cancelled(client.upload_file(&bucket, file, &key), &cancel).await {
                Some(Ok(())) => {}
                Some(Err(err)) => {
                    outcome = Err(err.to_string());
                    break;
                }
                None => {
                    outcome = Ok(true);
                    break;
                }
            }
            let _ = tx.send(ProgressMsg::Advance { id, delta: size });
        }

        match outcome {
            Ok(false) => {
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Upload });
            }
            // Objects already uploaded stay put: unlike a partial download there's no single
            // half-written file to clean up, and deleting them would undo work that succeeded.
            Ok(true) => {
                let _ = tx.send(ProgressMsg::Cancelled { id });
            }
            Err(error) => {
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
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<ProgressMsg>,
) {
    tokio::spawn(async move {
        let size = local_path.metadata().map(|m| m.len()).unwrap_or(0);
        let label = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.display().to_string());
        let _ = tx.send(ProgressMsg::New { id, label, kind: JobKind::Upload, total_bytes: size });

        match until_cancelled(client.upload_file(&bucket, &local_path, &key), &cancel).await {
            Some(Ok(())) => {
                let _ = tx.send(ProgressMsg::Advance { id, delta: size });
                let _ = tx.send(ProgressMsg::Done { id, kind: JobKind::Upload });
            }
            Some(Err(err)) => {
                let _ = tx.send(ProgressMsg::Failed { id, error: err.to_string() });
            }
            None => {
                let _ = tx.send(ProgressMsg::Cancelled { id });
            }
        }
    });
}

/// Copies (or, if `delete_source`, moves) a local file/directory in the background, checking
/// `cancel` between files so a big directory copy can be stopped partway through. Progress is
/// indeterminate, sizing the tree upfront isn't worth the extra pass for a fast local op.
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
    // `rename` is atomic and cheap when it works, but fails across filesystems/devices, 
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
/// between objects, same shape as [`spawn_remote_transfer`].
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_ratio_is_zero_when_total_is_unknown() {
        let job = Job {
            id: 1,
            label: "x".into(),
            kind: JobKind::Download,
            total_bytes: 0,
            done_bytes: 40,
            status: JobStatus::Running,
            local_path: PathBuf::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            remote_key: None,
            spec: None,
        };
        assert_eq!(job.progress_ratio(), 0.0);
    }

    #[test]
    fn progress_ratio_divides_done_by_total() {
        let mut job = Job {
            id: 1,
            label: "x".into(),
            kind: JobKind::Download,
            total_bytes: 200,
            done_bytes: 50,
            status: JobStatus::Running,
            local_path: PathBuf::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            remote_key: None,
            spec: None,
        };
        assert_eq!(job.progress_ratio(), 0.25);

        job.done_bytes = 500;
        assert_eq!(job.progress_ratio(), 1.0, "overshooting done_bytes clamps to 1.0");
    }

    #[test]
    fn upload_destination_key_appends_the_file_name_to_the_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("report.csv");
        std::fs::write(&file, b"a").unwrap();

        assert_eq!(upload_destination_key(&file, "data/2026/"), "data/2026/report.csv");
        assert_eq!(upload_destination_key(&file, ""), "report.csv", "the bucket root has no prefix");
    }

    #[test]
    fn upload_destination_key_marks_a_directory_with_a_trailing_slash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("logs");
        std::fs::create_dir(&nested).unwrap();

        assert_eq!(
            upload_destination_key(&nested, "data/"),
            "data/logs/",
            "a recursive upload's row in the listing is the directory prefix, not a bare key"
        );
    }

    #[test]
    fn upload_destination_key_named_swaps_in_the_new_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("logs");
        std::fs::create_dir(&nested).unwrap();
        let file = dir.path().join("report.csv");
        std::fs::write(&file, b"hi").unwrap();

        assert_eq!(upload_destination_key_named(&file, "data/", Some("q3.csv")), "data/q3.csv");
        assert_eq!(upload_destination_key_named(&nested, "data/", Some("old-logs")), "data/old-logs/");
    }

    #[test]
    fn collect_local_files_returns_a_single_file_as_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("solo.txt");
        std::fs::write(&file, b"hi").unwrap();

        assert_eq!(collect_local_files(&file), vec![file]);
    }

    #[test]
    fn collect_local_files_recurses_into_subdirectories() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("top.txt"), b"a").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/deep.txt"), b"b").unwrap();

        let mut found = collect_local_files(dir.path());
        found.sort();
        let mut expected = vec![dir.path().join("top.txt"), dir.path().join("nested/deep.txt")];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn copy_local_cancellable_copies_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"payload").unwrap();

        let cancel = AtomicBool::new(false);
        copy_local_cancellable(&src, &dst, &cancel).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(src.exists(), "copy leaves the source in place");
    }

    #[test]
    fn copy_local_cancellable_copies_a_directory_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::create_dir(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/b.txt"), b"b").unwrap();

        let dst = dir.path().join("dst");
        let cancel = AtomicBool::new(false);
        copy_local_cancellable(&src, &dst, &cancel).unwrap();

        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("sub/b.txt")).unwrap(), b"b");
    }

    #[test]
    fn copy_local_cancellable_bails_immediately_when_already_cancelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"payload").unwrap();

        let cancel = AtomicBool::new(true);
        let err = copy_local_cancellable(&src, &dst, &cancel).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert!(!dst.exists());
    }

    #[test]
    fn move_local_cancellable_moves_a_file_and_removes_the_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"payload").unwrap();

        let cancel = AtomicBool::new(false);
        move_local_cancellable(&src, &dst, &cancel).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(!src.exists(), "move removes the source");
    }
}

/// Cancellation of the network jobs, against a provider whose transfers never finish on their
/// own. These jobs used to be spawned with no cancel flag at all, so `esc` reported "cancelling"
/// while the download ran to completion regardless; these pin the behaviour that replaced it.
#[cfg(test)]
mod cancel_tests {
    use super::*;
    use crate::provider::{ObjectMeta, ProgressFn};
    use anyhow::Result;
    use async_trait::async_trait;

    /// Longer than any of these tests wait, so reaching the end of a transfer means the cancel
    /// didn't work rather than the test racing a short sleep.
    async fn stall() {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }

    /// A backend whose transfers write a little and then hang, standing in for a large object
    /// over a slow link: still running when the cancel arrives. `stalls` off makes every
    /// transfer return immediately instead, for the "left alone" case.
    struct TestProvider {
        stalls: bool,
        /// Bytes written before stalling, so a cancelled download has a real partial file to
        /// clean up rather than an empty one.
        partial: Vec<u8>,
    }

    impl TestProvider {
        fn stalling() -> Arc<dyn StorageProvider> {
            Arc::new(Self { stalls: true, partial: vec![b'x'; 512] })
        }

        fn instant() -> Arc<dyn StorageProvider> {
            Arc::new(Self { stalls: false, partial: Vec::new() })
        }
    }

    #[async_trait]
    impl StorageProvider for TestProvider {
        fn diagnostics(&self) -> &[String] {
            &[]
        }

        async fn list_containers(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn list(&self, _container: &str, _prefix: &str) -> Result<Vec<RemoteEntry>> {
            Ok(Vec::new())
        }

        async fn list_all_under(&self, _container: &str, _prefix: &str) -> Result<Vec<RemoteEntry>> {
            Ok(Vec::new())
        }

        async fn stat_size(&self, _container: &str, _key: &str) -> Result<i64> {
            Ok(1_000_000)
        }

        async fn stat_object(&self, _container: &str, _key: &str) -> Result<ObjectMeta> {
            Ok(ObjectMeta { size: 1_000_000, last_modified: None, extra: Vec::new() })
        }

        async fn read_range(&self, _container: &str, _key: &str, _max_bytes: u64) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }

        async fn download(&self, _container: &str, _key: &str, dest: &Path, on_chunk: ProgressFn<'_>) -> Result<()> {
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(dest, &self.partial).await?;
            on_chunk(self.partial.len() as u64);
            if self.stalls {
                stall().await;
            }
            Ok(())
        }

        async fn download_to_writer(
            &self,
            _container: &str,
            _key: &str,
            _writer: &mut (dyn std::io::Write + Send),
            _on_chunk: ProgressFn<'_>,
        ) -> Result<()> {
            if self.stalls {
                stall().await;
            }
            Ok(())
        }

        async fn upload_file(&self, _container: &str, _path: &Path, _key: &str) -> Result<()> {
            if self.stalls {
                stall().await;
            }
            Ok(())
        }

        async fn copy_object(&self, _container: &str, _old_key: &str, _new_key: &str) -> Result<()> {
            unimplemented!("not exercised by these tests")
        }

        async fn delete_object(&self, _container: &str, _key: &str) -> Result<()> {
            unimplemented!("not exercised by these tests")
        }
    }

    /// Waits for the job to report back, failing rather than hanging if nothing arrives. The
    /// cancel flag is polled on a 100ms tick, so the budget clears a round comfortably.
    async fn next_message(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ProgressMsg>) -> ProgressMsg {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the job should report back rather than hang")
            .expect("the job channel should stay open")
    }

    /// Drains progress chatter (`New`, `Advance`) to get at the outcome message.
    async fn final_message(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ProgressMsg>) -> ProgressMsg {
        loop {
            match next_message(rx).await {
                ProgressMsg::New { .. } | ProgressMsg::Advance { .. } => continue,
                other => return other,
            }
        }
    }

    #[tokio::test]
    async fn cancelling_a_download_stops_it_and_removes_the_partial_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        spawn_download_object(
            TestProvider::stalling(),
            1,
            "bucket".to_string(),
            "big.bin".to_string(),
            "big.bin".to_string(),
            dir.path().to_path_buf(),
            cancel.clone(),
            tx,
        );

        // Let it get as far as writing the partial file, so there's something to clean up.
        let dest = dir.path().join("big.bin");
        for _ in 0..50 {
            if dest.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(dest.exists(), "the download should have started writing before being cancelled");

        cancel.store(true, Ordering::Relaxed);

        match final_message(&mut rx).await {
            ProgressMsg::Cancelled { id } => assert_eq!(id, 1),
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(!dest.exists(), "a cancelled download shouldn't leave its half-written file behind");
    }

    #[tokio::test]
    async fn cancelling_an_upload_stops_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("report.csv");
        std::fs::write(&file, b"hello").expect("write source");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        spawn_upload(TestProvider::stalling(), 7, "bucket".to_string(), file, "data/".to_string(), None, cancel.clone(), tx);
        cancel.store(true, Ordering::Relaxed);

        match final_message(&mut rx).await {
            ProgressMsg::Cancelled { id } => assert_eq!(id, 7),
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelling_a_zip_download_stops_it_and_removes_the_partial_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let entries = vec![RemoteEntry {
            key: "logs/a.txt".to_string(),
            name: "a.txt".to_string(),
            is_dir: false,
            size: 10,
            last_modified: None,
            modified_unix: None,
        }];

        spawn_zip_download(
            TestProvider::stalling(),
            3,
            "bucket".to_string(),
            entries,
            "logs/".to_string(),
            "logs.zip".to_string(),
            dir.path().to_path_buf(),
            cancel.clone(),
            tx,
        );
        cancel.store(true, Ordering::Relaxed);

        match final_message(&mut rx).await {
            ProgressMsg::Cancelled { id } => assert_eq!(id, 3),
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(!dir.path().join("logs.zip").exists(), "an unfinished archive isn't a usable file");
    }

    #[tokio::test]
    async fn a_job_left_alone_runs_to_completion() {
        // The mirror of the tests above: nothing sets the flag, so the poller must never fire on
        // its own and turn an ordinary transfer into a cancellation.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("small.txt");
        std::fs::write(&file, b"hi").expect("write source");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_upload(TestProvider::instant(), 9, "bucket".to_string(), file, String::new(), None, cancel, tx);

        match final_message(&mut rx).await {
            ProgressMsg::Done { id, .. } => assert_eq!(id, 9),
            other => panic!("expected Done, got {other:?}"),
        }
    }
}
