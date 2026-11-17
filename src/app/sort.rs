//! Per-pane sorting. Each pane keeps its own [`Sort`]. Pressing a sort key picks that key, and
//! pressing it again flips the direction.

use std::cmp::Ordering;
use std::time::UNIX_EPOCH;

use super::{App, Focus};
use crate::local::LocalEntry;
use crate::provider::RemoteEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub dir: SortDir,
}

impl Default for Sort {
    fn default() -> Self {
        Self { key: SortKey::Name, dir: SortDir::Asc }
    }
}

impl Sort {
    /// The sort after pressing the key for `key`. A new key starts in the direction people
    /// usually want: A to Z for names, largest and newest first for size and date.
    fn cycle(self, key: SortKey) -> Self {
        if self.key == key {
            let dir = match self.dir {
                SortDir::Asc => SortDir::Desc,
                SortDir::Desc => SortDir::Asc,
            };
            return Self { key, dir };
        }
        let dir = match key {
            SortKey::Name => SortDir::Asc,
            SortKey::Size | SortKey::Modified => SortDir::Desc,
        };
        Self { key, dir }
    }

    /// A short badge for the pane title, e.g. `"size ↓"`. `None` for the default sort.
    pub fn label(self) -> Option<String> {
        if self == Self::default() {
            return None;
        }
        let arrow = match self.dir {
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

/// Sort `items` in place, directories first. Ties fall back to the name so the order is stable
/// between refreshes.
pub(super) fn sort_entries<T: Sortable>(items: &mut [&T], sort: Sort) {
    items.sort_by(|a, b| {
        let dirs_first = b.sort_is_dir().cmp(&a.sort_is_dir());
        let by_key = match sort.key {
            SortKey::Name => Ordering::Equal,
            SortKey::Size => a.sort_size().cmp(&b.sort_size()),
            SortKey::Modified => a.sort_modified().cmp(&b.sort_modified()),
        };
        let by_key = by_key.then_with(|| a.sort_name().cmp(&b.sort_name()));
        let by_key = if sort.dir == SortDir::Desc { by_key.reverse() } else { by_key };
        dirs_first.then(by_key)
    });
}

impl App {
    /// Change the focused pane's sort, keeping the cursor on the same entry.
    pub fn cycle_sort(&mut self, key: SortKey) {
        match self.focus {
            Focus::Remote => {
                let selected = self.current_entry().map(|e| e.key.clone());
                self.remote_sort = self.remote_sort.cycle(key);
                if let Some(pos) = self.visible_entries().iter().position(|e| Some(&e.key) == selected.as_ref()) {
                    self.cursor = pos;
                }
            }
            Focus::Local => {
                let selected = self.current_local_entry().map(|e| e.path.clone());
                self.local_sort = self.local_sort.cycle(key);
                if let Some(pos) = self.visible_local_entries().iter().position(|e| Some(&e.path) == selected.as_ref()) {
                    self.local_cursor = pos;
                }
            }
            Focus::Preview | Focus::Jobs => {}
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
    fn cycle_same_key_flips_the_direction() {
        let sort = Sort::default().cycle(SortKey::Name);
        assert_eq!(sort, Sort { key: SortKey::Name, dir: SortDir::Desc });
        assert_eq!(sort.cycle(SortKey::Name), Sort::default());
    }

    #[test]
    fn cycle_to_size_or_date_starts_largest_or_newest_first() {
        assert_eq!(Sort::default().cycle(SortKey::Size).dir, SortDir::Desc);
        assert_eq!(Sort::default().cycle(SortKey::Modified).dir, SortDir::Desc);
        let by_size = Sort { key: SortKey::Size, dir: SortDir::Asc };
        assert_eq!(by_size.cycle(SortKey::Name), Sort::default());
    }

    #[test]
    fn label_is_hidden_for_the_default_sort() {
        assert_eq!(Sort::default().label(), None);
        assert_eq!(Sort { key: SortKey::Size, dir: SortDir::Asc }.label().as_deref(), Some("size ↑"));
        assert_eq!(Sort { key: SortKey::Modified, dir: SortDir::Desc }.label().as_deref(), Some("modified ↓"));
    }

    fn names(items: &[&RemoteEntry]) -> Vec<String> {
        items.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn sort_entries_by_name_keeps_dirs_first_in_both_directions() {
        let file_a = entry("a-file", 1, 1, false);
        let file_b = entry("B-file", 1, 1, false);
        let dir = entry("z-dir", 1, 1, true);
        let mut items = vec![&file_b, &dir, &file_a];

        sort_entries(&mut items, Sort::default());
        assert_eq!(names(&items), ["z-dir", "a-file", "B-file"]);

        sort_entries(&mut items, Sort { key: SortKey::Name, dir: SortDir::Desc });
        assert_eq!(names(&items), ["z-dir", "B-file", "a-file"]);
    }

    #[test]
    fn sort_entries_by_size_descending_breaks_ties_by_name() {
        let small = entry("small", 10, 1, false);
        let big = entry("big", 100, 1, false);
        let also_big = entry("also-big", 100, 1, false);
        let mut items = vec![&small, &big, &also_big];
        sort_entries(&mut items, Sort { key: SortKey::Size, dir: SortDir::Desc });
        assert_eq!(names(&items), ["big", "also-big", "small"]);
    }

    #[test]
    fn sort_entries_by_modified_ascending() {
        let newer = entry("newer", 1, 200, false);
        let older = entry("older", 1, 100, false);
        let mut items = vec![&newer, &older];
        sort_entries(&mut items, Sort { key: SortKey::Modified, dir: SortDir::Asc });
        assert_eq!(names(&items), ["older", "newer"]);
    }
}
