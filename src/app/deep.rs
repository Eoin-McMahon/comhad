//! Deep fallback for `/`'s filter. The normal filter only ever looks at the single directory
//! currently listed; once it's non-empty, this additionally scans recursively under the
//! current prefix/directory (once, cached) and appends any further matches found elsewhere —
//! e.g. filtering for `hello.csv` with one copy at the root and another under `archive/2024/`
//! shows both, the root one via the normal listing and the nested one appended below it in a
//! distinct color with its path shown, rather than only ever finding whichever copy happens to
//! be a direct child.
//!
//! The remote scan runs off the render loop (like the preview pane's remote reads) so it never
//! freezes the UI, and uses [`StorageProvider::list_under_capped`] so it stops paginating once
//! it has enough rather than always fetching a bucket's entire listing before throwing most of
//! it away. It runs once per filter session (the first non-empty keystroke); every keystroke
//! after that just re-filters the already-scanned list. The local scan is a synchronous
//! filesystem walk — no network round trips, so no need to background it.

use std::collections::HashSet;
use std::path::PathBuf;

use super::{App, Focus};
use crate::local::{self, LocalEntry};
use crate::provider::RemoteEntry;

/// Cap on a single scan, and the number of objects `list_under_capped` is asked to stop at —
/// an enormous prefix shouldn't scan unboundedly before showing anything.
const MAX_SCAN: usize = 50_000;

pub struct DeepRemoteMatches {
    /// Every object found by the once-per-session scan under the prefix that was current when
    /// the filter first became non-empty. Empty until the background scan completes.
    all: Vec<RemoteEntry>,
    /// `all`, filtered by the current query and excluding anything already shown in the
    /// normal (shallow) listing — recomputed whenever the query changes or the scan
    /// completes, cheap since it's just an in-memory filter over `all`.
    pub extra: Vec<RemoteEntry>,
    pub truncated_scan: bool,
    /// Whether the background scan populating `all` is still running.
    pub loading: bool,
}

impl DeepRemoteMatches {
    /// The full scanned list (not just `extra`) — used to resolve a marked key that might be
    /// a deep match rather than a direct child of the current listing.
    pub fn all_entries(&self) -> &[RemoteEntry] {
        &self.all
    }
}

pub struct DeepLocalMatches {
    all: Vec<LocalEntry>,
    pub extra: Vec<LocalEntry>,
    pub truncated_scan: bool,
}

/// Case-insensitive substring match — the same predicate `visible_entries`/
/// `visible_local_entries` use for the shallow filter, so deep matches behave consistently
/// with shallow ones rather than introducing a second, differently-tuned search algorithm.
fn name_matches(name: &str, filter: &str) -> bool {
    name.to_lowercase().contains(&filter.to_lowercase())
}

impl App {
    /// Drops any deep-scan state for both panes — called whenever navigation moves either
    /// pane somewhere else, so a stale scan never lingers and gets shown for the wrong
    /// directory. Bumping the generation means a still-in-flight background scan's result
    /// gets dropped on arrival instead of resurrecting stale state.
    pub fn clear_deep_matches(&mut self) {
        self.deep_remote = None;
        self.deep_local = None;
        self.deep_scan_generation = self.deep_scan_generation.wrapping_add(1);
    }

    /// Re-evaluates deep-match state for the focused pane after its filter changed: drops it
    /// if the filter is now empty, kicks off a background scan if this is the first
    /// non-empty keystroke of a filter session, and otherwise just re-filters whatever's
    /// already been scanned (which may still be loading).
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

    /// Re-filters `deep_remote.all` (whatever's been scanned so far — possibly still empty,
    /// if the background scan hasn't completed) against the current query.
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

    /// Applies a background deep-scan result once it arrives, dropping it if it's stale (the
    /// filter/prefix changed since it started) — called every tick from the main loop,
    /// mirroring `drain_preview_messages`.
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

    /// Jumps the remote pane to a deep match's parent prefix, with it selected, and clears the
    /// filter/deep state — bound to `enter` when the hovered row is a deep extra match.
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
