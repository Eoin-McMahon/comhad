//! The preview pane for [`App`]: fetching a bounded snippet of the hovered file (local inline,
//! remote off-thread) and classifying it as text / binary / too-large.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::structure::{self, StructureMap};
use super::{App, Focus};
use crate::ui::theme::Mode;

/// Bytes read for a preview snippet.
const PREVIEW_BYTES: u64 = 4096;

/// Above this, skip fetching a preview and just show "too large".
const MAX_PREVIEW_SIZE: u64 = 5 * 1024 * 1024;

/// Images get a much higher cap than plain text/binary: a real photo (phone camera, a decent
/// screenshot) routinely runs well past 5 MB, and `MAX_PREVIEW_SIZE` used to apply to images too.
/// In practice every normal-sized photo just showed "too large to preview", never an image.
const MAX_IMAGE_PREVIEW_SIZE: u64 = 25 * 1024 * 1024;

/// Whether a background preview task's `generation` is still the one the app is showing. Checked
/// *before* doing any real work (network call, file read, decode, directory walk), not just when
/// the result comes back, so a burst of superseded requests (rapidly arrowing through a folder)
/// bail out near-instantly instead of each still paying the full cost only to be thrown away.
fn still_current(live: &Arc<AtomicU64>, generation: u64) -> bool {
    live.load(Ordering::Relaxed) == generation
}

/// One highlighted run within a preview line: an RGB foreground and the text it covers.
#[derive(Clone)]
pub struct HlSpan {
    pub rgb: (u8, u8, u8),
    pub text: String,
}

/// Whether the preview pane shows file content or metadata, toggled with `i`, sticky
/// across cursor movement.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewMode {
    #[default]
    Content,
    Info,
}

/// Everything the info view (`i`) shows about the hovered item.
pub struct InfoDetails {
    pub name: String,
    pub key: String,
    /// E.g. `s3://bucket/key`, for a remote item.
    pub remote_location: Option<String>,
    /// Absolute filesystem path, for a local item.
    pub local_path: Option<String>,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub last_modified: Option<String>,
    pub extra: Vec<(String, String)>,
}

pub enum Preview {
    Empty,
    Loading,
    Directory,
    Text {
        text: String,
        size: u64,
        truncated: bool,
        /// Per-line syntax-highlighted spans; `None` falls back to plain text.
        highlight: Option<Vec<Vec<HlSpan>>>,
    },
    /// A decoded image, fed to `ratatui_image`'s `StatefulImage` widget at render time.
    Image { size: u64, state: Box<StatefulProtocol> },
    /// A file whose bytes aren't text. `map` arrives in a second message once the background
    /// sampler has been across the object, so the pane says "binary" immediately rather than
    /// sitting on `Loading` while the windows are read.
    Binary { size: u64, map: Option<StructureMap> },
    /// A file past the preview cap. Same two-stage `map` as `Binary`: this is where it earns the
    /// most, since it's the only thing the pane can say about an object it will never open.
    TooLarge { size: u64, map: Option<StructureMap> },
    Info(InfoDetails),
    Error(String),
}

impl App {
    /// Recomputes the preview pane for whichever pane currently has focus.
    ///
    /// Local reads are inline; remote objects fetch in a background task (shown as
    /// `Preview::Loading` meanwhile) so a network round trip never blocks the UI.
    /// `preview_generation` tags each request so a stale response is dropped.
    pub fn refresh_preview(&mut self) {
        match self.focus {
            // Remember which browser pane the preview reflects, so focusing or scrolling the
            // preview/transfers pane keeps previewing that pane's selection rather than blanking.
            Focus::Remote | Focus::Local => self.preview_source = self.focus,
            // Navigating within the preview/transfers panes shouldn't refetch the preview.
            Focus::Preview | Focus::Transfers => return,
        }
        self.rebuild_preview();
    }

    /// Rebuilds the preview/info for `preview_source` (the browser pane it reflects), regardless
    /// of which pane currently has focus. Split from [`refresh_preview`](Self::refresh_preview) so
    /// the `p`/`i` tab toggles can force a rebuild even while the preview pane itself is focused.
    pub(super) fn rebuild_preview(&mut self) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        self.preview_generation_live.store(generation, std::sync::atomic::Ordering::Relaxed);
        self.preview_scroll = 0;
        self.info_scroll = 0;

