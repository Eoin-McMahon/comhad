//! Deep fallback for `/`'s filter: once the query is non-empty, additionally scans recursively
//! under the current prefix/directory (once per session, cached) and appends matches found
//! elsewhere, e.g. `hello.csv` at the root and under `archive/2024/` both show up, the nested
//! one appended below with its path shown and in a distinct color.
//!
//! The remote scan runs off the render loop and uses [`StorageProvider::list_under_capped`] to
//! cap pagination. The local scan is a synchronous filesystem walk (no network, no need to background it).

use std::collections::HashSet;
use std::path::PathBuf;

use super::{App, Focus};
use crate::fuzzy::fuzzy_matches;
use crate::local::{self, LocalEntry};
use crate::provider::RemoteEntry;

/// Cap on a single scan so an enormous prefix doesn't scan unboundedly before showing anything.
const MAX_SCAN: usize = 50_000;

pub struct DeepRemoteMatches {
    /// Everything found by the once-per-session scan. Empty until the background scan completes.
    all: Vec<RemoteEntry>,
    /// `all` filtered by the current query, excluding anything already in the shallow listing.
    pub extra: Vec<RemoteEntry>,
    pub truncated_scan: bool,
    /// Whether the background scan populating `all` is still running.
    pub loading: bool,
}

impl DeepRemoteMatches {
    /// The full scanned list (not just `extra`), used to resolve a marked key that's a deep
    /// match rather than a direct child of the current listing.
    pub fn all_entries(&self) -> &[RemoteEntry] {
        &self.all
    }
}

pub struct DeepLocalMatches {
    all: Vec<LocalEntry>,
    pub extra: Vec<LocalEntry>,
    pub truncated_scan: bool,
}

/// Same fuzzy predicate the shallow filter uses, so deep matches behave consistently.
fn name_matches(name: &str, filter: &str) -> bool {
    fuzzy_matches(name, filter)
}

impl App {
    /// Drops deep-scan state for both panes on navigation, so a stale scan never lingers for
    /// the wrong directory. Bumping the generation drops any still-in-flight scan's result on arrival.
    pub fn clear_deep_matches(&mut self) {
        self.deep_remote = None;
        self.deep_local = None;
        self.deep_scan_generation = self.deep_scan_generation.wrapping_add(1);
        // Navigation invalidates row indices a visual-mode range was anchored against.
        self.visual_anchor = None;
    }

    /// Re-evaluates deep-match state for the focused pane: drops it if the filter is now empty,
    /// kicks off a background scan on the first non-empty keystroke, otherwise just re-filters.
    pub async fn update_deep_matches(&mut self) {
        match self.focus {
            Focus::Remote => self.update_deep_remote(),
            Focus::Local => self.update_deep_local(),
            Focus::Preview | Focus::Transfers => {}
        }
    }

    fn update_deep_remote(&mut self) {
        let query = self.filter.clone().unwrap_or_default();
        if query.is_empty() {
            self.deep_remote = None;
            return;
        }
        if self.deep_remote.is_none() {
            let Some(client) = self.client.clone() else { return };
            self.deep_scan_generation = self.deep_scan_generation.wrapping_add(1);
            let generation = self.deep_scan_generation;
            let bucket = self.bucket.clone();
            let prefix = self.prefix.clone();
            let tx = self.deep_scan_tx.clone();
            tokio::spawn(async move {
                let result = client.list_under_capped(&bucket, &prefix, MAX_SCAN).await.map_err(|e| e.to_string());
                let _ = tx.send((generation, result));
            });
            self.deep_remote = Some(DeepRemoteMatches { all: Vec::new(), extra: Vec::new(), truncated_scan: false, loading: true });
        }
        self.recompute_deep_remote_extra();
    }

