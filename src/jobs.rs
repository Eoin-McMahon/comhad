use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use futures_util::{StreamExt, TryStreamExt, stream};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::provider::{ProgressFn, RemoteEntry, StorageProvider};

pub type JobId = u64;

/// How many objects a directory job works on at once.
const PARALLEL_REQUESTS: usize = 8;

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
    /// Whether `local_path` is the destination. The side being written shows the progress bar.
    pub fn writes_local(self) -> bool {
        matches!(self, JobKind::Download | JobKind::Zip | JobKind::LocalCopy | JobKind::LocalMove)
    }

    /// Whether `remote_key` is the destination. A delete writes to neither side.
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
    /// 0 means unknown, so the row shows a spinner instead of a percentage.
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub status: JobStatus,
    /// Opened or revealed from the jobs pane. Empty for remote-to-remote jobs.
    pub local_path: PathBuf,
    /// The remote row this job belongs to. For a write it's the destination key.
    pub remote_key: Option<String>,
    pub cancel: CancellationToken,
    /// What retry runs again. `None` for moves and deletes: once part of the source is gone,
    /// running them again from the start is a different operation.
    pub spec: Option<JobSpec>,
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

/// The retryable jobs, described well enough to start them again.
#[derive(Debug, Clone)]
pub enum JobSpec {
    Download { bucket: String, key: String, label: String, dest_dir: PathBuf },
    Zip { bucket: String, entries: Vec<RemoteEntry>, strip_prefix: String, zip_name: String, dest_dir: PathBuf },
    /// `name` replaces the local file or directory name in the destination key.
    Upload { bucket: String, local_path: PathBuf, key_prefix: String, name: Option<String> },
    UploadFile { bucket: String, local_path: PathBuf, key: String },
}

#[derive(Debug)]
pub enum ProgressMsg {
    New { id: JobId, label: String, kind: JobKind, total_bytes: u64 },
    Advance { id: JobId, delta: u64 },
    Done { id: JobId, kind: JobKind },
    Cancelled { id: JobId },
    Failed { id: JobId, error: String },
}

enum Outcome {
    Done,
    Cancelled,
    Failed(String),
}

/// What a running job needs to report back to the app.
#[derive(Clone)]
pub struct JobContext {
    pub id: JobId,
    pub cancel: CancellationToken,
    pub tx: UnboundedSender<ProgressMsg>,
}

impl JobContext {
    fn start(&self, label: String, kind: JobKind, total_bytes: u64) {
        let _ = self.tx.send(ProgressMsg::New { id: self.id, label, kind, total_bytes });
    }

    fn advance(&self, bytes: u64) {
        let _ = self.tx.send(ProgressMsg::Advance { id: self.id, delta: bytes });
    }

    fn finish(&self, kind: JobKind, outcome: Outcome) {
        let id = self.id;
        let msg = match outcome {
            Outcome::Done => ProgressMsg::Done { id, kind },
            Outcome::Cancelled => ProgressMsg::Cancelled { id },
            Outcome::Failed(error) => ProgressMsg::Failed { id, error },
        };
        let _ = self.tx.send(msg);
    }

    /// Run `work`, dropping it as soon as the job is cancelled.
    async fn run(&self, work: impl Future<Output = anyhow::Result<()>>) -> Outcome {
        match self.cancel.run_until_cancelled(work).await {
            Some(Ok(())) => Outcome::Done,
            Some(Err(err)) => Outcome::Failed(err.to_string()),
            None => Outcome::Cancelled,
        }
    }

    /// For jobs that check the token themselves and stop at a safe point instead of being dropped.
    fn outcome_of(&self, result: anyhow::Result<()>) -> Outcome {
        match result {
            Err(err) => Outcome::Failed(err.to_string()),
            Ok(()) if self.cancel.is_cancelled() => Outcome::Cancelled,
            Ok(()) => Outcome::Done,
        }
    }
}