        if !self.show_preview {
            self.preview = Preview::Empty;
            return;
        }

        if self.preview_mode == PreviewMode::Info {
            self.refresh_info(generation);
            return;
        }

        self.preview = match self.preview_source {
            Focus::Remote => match self.current_entry().cloned() {
                None => Preview::Empty,
                Some(entry) if entry.is_dir => Preview::Directory,
                Some(entry)
                    if entry.size.max(0) as u64
                        > if is_image_ext(&entry.name) { MAX_IMAGE_PREVIEW_SIZE } else { MAX_PREVIEW_SIZE } =>
                {
                    let size = entry.size.max(0) as u64;
                    self.spawn_remote_structure(&entry.key, size, generation, |size, map| Preview::TooLarge {
                        size,
                        map,
                    })
                }
                Some(entry) => match &self.client {
                    None => Preview::Empty,
                    Some(client) => {
                        let client = client.clone();
                        let bucket = self.bucket.clone();
                        let tx = self.preview_tx.clone();
                        let dark = self.theme == Mode::Dark;
                        let picker = self.picker.clone();
                        let size = entry.size.max(0) as u64;
                        let is_image = is_image_ext(&entry.name);
                        let live = self.preview_generation_live.clone();
                        tokio::spawn(async move {
                            if !still_current(&live, generation) {
                                return;
                            }
                            // Images need the whole object to decode, not just a snippet.
                            let read_size = if is_image { size } else { PREVIEW_BYTES };
                            let preview = match client.read_range(&bucket, &entry.key, read_size).await {
                                Ok(bytes) if is_image => classify_image(bytes, size, &picker),
                                Ok(bytes) => classify_bytes(bytes, size, &entry.name, dark),
                                Err(err) => Preview::Error(err.to_string()),
                            };
                            let binary = matches!(preview, Preview::Binary { .. });
                            let _ = tx.send((generation, preview));

                            // The snippet says it's binary; the map takes another round of
                            // requests, so it follows as a second message rather than holding
                            // the first one back.
                            if binary && still_current(&live, generation) {
                                if let Some(map) = structure::sample_remote(&client, &bucket, &entry.key, size).await {
                                    let _ = tx.send((generation, Preview::Binary { size, map: Some(map) }));
                                }
                            }
                        });
                        Preview::Loading
                    }
                },
            },
            Focus::Local => match self.current_local_entry().cloned() {
                None => Preview::Empty,
                Some(entry) if entry.is_dir => Preview::Directory,
                // Images: same higher cap as remote, and loaded off-thread the same way. Decode
                // and `picker.new_resize_protocol` are real CPU work, and reading + decoding a
                // multi-megabyte photo inline here used to block the whole UI (no draw calls,
                // frozen input) until it finished, unlike the remote path which already runs in
                // a background task.
                Some(entry) if is_image_ext(&entry.name) => {
                    if entry.size > MAX_IMAGE_PREVIEW_SIZE {
                        self.spawn_local_structure(&entry.path, entry.size, generation, |size, map| {
                            Preview::TooLarge { size, map }
                        })
                    } else {
                        let path = entry.path.clone();
                        let size = entry.size;
                        let tx = self.preview_tx.clone();
                        let picker = self.picker.clone();
                        let live = self.preview_generation_live.clone();
                        tokio::task::spawn_blocking(move || {
                            if !still_current(&live, generation) {
                                return;
                            }
                            let preview = match std::fs::read(&path) {
                                Ok(bytes) => classify_image(bytes, size, &picker),
                                Err(err) => Preview::Error(err.to_string()),
                            };
                            let _ = tx.send((generation, preview));
                        });
                        Preview::Loading
                    }
                }
                Some(entry) if entry.size > MAX_PREVIEW_SIZE => {
                    self.spawn_local_structure(&entry.path, entry.size, generation, |size, map| Preview::TooLarge {
                        size,
                        map,
                    })
                }
                Some(entry) => {
                    let dark = self.theme == Mode::Dark;
                    match read_local_prefix(&entry.path, PREVIEW_BYTES) {
                        Ok((bytes, truncated)) => {
                            let mut preview = classify_bytes(bytes, entry.size, &entry.name, dark);
                            if let Preview::Text { truncated: t, .. } = &mut preview {
                                *t = truncated;
                            }
                            // Same two stages as the remote path: say "binary" now, map it once
                            // the sampler has walked the file.
                            match preview {
                                Preview::Binary { size, .. } => self
                                    .spawn_local_structure(&entry.path, size, generation, |size, map| {
                                        Preview::Binary { size, map }
                                    }),
                                other => other,
                            }
                        }
                        Err(err) => Preview::Error(err.to_string()),
                    }
                }
            },
            Focus::Preview | Focus::Transfers => Preview::Empty,
        };
    }

    /// Starts sampling a remote object's byte structure and returns the preview to show meanwhile:
    /// `make(size, None)`, which the finished map replaces via `make(size, Some(map))`.
    ///
    /// `make` is what keeps this shared between the binary and too-large cases — they differ only
    /// in which variant carries the map, not in how one is obtained.
    fn spawn_remote_structure(
        &self,
        key: &str,
        size: u64,
        generation: u64,
        make: fn(u64, Option<StructureMap>) -> Preview,
    ) -> Preview {
        let Some(client) = self.client.clone() else { return make(size, None) };
        let bucket = self.bucket.clone();
        let key = key.to_string();
        let tx = self.preview_tx.clone();
        let live = self.preview_generation_live.clone();
        tokio::spawn(async move {
            if !still_current(&live, generation) {
                return;
            }
            // A map that couldn't be built sends nothing, leaving the plain size message up
            // rather than replacing it with an empty chart.
            if let Some(map) = structure::sample_remote(&client, &bucket, &key, size).await {
                let _ = tx.send((generation, make(size, Some(map))));
            }
        });
        make(size, None)
    }

    /// Local counterpart to [`spawn_remote_structure`](Self::spawn_remote_structure). Off-thread
    /// for the same reason the local image path is: seeking across a multi-gigabyte file is real
    /// work, and doing it inline would stall the frame.
    fn spawn_local_structure(
        &self,
        path: &std::path::Path,
        size: u64,
        generation: u64,
        make: fn(u64, Option<StructureMap>) -> Preview,
    ) -> Preview {
        let path = path.to_path_buf();
        let tx = self.preview_tx.clone();
        let live = self.preview_generation_live.clone();
        tokio::task::spawn_blocking(move || {
            if !still_current(&live, generation) {
                return;
            }
            if let Some(map) = structure::sample_local(&path, size) {
                let _ = tx.send((generation, make(size, Some(map))));
            }
        });
        make(size, None)
    }

    /// `PreviewMode::Info` counterpart to `refresh_preview`'s content-fetch branch.
    fn refresh_info(&mut self, generation: u64) {
        self.preview = match self.preview_source {
            Focus::Remote => match self.current_entry().cloned() {
                None => Preview::Empty,
                // A prefix has no metadata of its own, so aggregate the objects beneath it: total
                // size, object count, and the most-recent last-modified. Done in the background
                // since it walks the whole prefix.
                Some(entry) if entry.is_dir => match &self.client {
                    None => Preview::Empty,
                    Some(client) => {
                        let client = client.clone();
                        let bucket = self.bucket.clone();
                        let tx = self.preview_tx.clone();
                        let live = self.preview_generation_live.clone();
                        tokio::spawn(async move {
                            if !still_current(&live, generation) {
                                return;
                            }
                            let preview = match client.list_all_under(&bucket, &entry.key).await {
                                Ok(objects) => {
                                    let total: i64 = objects.iter().map(|o| o.size.max(0)).sum();
                                    let newest = objects.iter().max_by_key(|o| o.modified_unix.unwrap_or(i64::MIN));
                                    Preview::Info(InfoDetails {
                                        name: entry.name.clone(),
                                        key: entry.key.clone(),
                                        remote_location: Some(format!("s3://{bucket}/{}", entry.key)),
                                        local_path: None,
                                        is_dir: true,
                                        size: Some(total.max(0) as u64),
                                        last_modified: newest.and_then(|o| o.last_modified.clone()),
                                        extra: vec![("objects".to_string(), objects.len().to_string())],
                                    })
                                }
                                Err(err) => Preview::Error(err.to_string()),
                            };
                            let _ = tx.send((generation, preview));
                        });
                        Preview::Loading
                    }
                },
                Some(entry) => match &self.client {
                    None => Preview::Empty,
                    Some(client) => {
                        let client = client.clone();
                        let bucket = self.bucket.clone();
                        let tx = self.preview_tx.clone();
                        let live = self.preview_generation_live.clone();
                        tokio::spawn(async move {
                            if !still_current(&live, generation) {
                                return;
                            }
                            let preview = match client.stat_object(&bucket, &entry.key).await {
                                Ok(meta) => Preview::Info(InfoDetails {
                                    name: entry.name.clone(),
                                    key: entry.key.clone(),
                                    remote_location: Some(format!("s3://{bucket}/{}", entry.key)),
                                    local_path: None,
                                    is_dir: false,
                                    size: Some(meta.size.max(0) as u64),
                                    last_modified: meta.last_modified,
                                    extra: meta.extra,
                                }),
                                Err(err) => Preview::Error(err.to_string()),
                            };
                            let _ = tx.send((generation, preview));
                        });
                        Preview::Loading
                    }
                },
            },
            Focus::Local => match self.current_local_entry().cloned() {
                None => Preview::Empty,
                // Aggregate the files under a directory: total size, newest mtime, file count.
                // Off-thread, same reasoning as the image case above: walking a large directory
                // synchronously here used to freeze the whole UI until it finished, unlike the
                // remote `list_all_under` aggregate just below, which already runs in the
                // background.
                Some(entry) if entry.is_dir => {
                    let path = entry.path.clone();
                    let name = entry.name.clone();
                    let tx = self.preview_tx.clone();
                    let live = self.preview_generation_live.clone();
                    tokio::task::spawn_blocking(move || {
                        if !still_current(&live, generation) {
                            return;
                        }
                        let (total, newest, count) = aggregate_local_dir(&path);
                        let preview = Preview::Info(InfoDetails {
                            name,
                            key: path.display().to_string(),
                            remote_location: None,
                            local_path: Some(path.display().to_string()),
                            is_dir: true,
                            size: Some(total),
                            last_modified: newest
                                .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string()),
                            extra: vec![("files".to_string(), count.to_string())],
                        });
                        let _ = tx.send((generation, preview));
                    });
                    Preview::Loading
                }
                Some(entry) => Preview::Info(InfoDetails {
                    name: entry.name.clone(),
                    key: entry.path.display().to_string(),
                    remote_location: None,
                    local_path: Some(entry.path.display().to_string()),
                    is_dir: false,
                    size: Some(entry.size),
                    last_modified: entry
                        .modified
                        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string()),
                    extra: Vec::new(),
                }),
            },
            Focus::Preview | Focus::Transfers => Preview::Empty,
        };
    }

    /// Selects the **Preview** tab (file content), bound to `p`. Toggles the pane if
    /// already showing this tab.
    pub fn select_preview_tab(&mut self) {
        if self.show_preview && self.preview_mode == PreviewMode::Content {
            self.hide_preview();
            return;
        }
        self.show_preview = true;
        self.preview_mode = PreviewMode::Content;
        // With a browser pane focused the tab reflects it; from the preview pane, keep the
        // existing source. Either way rebuild directly so the toggle works even when focused here.
        if matches!(self.focus, Focus::Remote | Focus::Local) {
            self.preview_source = self.focus;
        }
        self.rebuild_preview();
    }

    /// Mirrors `select_preview_tab` for the **Info** tab, bound to `i`.
    pub fn select_info_tab(&mut self) {
        if self.show_preview && self.preview_mode == PreviewMode::Info {
            self.hide_preview();
            return;
        }
        self.show_preview = true;
        self.preview_mode = PreviewMode::Info;
        if matches!(self.focus, Focus::Remote | Focus::Local) {
            self.preview_source = self.focus;
        }
        self.rebuild_preview();
    }

    fn hide_preview(&mut self) {
        self.show_preview = false;
        if self.focus == Focus::Preview {
            self.focus = Focus::Remote;
            self.visual_anchor = None;
        }
    }

    /// Applies a preview fetched by `refresh_preview`'s background task, dropping stale ones.
    pub fn drain_preview_messages(&mut self) {
        while let Ok((generation, preview)) = self.preview_rx.try_recv() {
            if generation == self.preview_generation {
                self.preview = preview;
            }
        }
    }

    pub fn toggle_local(&mut self) {
        self.show_local = !self.show_local;
        if !self.show_local && self.focus == Focus::Local {
            self.focus = Focus::Remote;
            self.visual_anchor = None;
        }
    }

    /// Scrolls the preview pane by `delta` lines, clamped to the text's line count.
    pub fn scroll_preview(&mut self, delta: i32) {
        match &self.preview {
            Preview::Text { text, .. } => {
                let max_scroll = text.lines().count().saturating_sub(1) as i32;
                let next = self.preview_scroll as i32 + delta;
                self.preview_scroll = next.clamp(0, max_scroll) as u16;
            }
            // The exact content height depends on the palette (colored labels etc.), which isn't
            // available here. The renderer clamps this against the real line count at draw
            // time, this just has to stay non-negative and not run away unboundedly.
            Preview::Info(_) => {
                let next = (self.info_scroll as i32 + delta).clamp(0, 2000);
                self.info_scroll = next as u16;
            }
            _ => {}
        }
    }
}

