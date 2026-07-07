use std::path::Path;

use anyhow::{Context, Result};
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use crate::config::Connection;

/// A single row in the browser: either a "directory" (a common prefix) or an object.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Full S3 key (directories always end in `/`).
    pub key: String,
    /// Last path segment, for display.
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub last_modified: Option<String>,
}

#[derive(Clone)]
pub struct S3Client {
    client: Client,
    /// Human-readable notes about how this client was set up (endpoint, region, how the
    /// region was determined) — surfaced in the diagnostics panel, not just used for logic.
    pub diagnostics: Vec<String>,
}

impl S3Client {
    pub async fn connect(conn: &Connection) -> Result<Self> {
        let (bucket, _) = conn.bucket_and_prefix();
        let mut diagnostics = vec![format!("bucket: {bucket}")];

        let region = match &conn.region {
            Some(r) => {
                diagnostics.push(format!("region: {r} (from bookmark's \"region\" field)"));
                r.clone()
            }
            None => match detect_bucket_region(conn, &bucket).await {
                Ok(r) => {
                    diagnostics.push(format!("region: {r} (auto-detected via x-amz-bucket-region)"));
                    r
                }
                Err(err) => {
                    diagnostics.push(format!(
                        "region auto-detection FAILED, falling back to us-east-1: {err:#}"
                    ));
                    "us-east-1".to_string()
                }
            },
        };

        let endpoint = effective_endpoint(conn, &region);
        diagnostics.insert(0, format!("endpoint: {endpoint}"));

        let client = build_client(conn, &endpoint, &region);
        Ok(Self { client, diagnostics })
    }

    /// Lists every bucket visible to these credentials. Fails (often due to a scoped-down
    /// IAM policy without `s3:ListAllMyBuckets`) on many real-world setups, which callers
    /// should treat as "fall back to the bookmark's pinned bucket", not a hard error.
    pub async fn list_buckets(&self) -> Result<Vec<String>> {
        let resp = self.client.list_buckets().send().await.context("failed to list buckets")?;
        Ok(resp
            .buckets()
            .iter()
            .filter_map(|b| b.name().map(|s| s.to_string()))
            .collect())
    }

    /// Lists the immediate children of `prefix` (non-recursive), directories first.
    pub async fn list(&self, bucket: &str, prefix: &str) -> Result<Vec<Entry>> {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(bucket)
                .prefix(prefix)
                .delimiter("/");
            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }
            let resp = req
                .send()
                .await
                .with_context(|| format!("failed to list {bucket}/{prefix}"))?;

            for common in resp.common_prefixes() {
                if let Some(p) = common.prefix() {
                    let name = p
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or(p)
                        .to_string();
                    dirs.push(Entry {
                        key: p.to_string(),
                        name,
                        is_dir: true,
                        size: 0,
                        last_modified: None,
                    });
                }
            }

            for obj in resp.contents() {
                let key = obj.key().unwrap_or_default().to_string();
                if key == prefix {
                    continue;
                }
                let name = key.rsplit('/').next().unwrap_or(&key).to_string();
                files.push(Entry {
                    key,
                    name,
                    is_dir: false,
                    size: obj.size().unwrap_or(0),
                    last_modified: obj
                        .last_modified()
                        .and_then(|t| t.fmt(aws_sdk_s3::primitives::DateTimeFormat::HttpDate).ok()),
                });
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        dirs.extend(files);
        Ok(dirs)
    }

    /// Recursively lists every object (no directories) under `prefix`.
    pub async fn list_all_under(&self, bucket: &str, prefix: &str) -> Result<Vec<Entry>> {
        let mut files = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut req = self.client.list_objects_v2().bucket(bucket).prefix(prefix);
            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }
            let resp = req
                .send()
                .await
                .with_context(|| format!("failed to list {bucket}/{prefix}"))?;

            for obj in resp.contents() {
                let key = obj.key().unwrap_or_default().to_string();
                let name = key.rsplit('/').next().unwrap_or(&key).to_string();
                files.push(Entry {
                    key,
                    name,
                    is_dir: false,
                    size: obj.size().unwrap_or(0),
                    last_modified: obj
                        .last_modified()
                        .and_then(|t| t.fmt(aws_sdk_s3::primitives::DateTimeFormat::HttpDate).ok()),
                });
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(files)
    }

    /// Copies `old_key` to `new_key` on the server side, then removes the original. This is
    /// the only way an object is ever removed from the bucket — comhad exposes no standalone
    /// delete action.
    pub async fn rename_object(&self, bucket: &str, old_key: &str, new_key: &str) -> Result<()> {
        let source = format!(
            "{}/{}",
            urlencoding::encode(bucket),
            urlencoding::encode(old_key).replace("%2F", "/")
        );
        self.client
            .copy_object()
            .bucket(bucket)
            .copy_source(source)
            .key(new_key)
            .send()
            .await
            .with_context(|| format!("failed to copy {old_key} to {new_key}"))?;
        self.client
            .delete_object()
            .bucket(bucket)
            .key(old_key)
            .send()
            .await
            .with_context(|| format!("failed to remove old key {old_key} after rename"))?;
        Ok(())
    }