/// Start the job described by `spec`. Used for both the first run and a retry.
pub fn spawn(client: Arc<dyn StorageProvider>, ctx: JobContext, spec: &JobSpec) {
    let spec = spec.clone();
    tokio::spawn(async move {
        let (kind, outcome) = match spec {
            JobSpec::Download { bucket, key, label, dest_dir } => {
                (JobKind::Download, download(&client, &ctx, &bucket, &key, label, &dest_dir).await)
            }
            JobSpec::Zip { bucket, entries, strip_prefix, zip_name, dest_dir } => {
                let dest = dest_dir.join(&zip_name);
                ctx.start(zip_name, JobKind::Zip, entries.iter().map(|e| e.size.max(0) as u64).sum());
                (JobKind::Zip, zip_download(client, &ctx, bucket, entries, strip_prefix, dest).await)
            }
            JobSpec::Upload { bucket, local_path, key_prefix, name } => {
                (JobKind::Upload, upload(&client, &ctx, &bucket, &local_path, &key_prefix, name.as_deref()).await)
            }
            JobSpec::UploadFile { bucket, local_path, key } => {
                (JobKind::Upload, upload_file(&client, &ctx, &bucket, &local_path, &key).await)
            }
        };
        ctx.finish(kind, outcome);
    });
}

