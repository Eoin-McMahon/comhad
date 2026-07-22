//! The preview pane for [`App`]: fetching a bounded snippet of the hovered file (local inline,
//! remote off-thread) and classifying it as text / binary / too-large.

use std::sync::OnceLock;

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::{App, Focus};
use crate::ui::theme::Mode;

/// Bytes read for a preview snippet.
const PREVIEW_BYTES: u64 = 4096;

/// Above this, skip fetching a preview and just show "too large".
const MAX_PREVIEW_SIZE: u64 = 5 * 1024 * 1024;

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
    Binary { size: u64 },
    TooLarge { size: u64 },
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
        if matches!(self.focus, Focus::Preview | Focus::Transfers) {
            return;
        }

        self.preview_generation += 1;
        let generation = self.preview_generation;
        self.preview_scroll = 0;

        if !self.show_preview {
            self.preview = Preview::Empty;
            return;
        }

        if self.preview_mode == PreviewMode::Info {
            self.refresh_info(generation);
            return;
        }

        self.preview = match self.focus {
            Focus::Remote => match self.current_entry().cloned() {
                None => Preview::Empty,
                Some(entry) if entry.is_dir => Preview::Directory,
                Some(entry) if entry.size.max(0) as u64 > MAX_PREVIEW_SIZE => {
                    Preview::TooLarge { size: entry.size.max(0) as u64 }
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
                        tokio::spawn(async move {
                            // Images need the whole object to decode, not just a snippet.
                            let read_size = if is_image { size } else { PREVIEW_BYTES };
                            let preview = match client.read_range(&bucket, &entry.key, read_size).await {
                                Ok(bytes) if is_image => classify_image(bytes, size, &picker),
                                Ok(bytes) => classify_bytes(bytes, size, &entry.name, dark),
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
                Some(entry) if entry.is_dir => Preview::Directory,
                Some(entry) if entry.size > MAX_PREVIEW_SIZE => Preview::TooLarge { size: entry.size },
                Some(entry) if is_image_ext(&entry.name) => match std::fs::read(&entry.path) {
                    Ok(bytes) => classify_image(bytes, entry.size, &self.picker),
                    Err(err) => Preview::Error(err.to_string()),
                },
                Some(entry) => {
                    let dark = self.theme == Mode::Dark;
                    match read_local_prefix(&entry.path, PREVIEW_BYTES) {
                        Ok((bytes, truncated)) => {
                            let mut preview = classify_bytes(bytes, entry.size, &entry.name, dark);
                            if let Preview::Text { truncated: t, .. } = &mut preview {
                                *t = truncated;
                            }
                            preview
                        }
                        Err(err) => Preview::Error(err.to_string()),
                    }
                }
            },
            Focus::Preview | Focus::Transfers => Preview::Empty,
        };
    }

    /// `PreviewMode::Info` counterpart to `refresh_preview`'s content-fetch branch.
    fn refresh_info(&mut self, generation: u64) {
        self.preview = match self.focus {
            Focus::Remote => match self.current_entry().cloned() {
                None => Preview::Empty,
                Some(entry) if entry.is_dir => Preview::Info(InfoDetails {
                    name: entry.name.clone(),
                    key: entry.key.clone(),
                    remote_location: Some(format!("s3://{}/{}", self.bucket, entry.key)),
                    local_path: None,
                    is_dir: true,
                    size: None,
                    last_modified: None,
                    extra: Vec::new(),
                }),
                Some(entry) => match &self.client {
                    None => Preview::Empty,
                    Some(client) => {
                        let client = client.clone();
                        let bucket = self.bucket.clone();
                        let tx = self.preview_tx.clone();
                        tokio::spawn(async move {
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
                Some(entry) => Preview::Info(InfoDetails {
                    name: entry.name.clone(),
                    key: entry.path.display().to_string(),
                    remote_location: None,
                    local_path: Some(entry.path.display().to_string()),
                    is_dir: entry.is_dir,
                    size: if entry.is_dir { None } else { Some(entry.size) },
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
        self.refresh_preview();
    }

    /// Mirrors `select_preview_tab` for the **Info** tab, bound to `i`.
    pub fn select_info_tab(&mut self) {
        if self.show_preview && self.preview_mode == PreviewMode::Info {
            self.hide_preview();
            return;
        }
        self.show_preview = true;
        self.preview_mode = PreviewMode::Info;
        self.refresh_preview();
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
        let Preview::Text { text, .. } = &self.preview else {
            return;
        };
        let max_scroll = text.lines().count().saturating_sub(1) as i32;
        let next = self.preview_scroll as i32 + delta;
        self.preview_scroll = next.clamp(0, max_scroll) as u16;
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
        Err(_) => Preview::Binary { size },
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
        return Preview::Binary { size: total_size };
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
