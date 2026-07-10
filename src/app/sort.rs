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