async fn download(
    client: &Arc<dyn StorageProvider>,
    ctx: &JobContext,
    bucket: &str,
    key: &str,
    label: String,
    dest_dir: &Path,
) -> Outcome {
    let total = client.stat_size(bucket, key).await.unwrap_or(0).max(0) as u64;
    let dest = dest_dir.join(&label);
    ctx.start(label, JobKind::Download, total);

    let outcome = ctx.run(client.download(bucket, key, &dest, &|n| ctx.advance(n))).await;
    if matches!(outcome, Outcome::Cancelled) {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    outcome
}

/// One step of building a zip archive, sent from the downloads to the thread writing the file.
enum ZipPart {
    StartFile(String),
    Data(Vec<u8>),
}

/// Lets the provider's download write straight into the zip channel.
struct ZipPartSender(mpsc::Sender<ZipPart>);

impl Write for ZipPartSender {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .send(ZipPart::Data(buf.to_vec()))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "zip writer stopped"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Download every entry into one zip archive at `dest`. The archive is written on a blocking
/// thread so file I/O never stalls the async runtime.
async fn zip_download(
    client: Arc<dyn StorageProvider>,
    ctx: &JobContext,
    bucket: String,
    entries: Vec<RemoteEntry>,
    strip_prefix: String,
    dest: PathBuf,
) -> Outcome {
    let parent = dest.parent().unwrap_or(Path::new("."));
    if let Err(err) = tokio::fs::create_dir_all(parent).await {
        return Outcome::Failed(err.to_string());
    }

    // Unbounded so a write never blocks the async side. The disk keeps up with the network.
    let (parts_tx, parts_rx) = mpsc::channel();
    let writer = tokio::task::spawn_blocking({
        let dest = dest.clone();
        move || write_zip(&dest, parts_rx)
    });

    let send_parts = async move {
        let mut sender = ZipPartSender(parts_tx);
        for entry in &entries {
            let name = entry.key.strip_prefix(&strip_prefix).unwrap_or(&entry.key);
            sender.0.send(ZipPart::StartFile(name.to_string()))?;
            client.download_to_writer(&bucket, &entry.key, &mut sender, &|n| ctx.advance(n)).await?;
        }
        anyhow::Ok(())
    };
    let sent = ctx.cancel.run_until_cancelled(send_parts).await;

    // The sender is dropped by now, so the writer thread sees the end of the channel and returns.
    let written = writer.await.unwrap_or_else(|err| Err(err.into()));

    // If the writer failed, the send failed because of it, so its error is the useful one.
    let outcome = match sent {
        None => Outcome::Cancelled,
        Some(sent) => match written.and(sent) {
            Ok(()) => Outcome::Done,
            Err(err) => Outcome::Failed(err.to_string()),
        },
    };
    if !matches!(outcome, Outcome::Done) {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    outcome
}

fn write_zip(dest: &Path, parts: mpsc::Receiver<ZipPart>) -> anyhow::Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(dest)?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for part in parts {
        match part {
            ZipPart::StartFile(name) => zip.start_file(name, options)?,
            ZipPart::Data(bytes) => zip.write_all(&bytes)?,
        }
    }
    zip.finish()?;
    Ok(())
}

/// The key an upload of `local_path` into `key_prefix` shows up as in that prefix's listing:
/// the object for a file, or the directory prefix for a directory.
pub fn upload_destination_key(local_path: &Path, key_prefix: &str) -> String {
    upload_destination_key_named(local_path, key_prefix, None)
}

/// Like [`upload_destination_key`], with `name` in place of the local name.
pub fn upload_destination_key_named(local_path: &Path, key_prefix: &str, name: Option<&str>) -> String {
    let name = match name {
        Some(name) => name.to_string(),
        None => file_label(local_path),
    };
    let suffix = if local_path.is_dir() { "/" } else { "" };
    format!("{key_prefix}{name}{suffix}")
}

/// The key one file inside an upload of `root` lands on.
fn upload_key(file: &Path, root: &Path, key_prefix: &str, name: Option<&str>) -> String {
    let relative = match name {
        Some(name) => match file.strip_prefix(root) {
            Ok(inner) if !inner.as_os_str().is_empty() => format!("{name}/{}", inner.to_string_lossy()),
            _ => name.to_string(),
        },
        None => {
            let base = root.parent().unwrap_or(root);
            file.strip_prefix(base).unwrap_or(file).to_string_lossy().to_string()
        }
    };
    format!("{key_prefix}{}", relative.replace('\\', "/"))
}

/// Upload a file, or every file under a directory, a few at a time. Files that finished before a
/// cancel stay uploaded.
async fn upload(
    client: &Arc<dyn StorageProvider>,
    ctx: &JobContext,
    bucket: &str,
    local_path: &Path,
    key_prefix: &str,
    name: Option<&str>,
) -> Outcome {
    let root = local_path.to_path_buf();
    let walk = tokio::task::spawn_blocking(move || {
        let files = collect_local_files(&root);
        let total: u64 = files.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
        (files, total)
    });
    let (files, total) = match walk.await {
        Ok(found) => found,
        Err(err) => return Outcome::Failed(err.to_string()),
    };
    let label = name.map(str::to_string).unwrap_or_else(|| file_label(local_path));
    ctx.start(label, JobKind::Upload, total);

    let on_chunk: ProgressFn<'_> = &|n| ctx.advance(n);
    let uploads = stream::iter(files)
        .map(|file| {
            let key = upload_key(&file, local_path, key_prefix, name);
            async move { client.upload_file(bucket, &file, &key, on_chunk).await }
        })
        .buffer_unordered(PARALLEL_REQUESTS)
        .try_collect::<()>();
    ctx.run(uploads).await
}

/// Upload one file to an exact key. Sync and the editor use this, having worked out the key.
async fn upload_file(
    client: &Arc<dyn StorageProvider>,
    ctx: &JobContext,
    bucket: &str,
    local_path: &Path,
    key: &str,
) -> Outcome {
    let size = tokio::fs::metadata(local_path).await.map(|m| m.len()).unwrap_or(0);
    ctx.start(file_label(local_path), JobKind::Upload, size);
    ctx.run(client.upload_file(bucket, local_path, key, &|n| ctx.advance(n))).await
}

/// Copy, or move if `delete_source`, a local file or directory. Progress is indeterminate:
/// sizing the tree first would cost as much as the copy.
pub fn spawn_local_transfer(ctx: JobContext, src: PathBuf, dest: PathBuf, delete_source: bool) {
    let kind = if delete_source { JobKind::LocalMove } else { JobKind::LocalCopy };
    ctx.start(file_label(&src), kind, 0);

    tokio::task::spawn_blocking(move || {
        let result = if delete_source {
            move_local(&src, &dest, &ctx.cancel)
        } else {
            copy_local(&src, &dest, &ctx.cancel)
        };
        let outcome = match result {
            Ok(()) => Outcome::Done,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => Outcome::Cancelled,
            Err(err) => Outcome::Failed(err.to_string()),
        };
        ctx.finish(kind, outcome);
    });
}

fn cancelled_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")
}

/// Copy `src` to `dst`, stopping between files once `cancel` is set.
fn copy_local(src: &Path, dst: &Path, cancel: &CancellationToken) -> std::io::Result<()> {
    if cancel.is_cancelled() {
        return Err(cancelled_error());
    }
    if !src.is_dir() {
        return std::fs::copy(src, dst).map(|_| ());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        copy_local(&entry.path(), &dst.join(entry.file_name()), cancel)?;
    }
    Ok(())
}

fn move_local(src: &Path, dst: &Path, cancel: &CancellationToken) -> std::io::Result<()> {
    if cancel.is_cancelled() {
        return Err(cancelled_error());
    }
    // `rename` fails across filesystems, so fall back to copy and delete.
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_local(src, dst, cancel)?;
    if src.is_dir() {
        std::fs::remove_dir_all(src)
    } else {
        std::fs::remove_file(src)
    }
}

/// Copy, or move if `delete_source`, one object or every object under a prefix (a key ending in
/// `/`). Cancelling stops new objects from starting but lets the ones in flight finish, so a
/// move is never cut off between its copy and its delete.
pub fn spawn_remote_transfer(
    client: Arc<dyn StorageProvider>,
    ctx: JobContext,
    bucket: String,
    old_key: String,
    new_key: String,
    delete_source: bool,
) {
    let kind = if delete_source { JobKind::RemoteMove } else { JobKind::RemoteCopy };
    ctx.start(key_label(&old_key), kind, 0);

    tokio::spawn(async move {
        let result = remote_transfer(&client, &ctx.cancel, &bucket, &old_key, &new_key, delete_source).await;
        ctx.finish(kind, ctx.outcome_of(result));
    });
}

async fn remote_transfer(
    client: &Arc<dyn StorageProvider>,
    cancel: &CancellationToken,
    bucket: &str,
    old_key: &str,
    new_key: &str,
    delete_source: bool,
) -> anyhow::Result<()> {
    let sources = if old_key.ends_with('/') {
        client.list_all_under(bucket, old_key).await?.into_iter().map(|obj| obj.key).collect()
    } else {
        vec![old_key.to_string()]
    };

    stream::iter(sources)
        .map(|source| async move {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let dest = format!("{new_key}{}", source.strip_prefix(old_key).unwrap_or(&source));
            if delete_source {
                client.rename_object(bucket, &source, &dest).await
            } else {
                client.copy_object(bucket, &source, &dest).await
            }
        })
        .buffer_unordered(PARALLEL_REQUESTS)
        .try_collect()
        .await
}

/// Delete every object under `prefix`. Cancelling works the same way as for a remote transfer.
pub fn spawn_remote_delete(client: Arc<dyn StorageProvider>, ctx: JobContext, bucket: String, prefix: String) {
    ctx.start(key_label(&prefix), JobKind::RemoteDelete, 0);

    tokio::spawn(async move {
        let result = remote_delete(&client, &ctx.cancel, &bucket, &prefix).await;
        ctx.finish(JobKind::RemoteDelete, ctx.outcome_of(result));
    });
}

async fn remote_delete(
    client: &Arc<dyn StorageProvider>,
    cancel: &CancellationToken,
    bucket: &str,
    prefix: &str,
) -> anyhow::Result<()> {
    let objects = client.list_all_under(bucket, prefix).await?;
    stream::iter(objects)
        .map(|obj| async move {
            if cancel.is_cancelled() {
                return Ok(());
            }
            client.delete_object(bucket, &obj.key).await
        })
        .buffer_unordered(PARALLEL_REQUESTS)
        .try_collect()
        .await
}

fn file_label(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => path.display().to_string(),
    }
}