/// Reads at most `max_bytes` from the start of a local file without loading it all into memory.
fn read_local_prefix(path: &std::path::Path, max_bytes: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; max_bytes as usize];
    let mut total = 0;
    loop {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
        if total == buf.len() {
            break;
        }
    }
    buf.truncate(total);
    let truncated = file.read(&mut [0u8; 1])? > 0;
    Ok((buf, truncated))
}

/// Extensions the `image` crate is built to decode.
fn is_image_ext(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else { return false };
    matches!(ext.to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
}

/// Decodes an image via `picker`; falls back to `Binary` if decoding fails.
fn classify_image(bytes: Vec<u8>, size: u64, picker: &Picker) -> Preview {
    match image::load_from_memory(&bytes) {
        Ok(img) => Preview::Image { size, state: Box::new(picker.new_resize_protocol(img)) },
        Err(_) => Preview::Binary { size, map: None },
    }
}

/// Best-effort text/binary classification of a preview snippet, syntax-highlighting the text
/// when the filename's extension is recognized.
fn classify_bytes(bytes: Vec<u8>, total_size: u64, name: &str, dark: bool) -> Preview {
    if bytes.is_empty() {
        return Preview::Text { text: String::new(), size: total_size, truncated: false, highlight: None };
    }
    let sample_len = bytes.len().min(512);
    let non_printable = bytes[..sample_len]
        .iter()
        .filter(|&&b| b != b'\n' && b != b'\r' && b != b'\t' && (b < 0x20 || b == 0x7f))
        .count();
    if non_printable * 20 > sample_len {
        return Preview::Binary { size: total_size, map: None };
    }
    let truncated = (bytes.len() as u64) < total_size;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let highlight = highlight(&text, name, dark);
    Preview::Text { text, size: total_size, truncated, highlight }
}

/// Syntax highlighting via `syntect`; `None` for unrecognized extensions or on any error.
fn highlight(text: &str, name: &str, dark: bool) -> Option<Vec<Vec<HlSpan>>> {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();

    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);

    let ext = name.rsplit_once('.').map(|(_, e)| e)?;
    let syntax = syntaxes.find_syntax_by_extension(ext)?;
    let theme = themes.themes.get(if dark { "base16-ocean.dark" } else { "InspiredGitHub" })?;

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(text) {
        let ranges = highlighter.highlight_line(line, syntaxes).ok()?;
        let spans = ranges
            .into_iter()
            .map(|(style, piece)| HlSpan {
                rgb: (style.foreground.r, style.foreground.g, style.foreground.b),
                text: piece.trim_end_matches(['\n', '\r']).to_string(),
            })
            .collect();
        lines.push(spans);
    }
    Some(lines)
}

/// Recursively totals the files under a local directory: (total bytes, newest mtime, file count).
/// Bounded so a pathological tree can't hang the UI; symlinks are not followed.
fn aggregate_local_dir(root: &std::path::Path) -> (u64, Option<std::time::SystemTime>, usize) {
    const MAX_FILES: usize = 100_000;
    let mut total = 0u64;
    let mut newest: Option<std::time::SystemTime> = None;
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                    if let Ok(modified) = meta.modified() {
                        newest = Some(newest.map_or(modified, |n| n.max(modified)));
                    }
                }
                count += 1;
                if count >= MAX_FILES {
                    return (total, newest, count);
                }
            }
        }
    }
    (total, newest, count)
}
