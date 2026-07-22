use std::collections::HashMap;
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
    /// Directory the local pane opens at for this bookmark, e.g. `~/work/site/dist`. Pairs a
    /// bucket with the directory you actually sync it against, so `s` is useful the moment you
    /// connect instead of after navigating there by hand. Falls back to `[defaults] local_dir`,
    /// then `~/Downloads`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
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

/// Where bookmark files live: `~/.comhad/bookmarks/`.
pub fn bookmarks_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("bookmarks"))
}

/// Moves any `*.json` bookmark left directly under `~/.comhad/` (from before the
/// `bookmarks/` subdirectory existed) into `bookmarks_dir`. Best-effort: a file that fails
/// to move is left in place rather than aborting the whole load.
fn migrate_legacy_bookmarks(config_dir: &Path, bookmarks_dir: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(config_dir) else { return Ok(()) };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let dest = bookmarks_dir.join(path.file_name().expect("json file has a name"));
            let _ = fs::rename(&path, &dest);
        }
    }
    Ok(())
}

/// Loads every bookmark under `~/.comhad/bookmarks/`, migrating any left over from
/// `~/.comhad/*.json` (before that subdirectory existed) first.
pub fn load_connections() -> Result<Vec<(String, Connection)>> {
    let dir = bookmarks_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("failed to create bookmarks dir {}", dir.display()))?;
    migrate_legacy_bookmarks(&config_dir()?, &dir)?;
    load_connections_from(&dir)
}

/// Like [`load_connections`], but takes the bookmarks directory explicitly (for testing).
///
/// Files that fail to parse are skipped with an error message attached, rather than
/// aborting the whole load, so one bad file doesn't lock you out of every bookmark.
pub fn load_connections_from(dir: &Path) -> Result<Vec<(String, Connection)>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut bookmarks = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("failed to read bookmarks dir {}", dir.display()))?
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

/// Root app config, loaded from `~/.comhad/config.toml`. Every field is optional.
#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub keybinds: KeybindsRaw,
}

#[derive(Debug, Default, Deserialize)]
pub struct DefaultsConfig {
    pub show_local: Option<bool>,
    pub show_preview: Option<bool>,
    /// Where the local pane opens when the connected bookmark doesn't set its own
    /// `local_path`. Defaults to `~/Downloads`. A leading `~` is expanded.
    pub local_dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ThemeConfig {
    /// Startup mode: `"light"` or `"dark"`. Defaults to light; toggled at runtime with `t`.
    pub mode: Option<String>,
    #[serde(default)]
    pub light: PaletteOverride,
    #[serde(default)]
    pub dark: PaletteOverride,
}

/// Hex-color overrides for one theme palette, e.g. `accent = "#ff8800"`.
/// Unset fields keep comhad's built-in value.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct PaletteOverride {
    pub bg: Option<String>,
    pub panel_bg: Option<String>,
    pub accent: Option<String>,
    pub accent_dim: Option<String>,
    pub on_accent: Option<String>,
    pub text: Option<String>,
    pub muted: Option<String>,
    pub good: Option<String>,
    pub bad: Option<String>,
    pub dir: Option<String>,
}

/// Raw `[keybinds.*]` tables from `config.toml` — action name to comma-separated key spec
/// (e.g. `"q,ctrl+c"`). Parsed into [`crate::keys::Keybinds`] at startup.
#[derive(Debug, Default, Deserialize)]
pub struct KeybindsRaw {
    #[serde(default)]
    pub connection_picker: HashMap<String, String>,
    #[serde(default)]
    pub bucket_picker: HashMap<String, String>,
    #[serde(default)]
    pub browser: HashMap<String, String>,
    #[serde(default)]
    pub help: HashMap<String, String>,
    #[serde(default)]
    pub events: HashMap<String, String>,
    #[serde(default)]
    pub sync: HashMap<String, String>,
    #[serde(default)]
    pub confirm: HashMap<String, String>,
    #[serde(default)]
    pub bookmark_delete: HashMap<String, String>,
}

/// Loads `~/.comhad/config.toml`, or [`AppConfig::default`] if it doesn't exist.
pub fn load_app_config() -> Result<AppConfig> {
    load_app_config_from(&config_dir()?.join("config.toml"))
}

/// Like [`load_app_config`], but takes the file path explicitly (for testing). Unlike a bad
/// bookmark file, a malformed config is treated as an error rather than silently ignored.
pub fn load_app_config_from(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_app_config_from_missing_file_returns_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = load_app_config_from(&dir.path().join("config.toml")).expect("load_app_config_from");
        assert!(config.theme.mode.is_none());
        assert!(config.keybinds.browser.is_empty());
    }

    #[test]
    fn load_app_config_from_parses_every_section() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r##"
                [defaults]
                show_local = true

                [theme]
                mode = "dark"

                [theme.dark]
                accent = "#ff8800"

                [keybinds.browser]
                quit = "Q"
            "##,
        )
        .expect("write config.toml");

        let config = load_app_config_from(&path).expect("load_app_config_from");
        assert_eq!(config.defaults.show_local, Some(true));
        assert_eq!(config.theme.mode.as_deref(), Some("dark"));
        assert_eq!(config.theme.dark.accent.as_deref(), Some("#ff8800"));
        assert_eq!(config.keybinds.browser.get("quit").map(String::as_str), Some("Q"));
    }

    #[test]
    fn load_app_config_from_rejects_malformed_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not [ valid toml").expect("write config.toml");
        assert!(load_app_config_from(&path).is_err());
    }
}
