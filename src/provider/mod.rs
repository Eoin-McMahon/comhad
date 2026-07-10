//! Storage backend abstraction.
//!
//! comhad talks to remote object stores exclusively through the [`StorageProvider`] trait, so
//! the browser, transfers, preview, and sync layers never name a concrete backend. Adding a new
//! service (Google Cloud Storage, Dropbox, …, Cyberduck-style) is a matter of implementing this
//! trait for a new type and teaching [`connect`] to build it — no UI or app-state changes.

pub mod s3;

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::Connection;

/// A single row in the remote browser: either a "directory" (a common prefix) or an object.
/// Backend-neutral — every provider maps its own listing shape onto this.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    /// Full key/path within the container (directories always end in `/`).
    pub key: String,
    /// Last path segment, for display.
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    /// Last-modified, as an HTTP-date string for display. Object stores expose no separate
    /// "created" or "added" timestamp, so this is the only time axis available.
    pub last_modified: Option<String>,
    /// Last-modified as Unix seconds, for chronological sorting and sync comparison (the
    /// HTTP-date string above doesn't sort lexically). `None` for directories/common-prefixes.
    pub modified_unix: Option<i64>,
}

/// Progress callback passed to streaming transfers: invoked with each chunk's byte count.
/// Boxed rather than generic so the trait stays object-safe (`Arc<dyn StorageProvider>`).
pub type ProgressFn<'a> = &'a (dyn Fn(u64) + Send + Sync);

/// Full object metadata for the info view (`i`) — a superset of what [`RemoteEntry`] carries,
/// fetched on demand via a HEAD-style request rather than upfront for every listed row (most
/// of this is never looked at).
///
/// `size`/`last_modified` are the only fields every backend is expected to have (they're
/// already on [`RemoteEntry`]). Everything else backends actually expose — S3's ETag and
/// Content-Type, say — varies per service, so it's carried as generic label/value pairs
/// rather than named fields the trait would otherwise have to bake in one backend's concepts
/// for. The info view renders `extra` as-is, in order.
#[derive(Debug, Clone)]
pub struct ObjectMeta {
    pub size: i64,
    pub last_modified: Option<String>,
    pub extra: Vec<(String, String)>,
}

/// The operations every backend must provide. Object-safe (via `async_trait`) so it can be held
/// as `Arc<dyn StorageProvider>` and cheaply cloned into background transfer tasks.
///
/// "Container" is the top-level namespace — an S3/GCS bucket. Backends without one (e.g. Dropbox)
/// can treat it as an ignored empty string.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Human-readable notes about how this connection was established (endpoint, region, how the
    /// region was determined). Surfaced in the diagnostics panel.
    fn diagnostics(&self) -> &[String];

    /// Lists top-level containers visible to these credentials. Returning `Ok(empty)` or `Err`
    /// is normal on scoped-down credentials; callers fall back to the bookmark's pinned
    /// container rather than treating it as fatal.
    async fn list_containers(&self) -> Result<Vec<String>>;

    /// Lists the immediate children of `prefix` (non-recursive), directories first.
    async fn list(&self, container: &str, prefix: &str) -> Result<Vec<RemoteEntry>>;

    /// Recursively lists every object (no directories) under `prefix`.
    async fn list_all_under(&self, container: &str, prefix: &str) -> Result<Vec<RemoteEntry>>;

    /// Like [`list_all_under`](Self::list_all_under), but stops paginating as soon as `max`
    /// objects have been collected, rather than always fetching the complete listing before
    /// discarding the excess — for callers (like `/`'s deep filter) that only need "enough"
    /// results fast, not an exact total. The default just lists everything and truncates; a
    /// backend should override this when it can stop fetching pages early instead.
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

    /// Copies `old_key` to `new_key` on the server side, leaving the original in place.
    async fn copy_object(&self, container: &str, old_key: &str, new_key: &str) -> Result<()>;

    /// Copies every object under `old_prefix` to live under `new_prefix` as well, leaving the
    /// originals in place. Provided as a default in terms of [`list_all_under`](Self::list_all_under)
    /// + [`copy_object`](Self::copy_object), so a backend only overrides it if it has a cheaper
    /// native prefix-copy.
    async fn copy_prefix(&self, container: &str, old_prefix: &str, new_prefix: &str) -> Result<()> {
        let objects = self.list_all_under(container, old_prefix).await?;
        for obj in objects {
            let suffix = obj.key.strip_prefix(old_prefix).unwrap_or(&obj.key);
            let new_key = format!("{new_prefix}{suffix}");
            self.copy_object(container, &obj.key, &new_key).await?;
        }
        Ok(())
    }

    /// Permanently removes a single object.
    async fn delete_object(&self, container: &str, key: &str) -> Result<()>;

    /// Permanently removes every object under `prefix`. Provided as a default in terms of
    /// [`list_all_under`](Self::list_all_under) + [`delete_object`](Self::delete_object), so a
    /// backend only overrides it if it has a cheaper native bulk delete.
    async fn delete_prefix(&self, container: &str, prefix: &str) -> Result<()> {
        let objects = self.list_all_under(container, prefix).await?;
        for obj in objects {
            self.delete_object(container, &obj.key).await?;
        }
        Ok(())
    }

    /// Moves a single object from `old_key` to `new_key`: a [`copy_object`](Self::copy_object)
    /// followed by a [`delete_object`](Self::delete_object) of the original.
    async fn rename_object(&self, container: &str, old_key: &str, new_key: &str) -> Result<()> {
        self.copy_object(container, old_key, new_key).await?;
        self.delete_object(container, old_key).await?;
        Ok(())
    }

    /// Moves every object under `old_prefix` to live under `new_prefix` instead. Provided as a
    /// default in terms of [`list_all_under`](Self::list_all_under) + [`rename_object`](Self::rename_object),
    /// so a backend only overrides it if it has a cheaper native prefix-move.
    async fn rename_prefix(&self, container: &str, old_prefix: &str, new_prefix: &str) -> Result<()> {
        let objects = self.list_all_under(container, old_prefix).await?;
        for obj in objects {
            let suffix = obj.key.strip_prefix(old_prefix).unwrap_or(&obj.key);
            let new_key = format!("{new_prefix}{suffix}");
            self.rename_object(container, &obj.key, &new_key).await?;
        }
        Ok(())
    }
}

/// Builds a provider for `conn`, dispatching on the bookmark's `protocol`. This is the single
/// extension point for new backends — everything above the trait stays untouched.
pub async fn connect(conn: &Connection) -> Result<std::sync::Arc<dyn StorageProvider>> {
    // Today every protocol resolves to S3 (plain or PrivateLink); future backends (gcs, dropbox)
    // slot in here.
    let backend = s3::S3Backend::connect(conn).await?;
    Ok(std::sync::Arc::new(backend))
}