fn key_label(key: &str) -> String {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or(key).to_string()
}

fn collect_local_files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut files = Vec::new();
    let mut dirs = vec![path.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(total_bytes: u64, done_bytes: u64) -> Job {
        Job {
            id: 1,
            label: "x".into(),
            kind: JobKind::Download,
            total_bytes,
            done_bytes,
            status: JobStatus::Running,
            local_path: PathBuf::new(),
            cancel: CancellationToken::new(),
            remote_key: None,
            spec: None,
        }
    }

    #[test]
    fn progress_ratio_is_zero_when_total_is_unknown() {
        assert_eq!(job(0, 40).progress_ratio(), 0.0);
    }

    #[test]
    fn progress_ratio_divides_done_by_total() {
        assert_eq!(job(200, 50).progress_ratio(), 0.25);
        assert_eq!(job(200, 500).progress_ratio(), 1.0, "overshooting done_bytes clamps to 1.0");
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

        assert_eq!(upload_destination_key(&nested, "data/"), "data/logs/");
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
    fn upload_key_keeps_the_directory_name_or_swaps_in_the_new_one() {
        let root = Path::new("/home/me/logs");
        let file = Path::new("/home/me/logs/2026/a.txt");

        assert_eq!(upload_key(file, root, "data/", None), "data/logs/2026/a.txt");
        assert_eq!(upload_key(file, root, "data/", Some("old")), "data/old/2026/a.txt");
        assert_eq!(upload_key(root, root, "data/", Some("old")), "data/old");
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
    fn copy_local_copies_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"payload").unwrap();

        copy_local(&src, &dst, &CancellationToken::new()).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(src.exists(), "copy leaves the source in place");
    }

    #[test]
    fn copy_local_copies_a_directory_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::create_dir(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/b.txt"), b"b").unwrap();

        let dst = dir.path().join("dst");
        copy_local(&src, &dst, &CancellationToken::new()).unwrap();

        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("sub/b.txt")).unwrap(), b"b");
    }

    #[test]
    fn copy_local_stops_when_already_cancelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"payload").unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = copy_local(&src, &dst, &cancel).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert!(!dst.exists());
    }

    #[test]
    fn move_local_moves_a_file_and_removes_the_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"payload").unwrap();

        move_local(&src, &dst, &CancellationToken::new()).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(!src.exists(), "move removes the source");
    }
}

