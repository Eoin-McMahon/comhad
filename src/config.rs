use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One S3-compatible bookmark, loaded from a single JSON file under `~/.comhad/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    pub server: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Bucket, or `bucket/prefix`, to open the browser at.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
    /// AWS region used for SigV4 signing. If omitted, comhad auto-detects it from the
    /// bucket via an unauthenticated HEAD request (the same trick Cyberduck's generic S3
    /// profile relies on), so most bookmarks never need to set this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// `"s3"` (default) or `"s3_private_link"`. Purely informational plus picks a sane
    /// default for `force_path_style` — PrivateLink VPC endpoints are conventionally
    /// addressed virtual-hosted-style, unlike the public S3 endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Overrides the request style. `true` = `endpoint/bucket/key` (matches Cyberduck's
    /// generic S3 profile against the public endpoint). `false` = `bucket.endpoint/key`.
    /// Defaults to `true`, unless `protocol` is `"s3_private_link"` in which case it
    /// defaults to `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_path_style: Option<bool>,
}

impl Connection {
    /// Splits `path` into the bucket name and the (possibly empty) starting prefix.
    pub fn bucket_and_prefix(&self) -> (String, String) {
        match self.path.split_once('/') {
            Some((bucket, prefix)) => {
                let prefix = prefix.trim_start_matches('/');
                let prefix = if prefix.is_empty() {
                    String::new()
                } else if prefix.ends_with('/') {
                    prefix.to_string()
                } else {
                    format!("{prefix}/")
                };
                (bucket.to_string(), prefix)
            }
            None => (self.path.clone(), String::new()),
        }
    }

    /// `server` in a bookmark file is a bare host (e.g. `s3.amazonaws.com`); the AWS SDK
    /// needs a full URL with a scheme.
    pub fn endpoint_url(&self) -> String {
        if self.server.starts_with("http://") || self.server.starts_with("https://") {
            self.server.clone()
        } else {
            format!("https://{}", self.server)
        }
    }

    pub fn force_path_style(&self) -> bool {
        self.force_path_style
            .unwrap_or(self.protocol.as_deref() != Some("s3_private_link"))
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".comhad"))
}

/// Loads every `*.json` file directly under `~/.comhad/` as a [`Connection`] bookmark.
///
/// Files that fail to parse are skipped with an error message attached, rather than
/// aborting the whole load, so one bad file doesn't lock you out of every bookmark.
pub fn load_connections() -> Result<Vec<(String, Connection)>> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create config dir {}", dir.display()))?;
        return Ok(Vec::new());
    }

    let mut bookmarks = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .with_context(|| format!("failed to read config dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str::<Connection>(&raw) {
            Ok(mut conn) => {
                conn.access_key_id = interpolate_env(conn.access_key_id);
                conn.secret_access_key = interpolate_env(conn.secret_access_key);
                bookmarks.push((path.display().to_string(), conn));
            }
            Err(err) => {
                eprintln!("comhad: skipping {} ({err})", path.display());
            }
        }
    }

    Ok(bookmarks)
}

/// Writes a bookmark to disk as pretty JSON, restricted to owner read/write (it holds a
/// secret access key). Used by the in-app add/edit bookmark wizard.
pub fn write_bookmark(path: &Path, conn: &Connection) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(conn).context("failed to serialize bookmark")?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms).with_context(|| format!("failed to chmod {}", path.display()))?;
    }

    Ok(())
}

/// Replaces `${VAR_NAME}` occurrences with the named environment variable's value
fn interpolate_env(value: String) -> String {
    if !value.contains("${") {
        return value;
    }
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    closed = true;
                    break;
                }
                name.push(c2);
            }
            if closed {
                match std::env::var(&name) {
                    Ok(v) => result.push_str(&v),
                    Err(_) => {
                        result.push_str("${");
                        result.push_str(&name);
                        result.push('}');
                    }
                }
            } else {
                result.push_str("${");
                result.push_str(&name);
            }
        } else {
            result.push(c);
        }
    }
    result
}