    /// Re-filters `deep_remote.all` (possibly still empty, if the scan hasn't completed) against the current query.
    fn recompute_deep_remote_extra(&mut self) {
        let query = self.filter.clone().unwrap_or_default();
        let shallow_keys: HashSet<String> = self.visible_entries().iter().map(|e| e.key.clone()).collect();
        if let Some(deep) = &mut self.deep_remote {
            deep.extra = deep
                .all
                .iter()
                .filter(|e| name_matches(&e.name, &query) && !shallow_keys.contains(&e.key))
                .cloned()
                .collect();
        }
        let total = self.visible_entries().len() + self.deep_remote.as_ref().map(|d| d.extra.len()).unwrap_or(0);
        if self.cursor >= total {
            self.cursor = 0;
        }
    }

    /// Applies a background deep-scan result once it arrives, dropping it if stale, called every tick, mirroring `drain_preview_messages`.
    pub fn drain_deep_scan_messages(&mut self) {
        let mut scan_error = None;
        while let Ok((generation, result)) = self.deep_scan_rx.try_recv() {
            if generation != self.deep_scan_generation {
                continue;
            }
            match (&mut self.deep_remote, result) {
                (Some(deep), Ok(entries)) => {
                    deep.truncated_scan = entries.len() >= MAX_SCAN;
                    deep.all = entries;
                    deep.loading = false;
                }
                (Some(deep), Err(err)) => {
                    deep.loading = false;
                    scan_error = Some(err);
                }
                (None, _) => {}
            }
        }
        if let Some(err) = scan_error {
            self.set_error("deep filter scan failed", &anyhow::anyhow!(err));
        }
        if self.deep_remote.as_ref().is_some_and(|d| !d.loading) {
            self.recompute_deep_remote_extra();
        }
    }

    fn update_deep_local(&mut self) {
        let query = self.local_filter.clone().unwrap_or_default();
        if query.is_empty() {
            self.deep_local = None;
            return;
        }
        if self.deep_local.is_none() {
            let all = local::list_local_recursive(&self.local_cwd, MAX_SCAN);
            let truncated_scan = all.len() >= MAX_SCAN;
            self.deep_local = Some(DeepLocalMatches { all, extra: Vec::new(), truncated_scan });
        }

        let shallow_paths: HashSet<PathBuf> =
            self.visible_local_entries().iter().map(|e| e.path.clone()).collect();
        if let Some(deep) = &mut self.deep_local {
            deep.extra = deep
                .all
                .iter()
                .filter(|e| name_matches(&e.name, &query) && !shallow_paths.contains(&e.path))
                .cloned()
                .collect();
        }
        if self.local_cursor
            >= self.visible_local_entries().len() + self.deep_local.as_ref().map(|d| d.extra.len()).unwrap_or(0)
        {
            self.local_cursor = 0;
        }
    }

    /// Jumps the remote pane to a deep match's parent prefix, with it selected, and clears the filter/deep state.
    pub async fn jump_to_deep_remote(&mut self) -> anyhow::Result<()> {
        let Some(entry) = self.current_entry().cloned() else { return Ok(()) };
        self.filter = None;
        self.clear_deep_matches();
        self.prefix = entry.key.rsplit_once('/').map(|(parent, _)| format!("{parent}/")).unwrap_or_default();
        self.cursor = 0;
        self.refresh().await?;
        if let Some(pos) = self.visible_entries().iter().position(|e| e.key == entry.key) {
            self.cursor = pos;
        }
        self.refresh_preview();
        Ok(())
    }

    /// Mirrors `jump_to_deep_remote`, but for the local pane.
    pub fn jump_to_deep_local(&mut self) {
        let Some(entry) = self.current_local_entry().cloned() else { return };
        self.local_filter = None;
        self.clear_deep_matches();
        self.local_cwd = entry.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.local_cwd.clone());
        self.local_cursor = 0;
        self.refresh_local();
        if let Some(pos) = self.visible_local_entries().iter().position(|e| e.path == entry.path) {
            self.local_cursor = pos;
        }
        self.refresh_preview();
    }
}
