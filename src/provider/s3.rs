//! S3 (and S3-compatible / PrivateLink) implementation of [`StorageProvider`].

use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use crate::config::Connection;
use crate::provider::{ProgressFn, RemoteEntry, StorageProvider};

pub struct S3Backend {
    client: Client,
    /// Human-readable notes about how this client was set up (endpoint, region, how the
    /// region was determined) — surfaced in the diagnostics panel, not just used for logic.
    diagnostics: Vec<String>,
}

impl S3Backend {
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
}

#[async_trait]
impl StorageProvider for S3Backend {
    fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    async fn list_containers(&self) -> Result<Vec<String>> {
        let resp = self.client.list_buckets().send().await.context("failed to list buckets")?;
        Ok(resp
            .buckets()
            .iter()
            .filter_map(|b| b.name().map(|s| s.to_string()))
            .collect())
    }

    async fn list(&self, bucket: &str, prefix: &str) -> Result<Vec<RemoteEntry>> {
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
                    dirs.push(RemoteEntry {
                        key: p.to_string(),
                        name,
                        is_dir: true,
                        size: 0,
                        last_modified: None,
                        modified_unix: None,
                    });
                }
            }

            for obj in resp.contents() {
                let key = obj.key().unwrap_or_default().to_string();
                if key == prefix {
                    continue;
                }
                let name = key.rsplit('/').next().unwrap_or(&key).to_string();
                files.push(RemoteEntry {
                    key,
                    name,
                    is_dir: false,
                    size: obj.size().unwrap_or(0),
                    last_modified: obj
                        .last_modified()
                        .and_then(|t| t.fmt(aws_sdk_s3::primitives::DateTimeFormat::HttpDate).ok()),
                    modified_unix: obj.last_modified().map(|t| t.secs()),
                });
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        dirs.sort_by_key(|a| a.name.to_lowercase());
        files.sort_by_key(|a| a.name.to_lowercase());
        dirs.extend(files);
        Ok(dirs)
    }

    async fn list_all_under(&self, bucket: &str, prefix: &str) -> Result<Vec<RemoteEntry>> {
        list_objects_paginated(&self.client, bucket, prefix, None).await
    }

    async fn list_under_capped(&self, bucket: &str, prefix: &str, max: usize) -> Result<Vec<RemoteEntry>> {
        list_objects_paginated(&self.client, bucket, prefix, Some(max)).await
    }

    async fn stat_size(&self, bucket: &str, key: &str) -> Result<i64> {
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

    async fn stat_object(&self, bucket: &str, key: &str) -> Result<crate::provider::ObjectMeta> {
        let resp = self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to stat {key}"))?;

        let mut extra = Vec::new();
        if let Some(etag) = resp.e_tag() {
            extra.push(("ETag".to_string(), etag.trim_matches('"').to_string()));
        }
        if let Some(content_type) = resp.content_type() {
            extra.push(("Content-Type".to_string(), content_type.to_string()));
        }
        if let Some(storage_class) = resp.storage_class() {
            extra.push(("Storage Class".to_string(), storage_class.as_str().to_string()));
        }

        Ok(crate::provider::ObjectMeta {
            size: resp.content_length().unwrap_or(0),
            last_modified: resp.last_modified().and_then(|t| t.fmt(aws_sdk_s3::primitives::DateTimeFormat::HttpDate).ok()),
            extra,
        })
    }

    async fn read_range(&self, bucket: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>> {
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

    async fn download(&self, bucket: &str, key: &str, dest: &Path, on_chunk: ProgressFn<'_>) -> Result<()> {
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

    async fn download_to_writer(
        &self,
        bucket: &str,
        key: &str,
        writer: &mut (dyn std::io::Write + Send),
        on_chunk: ProgressFn<'_>,
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

    async fn upload_file(&self, bucket: &str, local_path: &Path, key: &str) -> Result<()> {
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

    /// Copies `old_key` to `new_key` on the server side, leaving the original in place.
    async fn copy_object(&self, bucket: &str, old_key: &str, new_key: &str) -> Result<()> {
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
        Ok(())
    }

    /// Permanently removes a single object.
    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to delete {key}"))?;
        Ok(())
    }
}

/// Shared `ListObjectsV2` pagination loop for [`StorageProvider::list_all_under`] and
/// [`StorageProvider::list_under_capped`] — identical except whether `max` lets it stop
/// requesting further pages once enough objects have been collected, rather than always
/// paging through everything under `prefix` before the caller gets a chance to discard the
/// excess (the difference between a handful of requests and, on a bucket with hundreds of
/// thousands of keys, potentially hundreds of them).
async fn list_objects_paginated(
    client: &Client,
    bucket: &str,
    prefix: &str,
    max: Option<usize>,
) -> Result<Vec<RemoteEntry>> {
    let mut files = Vec::new();
    let mut continuation_token = None;

    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
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
            files.push(RemoteEntry {
                key,
                name,
                is_dir: false,
                size: obj.size().unwrap_or(0),
                last_modified: obj
                    .last_modified()
                    .and_then(|t| t.fmt(aws_sdk_s3::primitives::DateTimeFormat::HttpDate).ok()),
                modified_unix: obj.last_modified().map(|t| t.secs()),
            });
        }

        if max.is_some_and(|max| files.len() >= max) {
            break;
        }
        if resp.is_truncated().unwrap_or(false) {
            continuation_token = resp.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(files)
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
