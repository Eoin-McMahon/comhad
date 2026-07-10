use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::provider::{RemoteEntry, StorageProvider};

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Download,
    Upload,
    Zip,
}

#[derive(Debug, Clone)]
pub enum JobStatus {
    Running,
    Done,
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
    /// Local filesystem path this job reads from (upload) or writes to (download/zip) — lets
    /// the transfers pane open or reveal the file once the job is done.
    pub local_path: PathBuf,
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
    Failed {
        id: JobId,
        error: String,
    },
}

/// Downloads a single object straight into `dest_dir` (the local pane's current directory),
/// reporting progress as it streams.
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
///
/// `entries` are files only (directories must already be expanded by the caller via
/// `list_all_under`); `strip_prefix` is removed from each key to compute the path stored
/// inside the archive.
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

/// Uploads a single local file to an exact `key` (as opposed to [`spawn_upload`], which derives
/// keys from a prefix + directory structure). Used by sync, which has already computed the
/// precise destination key for each file.
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
