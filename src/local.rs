use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// Where the local pane starts, and where an undirected download lands if you never open or
/// navigate it — `~/Downloads`, matching every other tool's convention, rather than dumping
/// files straight into `$HOME`.
pub fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads")
}

/// Lists the immediate children of `dir`, directories first, both sorted case-insensitively.
/// Hidden dotfiles are skipped, matching ranger's default behavior.
pub fn list_local(dir: &Path) -> Result<Vec<LocalEntry>> {
    let read = std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let item = LocalEntry {
            path,
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: meta.modified().ok(),
        };
        if item.is_dir {
            dirs.push(item);
        } else {
            files.push(item);
        }
    }

    dirs.sort_by_key(|a| a.name.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());
    dirs.extend(files);
    Ok(dirs)
}
