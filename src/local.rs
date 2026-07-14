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

/// Where the local pane starts and undirected downloads land — `~/Downloads`, matching every other tool's convention.
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

/// Recursively collects every file under `dir`, up to `cap` entries — used by `/`'s deep
/// filter fallback. Skips dotfiles, like `list_local`.
pub fn list_local_recursive(dir: &Path, cap: usize) -> Vec<LocalEntry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let Ok(read) = std::fs::read_dir(&current) else { continue };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else {
                out.push(LocalEntry { path, name, is_dir: false, size: meta.len(), modified: meta.modified().ok() });
                if out.len() >= cap {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_local_puts_dirs_first_and_sorts_case_insensitively() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("banana.txt"), "b").expect("write");
        std::fs::write(dir.path().join("Apple.txt"), "a").expect("write");
        std::fs::create_dir(dir.path().join("Zdir")).expect("mkdir");

        let entries = list_local(dir.path()).expect("list_local");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Zdir", "Apple.txt", "banana.txt"]);
        assert!(entries[0].is_dir);
    }

    #[test]
    fn list_local_skips_dotfiles() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".hidden"), "h").expect("write");
        std::fs::write(dir.path().join("visible.txt"), "v").expect("write");

        let entries = list_local(dir.path()).expect("list_local");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "visible.txt");
    }

    #[test]
    fn list_local_recursive_collects_nested_files_and_skips_dotfiles() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("top.txt"), "t").expect("write");
        std::fs::write(dir.path().join("sub/nested.txt"), "n").expect("write");
        std::fs::write(dir.path().join("sub/.hidden"), "h").expect("write");

        let entries = list_local_recursive(dir.path(), 10);
        let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["nested.txt", "top.txt"]);
        assert!(entries.iter().all(|e| !e.is_dir));
    }

    #[test]
    fn list_local_recursive_stops_at_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("file{i}.txt")), "x").expect("write");
        }

        let entries = list_local_recursive(dir.path(), 3);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn default_download_dir_is_under_home() {
        let home = home_dir();
        assert_eq!(default_download_dir(), home.join("Downloads"));
    }
}