/// Cancellation of the network jobs, against a provider whose transfers never finish.
#[cfg(test)]
mod cancel_tests {
    use std::time::Duration;

    use super::*;
    use crate::provider::ObjectMeta;
    use anyhow::Result;
    use async_trait::async_trait;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    /// Longer than any test waits, so reaching the end means the cancel didn't work.
    async fn stall() {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }

    /// Writes `partial` and then hangs if `stalls`, like a large object on a slow link.
    struct TestProvider {
        stalls: bool,
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
            writer: &mut (dyn std::io::Write + Send),
            _on_chunk: ProgressFn<'_>,
        ) -> Result<()> {
            writer.write_all(&self.partial)?;
            if self.stalls {
                stall().await;
            }
            Ok(())
        }

        async fn upload_file(&self, _container: &str, _path: &Path, _key: &str, _on_chunk: ProgressFn<'_>) -> Result<()> {
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

    fn context(id: JobId) -> (JobContext, UnboundedReceiver<ProgressMsg>) {
        let (tx, rx) = unbounded_channel();
        (JobContext { id, cancel: CancellationToken::new(), tx }, rx)
    }

    /// Skip progress messages and return how the job ended, failing rather than hanging.
    async fn final_message(rx: &mut UnboundedReceiver<ProgressMsg>) -> ProgressMsg {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("the job should report back rather than hang")
                .expect("the job channel should stay open");
            if !matches!(msg, ProgressMsg::New { .. } | ProgressMsg::Advance { .. }) {
                return msg;
            }
        }
    }