    /// Renames every object under `old_prefix` to live under `new_prefix` instead.
    pub async fn rename_prefix(&self, bucket: &str, old_prefix: &str, new_prefix: &str) -> Result<()> {
        let objects = self.list_all_under(bucket, old_prefix).await?;
        for obj in objects {
            let suffix = obj.key.strip_prefix(old_prefix).unwrap_or(&obj.key);
            let new_key = format!("{new_prefix}{suffix}");
            self.rename_object(bucket, &obj.key, &new_key).await?;
        }
        Ok(())
    }

    pub async fn upload_file(&self, bucket: &str, local_path: &Path, key: &str) -> Result<()> {
        let body = ByteStream::from_path(local_path)
            .await
            .with_context(|| format!("failed to open {}", local_path.display()))?;
        self.client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(body)
            .send()
            .await
            .with_context(|| format!("failed to upload to {key}"))?;
        Ok(())
    }

    pub async fn head_size(&self, bucket: &str, key: &str) -> Result<i64> {
        let resp = self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to stat {key}"))?;
        Ok(resp.content_length().unwrap_or(0))
    }

    /// Reads at most `max_bytes` from the start of an object, for the preview pane. Uses an
    /// HTTP Range request so previewing a multi-GB object doesn't pull the whole thing down.
    pub async fn read_preview(&self, bucket: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let range = format!("bytes=0-{}", max_bytes.saturating_sub(1));
        let resp = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .range(range)
            .send()
            .await
            .with_context(|| format!("failed to read {key}"))?;
        let bytes = resp
            .body
            .collect()
            .await
            .with_context(|| format!("failed to read body of {key}"))?
            .into_bytes();
        Ok(bytes.to_vec())
    }

    /// Streams a single object to `dest`, invoking `on_chunk` with each chunk's byte count.
    pub async fn download_object(
        &self,
        bucket: &str,
        key: &str,
        dest: &Path,
        mut on_chunk: impl FnMut(u64),
    ) -> Result<()> {
        let resp = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to start download of {key}"))?;

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .with_context(|| format!("failed to create {}", dest.display()))?;
        let mut body = resp.body;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = body.try_next().await? {
            on_chunk(chunk.len() as u64);
            file.write_all(&chunk).await?;
        }
        Ok(())
    }

    /// Streams a single object's bytes chunk-by-chunk to a synchronous writer (used for
    /// writing straight into a zip archive without buffering the whole object in memory).
    pub async fn download_object_to_writer(
        &self,
        bucket: &str,
        key: &str,
        mut writer: impl std::io::Write,
        mut on_chunk: impl FnMut(u64),
    ) -> Result<()> {
        let resp = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to start download of {key}"))?;
        let mut body = resp.body;
        while let Some(chunk) = body.try_next().await? {
            on_chunk(chunk.len() as u64);
            writer.write_all(&chunk)?;
        }
        Ok(())
    }
}

fn build_client(conn: &Connection, endpoint: &str, region: &str) -> Client {
    let creds = Credentials::new(&conn.access_key_id, &conn.secret_access_key, None, None, "comhad");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(region.to_string()))
        .credentials_provider(creds)
        .endpoint_url(endpoint)
        .force_path_style(conn.force_path_style())
        .build();
    Client::from_conf(config)
}

/// S3's global `s3.amazonaws.com` endpoint only transparently serves `us-east-1` buckets —
/// for any other region it answers with a `PermanentRedirect` telling you to use the
/// region-specific endpoint instead, *even if* your request was already signed for the
/// correct region. Cyberduck's bookmark shows the generic host but actually issues requests
/// against the discovered regional endpoint; we do the same, and leave any other server
/// (custom S3-compatible endpoints, PrivateLink DNS names, an already region-specific
/// hostname) untouched since those aren't sharded the same way.
fn effective_endpoint(conn: &Connection, region: &str) -> String {
    let endpoint = conn.endpoint_url();
    if endpoint == "https://s3.amazonaws.com" {
        format!("https://s3.{region}.amazonaws.com")
    } else {
        endpoint
    }
}

/// Cyberduck's generic S3 profile never asks for a region either — it discovers the
/// bucket's actual region from the `x-amz-bucket-region` response header, which S3 sends
/// back on a plain, *unauthenticated* HEAD request regardless of whether the caller's
/// credentials would actually be allowed to read the bucket. This deliberately avoids
/// `GetBucketLocation`, which needs its own IAM permission that scoped-down policies
/// (allowing only `GetObject`/`PutObject`/`ListBucket` on a specific prefix, say) often don't
/// grant — using it would silently fall back to a wrong region and fail every real request
/// with an opaque signature error.
async fn detect_bucket_region(conn: &Connection, bucket: &str) -> Result<String> {
    let url = format!("{}/{}", conn.endpoint_url(), bucket);
    let resp = reqwest::Client::new()
        .head(&url)
        .send()
        .await
        .with_context(|| format!("failed to probe region for {bucket} at {url}"))?;
    let region = resp
        .headers()
        .get("x-amz-bucket-region")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .with_context(|| format!("{url} did not return an x-amz-bucket-region header (status {})", resp.status()))?;
    Ok(region)
}
