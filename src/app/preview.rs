//! The preview pane for [`App`]: fetching a bounded snippet of the hovered file (local inline,
//! remote off-thread) and classifying it as text / binary / too-large.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::{App, Focus};
use crate::ui::theme::Mode;

/// Bytes read for a preview snippet — small enough to be cheap even over the network for a
/// remote object, large enough to show a meaningful chunk of a text/JSON/YAML file.
const PREVIEW_BYTES: u64 = 4096;

/// Above this, skip fetching a preview entirely (even the bounded read above isn't free once
/// you count network/head latency) and just say the file's too large.
const MAX_PREVIEW_SIZE: u64 = 5 * 1024 * 1024;

/// One highlighted run within a preview line: an RGB foreground and the text it covers.
#[derive(Clone)]
pub struct HlSpan {
    pub rgb: (u8, u8, u8),
    pub text: String,
}

#[derive(Clone)]
pub enum Preview {
    Empty,
    Loading,
    Directory,
    Text {
        text: String,
        size: u64,
        truncated: bool,
        /// Per-line syntax-highlighted spans, when the file type is recognized. `None` falls
        /// back to plain unstyled text. Computed off the render loop so it never causes lag.
        highlight: Option<Vec<Vec<HlSpan>>>,
    },
    Binary { size: u64 },
    TooLarge { size: u64 },
    Error(String),
}

impl App {
    /// Recomputes the preview pane for whichever pane currently has focus.
    ///
    /// Synchronous and non-blocking: local reads are fast enough to do inline, but a remote
    /// object needs a network round trip, so that case spawns a background task and returns
    /// immediately (showing `Preview::Loading` in the meantime) rather than freezing the UI
    /// on every arrow key press. `preview_generation` tags each request so a slow response
    /// for an object you've since scrolled past is silently dropped instead of clobbering a
    /// newer preview.
    pub fn refresh_preview(&mut self) {
        // Tabbing focus into the preview or transfers pane doesn't change what's being
        // previewed, so leave whatever content and scroll position are already there alone.
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
                        tokio::spawn(async move {
                            let preview = match client.read_range(&bucket, &entry.key, PREVIEW_BYTES).await {
                                Ok(bytes) => classify_bytes(bytes, entry.size.max(0) as u64, &entry.name, dark),
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
            // Unreachable: handled by the early return above.
            Focus::Preview | Focus::Transfers => Preview::Empty,
        };
    }

    /// Applies any preview fetched by a background task started by `refresh_preview`,
    /// dropping it if it's for a stale request (the cursor has since moved on).
    pub fn drain_preview_messages(&mut self) {
        while let Ok((generation, preview)) = self.preview_rx.try_recv() {
            if generation == self.preview_generation {
                self.preview = preview;
            }
        }
    }

    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
        if !self.show_preview && self.focus == Focus::Preview {
            self.focus = Focus::Remote;
        }
    }

    pub fn toggle_local(&mut self) {
        self.show_local = !self.show_local;
        if !self.show_local && self.focus == Focus::Local {
            self.focus = Focus::Remote;
        }
    }

    /// Scrolls the preview pane by `delta` lines, clamped to the text's line count. A no-op
    /// for previews with nothing to scroll (directories, binaries, loading, etc).
    pub fn scroll_preview(&mut self, delta: i32) {
        let Preview::Text { text, .. } = &self.preview else {
            return;
        };
        let max_scroll = text.lines().count().saturating_sub(1) as i32;
        let next = self.preview_scroll as i32 + delta;
        self.preview_scroll = next.clamp(0, max_scroll) as u16;
    }
}

/// Reads at most `max_bytes` from the start of a local file without loading the whole thing
/// into memory first — important once `MAX_PREVIEW_SIZE` no longer catches every large file
/// on a slow filesystem (network mounts, etc).
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

/// Syntax highlighting via `syntect`. Returns `None` (plain-text fallback) for unrecognized
/// extensions or if anything goes wrong. The syntax/theme sets are embedded defaults, loaded
/// once and cached — no filesystem access. Runs off the render loop (in the preview fetch
/// path), so even a large snippet never blocks a redraw.
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
