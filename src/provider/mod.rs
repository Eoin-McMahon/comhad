//! Storage backend abstraction.
//!
//! comhad talks to remote object stores through the [`StorageProvider`] trait, so app code
//! never names a concrete backend. New backends implement the trait and register in [`connect`].

pub mod s3;

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::Connection;

/// A single row in the remote browser: either a directory (common prefix) or an object.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    /// Full key/path within the container (directories always end in `/`).
    pub key: String,
    /// Last path segment, for display.
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    /// Last-modified, as an HTTP-date string for display.
    pub last_modified: Option<String>,
    /// Last-modified as Unix seconds, for sorting/sync comparison. `None` for directories.
    pub modified_unix: Option<i64>,
}

/// Progress callback for streaming transfers, invoked with each chunk's byte count.
pub type ProgressFn<'a> = &'a (dyn Fn(u64) + Send + Sync);

/// Full object metadata for the info view (`i`), fetched on demand.
///
/// `extra` carries backend-specific fields (S3's ETag, Content-Type, ...) as label/value
/// pairs rather than named struct fields, so the trait stays backend-neutral.
#[derive(Debug, Clone)]
pub struct ObjectMeta {
    pub size: i64,
    pub last_modified: Option<String>,
    pub extra: Vec<(String, String)>,
}

/// The operations every backend must provide.
///
/// "Container" is the top-level namespace (an S3/GCS bucket); backends without one can
/// treat it as an ignored empty string.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Human-readable connection notes (endpoint, region, ...), shown in the diagnostics panel.
    fn diagnostics(&self) -> &[String];

    /// Lists top-level containers visible to these credentials. `Ok(empty)`/`Err` on
    /// scoped-down credentials is normal; callers fall back to the bookmark's pinned container.
    async fn list_containers(&self) -> Result<Vec<String>>;

    /// Lists the immediate children of `prefix` (non-recursive), directories first.
    async fn list(&self, container: &str, prefix: &str) -> Result<Vec<RemoteEntry>>;

    /// Recursively lists every object (no directories) under `prefix`.
    async fn list_all_under(&self, container: &str, prefix: &str) -> Result<Vec<RemoteEntry>>;

    /// Like [`list_all_under`](Self::list_all_under), but stops once `max` objects are
    /// collected. Default truncates after listing everything; override for early-stop paging.
    async fn list_under_capped(&self, container: &str, prefix: &str, max: usize) -> Result<Vec<RemoteEntry>> {
        let mut all = self.list_all_under(container, prefix).await?;
        all.truncate(max);
        Ok(all)
    }

    /// Returns the size in bytes of a single object.
    async fn stat_size(&self, container: &str, key: &str) -> Result<i64>;

    /// Fetches full metadata for a single object, for the info view (`i`).
    async fn stat_object(&self, container: &str, key: &str) -> Result<ObjectMeta>;

    /// Reads at most `max_bytes` from the start of an object (for the preview pane).
    async fn read_range(&self, container: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>>;

    /// Streams a single object to `dest`, invoking `on_chunk` with each chunk's byte count.
    async fn download(&self, container: &str, key: &str, dest: &Path, on_chunk: ProgressFn<'_>) -> Result<()>;

    /// Streams a single object chunk-by-chunk to a synchronous writer (e.g. a zip archive).
    async fn download_to_writer(
        &self,
        container: &str,
        key: &str,
        writer: &mut (dyn std::io::Write + Send),
        on_chunk: ProgressFn<'_>,
    ) -> Result<()>;

    /// Uploads a local file to `key`.
    async fn upload_file(&self, container: &str, path: &Path, key: &str) -> Result<()>;

    /// Generates a time-limited public URL for an object, valid for `expires_in`. `Ok(None)`
    /// means this backend has no such concept; defaults to `None`.
    async fn share_url(&self, container: &str, key: &str, expires_in: std::time::Duration) -> Result<Option<String>> {
        let _ = (container, key, expires_in);
        Ok(None)
    }

    /// Copies `old_key` to `new_key` on the server side, leaving the original in place.
    async fn copy_object(&self, container: &str, old_key: &str, new_key: &str) -> Result<()>;

    /// Permanently removes a single object.
    async fn delete_object(&self, container: &str, key: &str) -> Result<()>;

    /// Permanently removes every object under `prefix`. Override if a cheaper native bulk
    /// delete exists; the default lists and deletes one by one.
    async fn delete_prefix(&self, container: &str, prefix: &str) -> Result<()> {
        let objects = self.list_all_under(container, prefix).await?;
        for obj in objects {
            self.delete_object(container, &obj.key).await?;
        }
        Ok(())
    }

    /// Moves an object: copy to `new_key` then delete the original.
    async fn rename_object(&self, container: &str, old_key: &str, new_key: &str) -> Result<()> {
        self.copy_object(container, old_key, new_key).await?;
        self.delete_object(container, old_key).await?;
        Ok(())
    }
}

/// Builds a provider for `conn`, dispatching on the bookmark's `protocol`.
pub async fn connect(conn: &Connection) -> Result<std::sync::Arc<dyn StorageProvider>> {
    let backend = s3::S3Backend::connect(conn).await?;
    Ok(std::sync::Arc::new(backend))
}
