//! Per-pane sorting. Each pane keeps its own [`Sort`] state; the F1/F2/F3 keys cycle the
//! focused pane's sort through Off → Asc → Desc for name / size / modified respectively.

use std::cmp::Ordering;
use std::time::UNIX_EPOCH;

use super::{App, Focus};
use crate::local::LocalEntry;
use crate::provider::RemoteEntry;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Off,
    Asc,
    Desc,
}

/// A pane's sort state: which key, and which direction (Off = provider's natural order).
#[derive(Clone, Copy)]
pub struct Sort {
    pub key: SortKey,
    pub dir: SortDir,
}

impl Default for Sort {
    fn default() -> Self {
        Self { key: SortKey::Name, dir: SortDir::Off }
    }
}

impl Sort {
    /// Advances the state when the F-key for `key` is pressed: pressing the *same* key cycles
    /// Off → Asc → Desc → Off; pressing a *different* key jumps straight to that key ascending.
    fn cycle(self, key: SortKey) -> Self {
        if self.key != key {
            return Self { key, dir: SortDir::Asc };
        }
        let dir = match self.dir {
            SortDir::Off => SortDir::Asc,
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Off,
        };
        Self { key, dir }
    }

    /// A short badge for the pane title, e.g. `"size ↓"`. `None` when sorting is off.
    pub fn label(self) -> Option<String> {
        let arrow = match self.dir {
            SortDir::Off => return None,
            SortDir::Asc => "↑",
            SortDir::Desc => "↓",
        };
        let name = match self.key {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "modified",
        };
        Some(format!("{name} {arrow}"))
    }
}

/// The fields any listed entry must expose to be sortable. Keeps the ordering logic in one
/// place across the remote and local panes.
pub(super) trait Sortable {
    fn sort_name(&self) -> String;
    fn sort_size(&self) -> u64;
    fn sort_modified(&self) -> i64;
    fn sort_is_dir(&self) -> bool;
}

impl Sortable for RemoteEntry {
    fn sort_name(&self) -> String {
        self.name.to_lowercase()
    }
    fn sort_size(&self) -> u64 {
        self.size.max(0) as u64
    }
    fn sort_modified(&self) -> i64 {
        self.modified_unix.unwrap_or(0)
    }
    fn sort_is_dir(&self) -> bool {
        self.is_dir
    }
}

impl Sortable for LocalEntry {
    fn sort_name(&self) -> String {
        self.name.to_lowercase()
    }
    fn sort_size(&self) -> u64 {
        self.size
    }
    fn sort_modified(&self) -> i64 {
        self.modified
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    fn sort_is_dir(&self) -> bool {
        self.is_dir
    }
}

/// Sorts `items` in place by `sort`, always keeping directories grouped ahead of files (a
/// no-op when the direction is Off, preserving the provider's natural order).
pub(super) fn sort_entries<T: Sortable>(items: &mut [&T], sort: Sort) {
    if sort.dir == SortDir::Off {
        return;
    }
    items.sort_by(|a, b| {
        match (a.sort_is_dir(), b.sort_is_dir()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        let ord = match sort.key {
            SortKey::Name => a.sort_name().cmp(&b.sort_name()),
            SortKey::Size => a.sort_size().cmp(&b.sort_size()),
            SortKey::Modified => a.sort_modified().cmp(&b.sort_modified()),
        };
        if sort.dir == SortDir::Desc { ord.reverse() } else { ord }
    });
}

impl App {
    /// Cycles the focused pane's sort for `key` (F1/F2/F3). Ignored on the preview/transfers
    /// panes, which aren't sortable file listings.
    pub fn cycle_sort(&mut self, key: SortKey) {
        match self.focus {
            Focus::Remote => self.remote_sort = self.remote_sort.cycle(key),
            Focus::Local => self.local_sort = self.local_sort.cycle(key),
            Focus::Preview | Focus::Transfers => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: i64, modified: i64, is_dir: bool) -> RemoteEntry {
        RemoteEntry {
            key: name.to_string(),
            name: name.to_string(),
            is_dir,
            size,
            last_modified: None,
            modified_unix: Some(modified),
        }
    }

    #[test]
    fn cycle_same_key_goes_off_asc_desc_off() {
        let sort = Sort::default();
        assert!(matches!(sort.dir, SortDir::Off));
        let sort = sort.cycle(SortKey::Name);
        assert!(matches!(sort.dir, SortDir::Asc));
        let sort = sort.cycle(SortKey::Name);
        assert!(matches!(sort.dir, SortDir::Desc));
        let sort = sort.cycle(SortKey::Name);
        assert!(matches!(sort.dir, SortDir::Off));
    }

    #[test]
    fn cycle_different_key_jumps_to_ascending() {
        let sort = Sort { key: SortKey::Name, dir: SortDir::Desc };
        let sort = sort.cycle(SortKey::Size);
        assert!(matches!(sort.key, SortKey::Size));
        assert!(matches!(sort.dir, SortDir::Asc));
    }

    #[test]
    fn label_is_none_when_off_and_formatted_otherwise() {
        assert_eq!(Sort::default().label(), None);
        assert_eq!(Sort { key: SortKey::Size, dir: SortDir::Asc }.label(), Some("size ↑".to_string()));
        assert_eq!(Sort { key: SortKey::Modified, dir: SortDir::Desc }.label(), Some("modified ↓".to_string()));
    }

    #[test]
    fn sort_entries_off_preserves_order() {
        let a = entry("b", 1, 1, false);
        let b = entry("a", 2, 2, false);
        let mut items = vec![&a, &b];
        sort_entries(&mut items, Sort::default());
        assert_eq!(items[0].name, "b");
        assert_eq!(items[1].name, "a");
    }

    #[test]
    fn sort_entries_by_name_keeps_dirs_first() {
        let file = entry("a-file", 1, 1, false);
        let dir = entry("z-dir", 1, 1, true);
        let mut items = vec![&file, &dir];
        sort_entries(&mut items, Sort { key: SortKey::Name, dir: SortDir::Asc });
        assert!(items[0].is_dir);
        assert_eq!(items[1].name, "a-file");
    }

    #[test]
    fn sort_entries_by_size_descending() {
        let small = entry("small", 10, 1, false);
        let big = entry("big", 100, 1, false);
        let mut items = vec![&small, &big];
        sort_entries(&mut items, Sort { key: SortKey::Size, dir: SortDir::Desc });
        assert_eq!(items[0].name, "big");
        assert_eq!(items[1].name, "small");
    }

    #[test]
    fn sort_entries_by_modified_ascending() {
        let newer = entry("newer", 1, 200, false);
        let older = entry("older", 1, 100, false);
        let mut items = vec![&newer, &older];
        sort_entries(&mut items, Sort { key: SortKey::Modified, dir: SortDir::Asc });
        assert_eq!(items[0].name, "older");
        assert_eq!(items[1].name, "newer");
    }
}