    #[tokio::test]
    async fn cancelling_a_download_stops_it_and_removes_the_partial_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ctx, mut rx) = context(1);
        let cancel = ctx.cancel.clone();
        let spec = JobSpec::Download {
            bucket: "bucket".to_string(),
            key: "big.bin".to_string(),
            label: "big.bin".to_string(),
            dest_dir: dir.path().to_path_buf(),
        };
        spawn(TestProvider::stalling(), ctx, &spec);

        let dest = dir.path().join("big.bin");
        for _ in 0..50 {
            if dest.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(dest.exists(), "the download should have started writing before being cancelled");

        cancel.cancel();

        assert!(matches!(final_message(&mut rx).await, ProgressMsg::Cancelled { id: 1 }));
        assert!(!dest.exists(), "a cancelled download shouldn't leave its partial file behind");
    }

    #[tokio::test]
    async fn cancelling_an_upload_stops_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("report.csv");
        std::fs::write(&file, b"hello").expect("write source");

        let (ctx, mut rx) = context(7);
        let cancel = ctx.cancel.clone();
        let spec = JobSpec::Upload {
            bucket: "bucket".to_string(),
            local_path: file,
            key_prefix: "data/".to_string(),
            name: None,
        };
        spawn(TestProvider::stalling(), ctx, &spec);
        cancel.cancel();

        assert!(matches!(final_message(&mut rx).await, ProgressMsg::Cancelled { id: 7 }));
    }

    #[tokio::test]
    async fn cancelling_a_zip_download_stops_it_and_removes_the_partial_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ctx, mut rx) = context(3);
        let cancel = ctx.cancel.clone();
        let entries = vec![RemoteEntry {
            key: "logs/a.txt".to_string(),
            name: "a.txt".to_string(),
            is_dir: false,
            size: 10,
            last_modified: None,
            modified_unix: None,
        }];
        let spec = JobSpec::Zip {
            bucket: "bucket".to_string(),
            entries,
            strip_prefix: "logs/".to_string(),
            zip_name: "logs.zip".to_string(),
            dest_dir: dir.path().to_path_buf(),
        };
        spawn(TestProvider::stalling(), ctx, &spec);
        cancel.cancel();

        assert!(matches!(final_message(&mut rx).await, ProgressMsg::Cancelled { id: 3 }));
        assert!(!dir.path().join("logs.zip").exists(), "an unfinished archive isn't a usable file");
    }

    #[tokio::test]
    async fn a_zip_download_writes_every_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ctx, mut rx) = context(4);
        let entry = |key: &str| RemoteEntry {
            key: key.to_string(),
            name: String::new(),
            is_dir: false,
            size: 0,
            last_modified: None,
            modified_unix: None,
        };
        let spec = JobSpec::Zip {
            bucket: "bucket".to_string(),
            entries: vec![entry("logs/a.txt"), entry("logs/sub/b.txt")],
            strip_prefix: "logs/".to_string(),
            zip_name: "logs.zip".to_string(),
            dest_dir: dir.path().to_path_buf(),
        };
        let provider = Arc::new(TestProvider { stalls: false, partial: b"hello".to_vec() });
        spawn(provider, ctx, &spec);

        assert!(matches!(final_message(&mut rx).await, ProgressMsg::Done { id: 4, .. }));
        let archive = std::fs::File::open(dir.path().join("logs.zip")).expect("archive exists");
        let mut archive = zip::ZipArchive::new(archive).expect("archive is valid");
        let mut names: Vec<_> = archive.file_names().map(str::to_string).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "sub/b.txt"]);
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("sub/b.txt").unwrap(), &mut contents).unwrap();
        assert_eq!(contents, "hello");
    }

    #[tokio::test]
    async fn a_job_left_alone_runs_to_completion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("small.txt");
        std::fs::write(&file, b"hi").expect("write source");

        let (ctx, mut rx) = context(9);
        let spec = JobSpec::Upload { bucket: "bucket".to_string(), local_path: file, key_prefix: String::new(), name: None };
        spawn(TestProvider::instant(), ctx, &spec);

        assert!(matches!(final_message(&mut rx).await, ProgressMsg::Done { id: 9, .. }));
    }
}
