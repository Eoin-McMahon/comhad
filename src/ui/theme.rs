use std::fs;
use std::path::{Path, PathBuf};

use ratatui::style::Color;

use crate::config::IconSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

impl Mode {
    pub fn toggled(self) -> Self {
        match self {
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Light,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Mode::Light => Palette::light(),
            Mode::Dark => Palette::dark(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub panel_bg: Color,
    pub accent: Color,
    pub accent_dim: Color,
    pub on_accent: Color,
    pub text: Color,
    pub muted: Color,
    pub good: Color,
    pub bad: Color,
    pub dir: Color,
}

impl Palette {
    // modus_vivendi, tinted variant (github.com/miikanissi/modus-themes.nvim).
    pub fn dark() -> Self {
        Self {
            bg: Color::Rgb(0x0d, 0x0e, 0x1c),         // tinted_bg_main
            panel_bg: Color::Rgb(0x1d, 0x22, 0x35),   // tinted_bg_dim
            accent: Color::Rgb(0x2f, 0xaf, 0xff),     // blue
            accent_dim: Color::Rgb(0x82, 0xb0, 0xec), // blue_faint
            on_accent: Color::Rgb(0x0d, 0x0e, 0x1c),  // tinted_bg_main
            text: Color::Rgb(0xff, 0xff, 0xff),       // fg_main
            muted: Color::Rgb(0x98, 0x98, 0x98),      // fg_dim
            good: Color::Rgb(0x44, 0xbc, 0x44),       // green
            bad: Color::Rgb(0xff, 0x5f, 0x59),        // red
            dir: Color::Rgb(0xc0, 0x96, 0x5b),        // gold
        }
    }

    // modus_operandi, tinted variant (github.com/miikanissi/modus-themes.nvim).
    pub fn light() -> Self {
        Self {
            bg: Color::Rgb(0xfb, 0xf7, 0xf0),         // tinted_bg_main
            panel_bg: Color::Rgb(0xef, 0xe9, 0xdd),   // tinted_bg_dim
            accent: Color::Rgb(0x00, 0x31, 0xa9),     // blue
            accent_dim: Color::Rgb(0x00, 0x34, 0x97), // blue_faint
            on_accent: Color::Rgb(0xfb, 0xf7, 0xf0),  // tinted_bg_main
            text: Color::Rgb(0x00, 0x00, 0x00),       // fg_main
            muted: Color::Rgb(0x59, 0x59, 0x59),      // fg_dim
            good: Color::Rgb(0x00, 0x68, 0x00),       // green
            bad: Color::Rgb(0xa6, 0x00, 0x00),        // red
            dir: Color::Rgb(0x80, 0x60, 0x1f),        // gold
        }
    }

    /// Applies config-supplied hex overrides field by field; unset fields keep their built-in value.
    pub fn with_overrides(mut self, overrides: &crate::config::PaletteOverride) -> Self {
        let fields: [(&Option<String>, &mut Color); 10] = [
            (&overrides.bg, &mut self.bg),
            (&overrides.panel_bg, &mut self.panel_bg),
            (&overrides.accent, &mut self.accent),
            (&overrides.accent_dim, &mut self.accent_dim),
            (&overrides.on_accent, &mut self.on_accent),
            (&overrides.text, &mut self.text),
            (&overrides.muted, &mut self.muted),
            (&overrides.good, &mut self.good),
            (&overrides.bad, &mut self.bad),
            (&overrides.dir, &mut self.dir),
        ];
        for (hex, slot) in fields {
            if let Some(hex) = hex
                && let Some(color) = parse_hex_color(hex)
            {
                *slot = color;
            }
        }
        self
    }
}

/// Parses a `#rrggbb` (or bare `rrggbb`) hex string. Returns `None` on malformed input rather
/// than erroring, a typo'd color in `config.toml` should fall back, not stop the app starting.
fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

pub const SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(frame: usize) -> &'static str {
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

/// The Unicode fallback is deliberately a small set of old (pre-2016 Unicode),
/// single-codepoint, default-emoji-presentation glyphs, no variation selectors (`\u{fe0f}`),
/// which is what caused border and column misalignment: several "nicer" icons
/// (🖼️ 🗂️ ☁️ 👁) default to *text* presentation and only render as emoji with a trailing
/// variation selector, so their on-screen width disagrees with what ratatui/unicode-width
/// computes depending on the font and terminal. It stays coarse-grained (one generic code
/// icon for every language) — there's no plain-Unicode equivalent of a per-language logo.
///
/// The Nerd Font set gives each language its own logo, the same idea as nvim-web-devicons or
/// VS Code's file-icon theme, using private-use-area codepoints from the Devicons/Seti/Font
/// Awesome sets bundled with Nerd Fonts. In Nerd Font mode each icon also carries the
/// nvim-web-devicons colour for that type ([`FileIcon::color`]), so the file list reads the
/// same way a devicons-enabled editor does.
pub struct FileIcon {
    pub glyph: &'static str,
    /// nvim-web-devicons colour for the type. `Some` only in Nerd Font mode: the emoji/Unicode
    /// fallbacks carry their own presentation colour, so recolouring them would look wrong.
    pub color: Option<Color>,
}

pub fn icon_for(name: &str, is_dir: bool, icons: IconSet) -> FileIcon {
    if is_dir {
        let glyph = match icons {
            IconSet::Unicode | IconSet::Auto => "📁",
            IconSet::Nerdfont => "\u{f07b}", // nf-fa-folder
        };
        // Directories keep the pane's own `dir` colour (set by the caller), not a devicon one.
        return FileIcon { glyph, color: None };
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    if icons == IconSet::Nerdfont {
        let (glyph, color) = nerdfont_file_icon(ext.as_deref());
        return FileIcon { glyph, color: Some(color) };
    }
    let glyph = match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp") => "📷",
        Some("mp4" | "mov" | "avi" | "mkv" | "webm") => "🎬",
        Some("mp3" | "wav" | "flac" | "aac" | "ogg") => "🎵",
        Some("zip" | "tar" | "gz" | "7z" | "rar" | "bz2") => "📦",
        Some("pdf") => "📕",
        Some("md" | "txt" | "rst") => "📝",
        Some("rs" | "py" | "js" | "ts" | "go" | "java" | "c" | "cpp" | "sh") => "💻",
        _ => "📄",
    };
    FileIcon { glyph, color: None }
}

/// `#rrggbb` -> [`Color::Rgb`], for transcribing nvim-web-devicons colours literally.
const fn hex(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Per-type logo + colour for Nerd Font mode, mirroring nvim-web-devicons. The colours are its
/// `color` (GUI) values; a handful of near-white greys (markdown/text/config/default) are
/// nudged to a mid grey so they stay legible on the light theme too, where devicons' own light
/// values would wash out.
///
/// Codepoints follow the Nerd Fonts cheat sheet (nerdfonts.com/cheat-sheet). The Font Awesome
/// `file-*` glyphs (`\u{f1c*}`) and the common language logos are long-stable; if a rarer one
/// renders as a box in your terminal, swap the codepoint on that single line.
fn nerdfont_file_icon(ext: Option<&str>) -> (&'static str, Color) {
    let grey = hex(0x7c7c7c); // legible on both themes; used for the near-white devicons greys
    match ext {
        // ── languages ──────────────────────────────────────────────────────────────────
        Some("rs") => ("\u{e7a8}", hex(0xdea584)),                       // nf-dev-rust
        Some("py" | "pyw" | "pyi") => ("\u{e606}", hex(0xffbc03)),       // nf-seti-python
        Some("js" | "mjs" | "cjs") => ("\u{e74e}", hex(0xcbcb41)),       // nf-seti-javascript
        Some("jsx") => ("\u{e7ba}", hex(0x519aba)),                      // nf-dev-react
        Some("ts") => ("\u{e628}", hex(0x519aba)),                       // nf-seti-typescript
        Some("tsx") => ("\u{e7ba}", hex(0x519aba)),                      // nf-dev-react
        Some("go") => ("\u{e627}", hex(0x519aba)),                       // nf-seti-go
        Some("java") => ("\u{e738}", hex(0xcc3e44)),                     // nf-dev-java
        Some("c") => ("\u{e61e}", hex(0x599eff)),                        // nf-custom-c
        Some("h") => ("\u{e61e}", hex(0xa074c4)),                        // nf-custom-c (header)
        Some("cpp" | "cc" | "cxx") => ("\u{e61d}", hex(0xf34b7d)),       // nf-custom-cpp
        Some("hpp" | "hxx") => ("\u{e61d}", hex(0xa074c4)),              // nf-custom-cpp (header)
        Some("php") => ("\u{e73d}", hex(0xa074c4)),                      // nf-dev-php
        Some("lua") => ("\u{e620}", hex(0x51a0cf)),                      // nf-seti-lua
        Some("vim") => ("\u{e62b}", hex(0x019833)),                      // nf-seti-vim
        Some("sh" | "bash" | "zsh" | "fish") => ("\u{f489}", hex(0x89e051)), // nf-oct-terminal
        // ── web ────────────────────────────────────────────────────────────────────────
        Some("html" | "htm") => ("\u{e736}", hex(0xe34c26)),            // nf-dev-html5
        Some("css") => ("\u{e749}", hex(0x563d7c)),                     // nf-dev-css3
        Some("scss" | "sass") => ("\u{e749}", hex(0xcf649a)),           // nf-dev-css3
        Some("json" | "jsonc") => ("\u{e60b}", hex(0xcbcb41)),          // nf-seti-json
        Some("xml") => ("\u{f121}", hex(0xe37933)),                     // nf-fa-code
        // ── docs / text ────────────────────────────────────────────────────────────────
        Some("md" | "markdown") => ("\u{e73e}", grey),                  // nf-dev-markdown (#dddddd)
        Some("txt" | "text" | "log") => ("\u{f0f6}", grey),            // nf-fa-file_text_o
        Some("rst") => ("\u{f0f6}", grey),                              // nf-fa-file_text_o
        Some("pdf") => ("\u{f1c1}", hex(0xe5252a)),                     // nf-fa-file_pdf_o
        Some("doc" | "docx") => ("\u{f1c2}", hex(0x185abd)),            // nf-fa-file_word_o
        Some("ppt" | "pptx") => ("\u{f1c4}", hex(0xcb4a32)),           // nf-fa-file_powerpoint_o
        // ── data / config (the yaml/csv the demo shows) ────────────────────────────────
        Some("yaml" | "yml") => ("\u{e615}", hex(0x6d8086)),           // nf-seti-config
        Some("toml") => ("\u{e615}", hex(0x9c4221)),                    // nf-seti-config
        Some("ini" | "cfg" | "conf" | "env") => ("\u{f013}", hex(0x6d8086)), // nf-fa-cog
        Some("csv" | "tsv") => ("\u{f1c3}", hex(0x89e051)),            // nf-fa-file_excel_o
        Some("xls" | "xlsx") => ("\u{f1c3}", hex(0x207245)),           // nf-fa-file_excel_o
        Some("sql") => ("\u{f1c0}", hex(0xdad8d8)),                     // nf-fa-database
        Some("lock") => ("\u{f023}", hex(0xbbbbbb)),                    // nf-fa-lock
        // ── media ──────────────────────────────────────────────────────────────────────
        Some("svg") => ("\u{f1c5}", hex(0xffb13b)),                     // nf-fa-file_image_o
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico") => ("\u{f1c5}", hex(0xa074c4)),
        Some("mp4" | "mov" | "avi" | "mkv" | "webm") => ("\u{f1c8}", hex(0xfd971f)), // video
        Some("mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a") => ("\u{f1c7}", hex(0x00afaa)), // audio
        // ── archives ───────────────────────────────────────────────────────────────────
        Some("zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz" | "zst") => {
            ("\u{f1c6}", hex(0xeca517)) // nf-fa-file_archive_o
        }
        _ => ("\u{f016}", grey), // nf-fa-file_o
    }
}

/// Glyphs for the action iconography outside the file/dir icons above: clipboard staging,
/// transfer-job kind/status. Kept to actions where a Nerd Font icon is a real improvement
/// over the existing symbol (a purpose-drawn "copy" glyph vs. a dingbat) — plain typographic
/// marks used elsewhere (→ ← · + ~ = -) are left alone since they already read cleanly in any
/// font and Nerd Font has no equivalent that would look better.
#[derive(Clone, Copy)]
pub enum Glyph {
    ClipboardCopy,
    ClipboardMove,
    JobDownload,
    JobUpload,
    JobBundle,
    JobDelete,
    StatusDone,
    /// Same-store copy/move completing — reads as "arrived", not just "done".
    StatusArrived,
    StatusCancelled,
    StatusFailed,
    /// The sort-active badge in a pane title, e.g. " ⇅ size ↓".
    Sort,
}

pub fn glyph(g: Glyph, icons: IconSet) -> &'static str {
    use Glyph::*;
    if !matches!(icons, IconSet::Nerdfont) {
        return match g {
            ClipboardCopy => "⧉",
            ClipboardMove => "✂",
            JobDownload => "↓",
            JobUpload => "↑",
            JobBundle => "⇊",
            JobDelete => "⌫",
            StatusDone => "✓",
            StatusArrived => "→",
            StatusCancelled => "⊘",
            StatusFailed => "✗",
            Sort => "⇅",
        };
    }
    match g {
        ClipboardCopy => "\u{f0c5}",  // nf-fa-copy
        ClipboardMove => "\u{f0c4}",  // nf-fa-cut (scissors)
        JobDownload => "\u{f019}",    // nf-fa-download
        JobUpload => "\u{f093}",      // nf-fa-upload
        JobBundle => "\u{f187}",      // nf-fa-archive
        JobDelete => "\u{f1f8}",      // nf-fa-trash
        StatusDone => "\u{f00c}",     // nf-fa-check
        StatusArrived => "\u{f061}",  // nf-fa-arrow_right
        StatusCancelled => "\u{f05e}", // nf-fa-ban
        StatusFailed => "\u{f00d}",   // nf-fa-times
        Sort => "\u{f0dc}",           // nf-fa-sort
    }
}

/// Resolves a configured [`IconSet`] to a concrete choice, probing for an installed Nerd
/// Font when the config says `Auto` (the default). Do this once at startup and cache the
/// result on `App` — the probe walks font directories on disk.
pub fn resolve_icon_set(configured: IconSet) -> IconSet {
    match configured {
        IconSet::Auto => {
            if nerd_font_installed() {
                IconSet::Nerdfont
            } else {
                IconSet::Unicode
            }
        }
        other => other,
    }
}

/// Best-effort probe for whether *some* Nerd Font is installed on the system. This only
/// checks that a patched font file exists on disk somewhere comhad knows to look — it can't
/// see which font the terminal emulator is actually rendering with, so it's a heuristic, not
/// a guarantee. `icons = "nerdfont"` / `"unicode"` in config.toml bypass it entirely.
fn nerd_font_installed() -> bool {
    let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
    if let Ok(home) = std::env::var("HOME") {
        let home = Path::new(&home);
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
        dirs.push(home.join("Library/Fonts")); // macOS, user-installed
    }
    dirs.push(PathBuf::from("/Library/Fonts")); // macOS, system-installed
    dirs.iter().any(|dir| dir_has_nerd_font(dir, 2))
}

/// Recursively checks up to `depth` levels for a font file whose name marks it as a Nerd
/// Fonts patched font — the project always includes "Nerd Font" in patched font names.
fn dir_has_nerd_font(dir: &Path, depth: u8) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 && dir_has_nerd_font(&path, depth - 1) {
                return true;
            }
            continue;
        }
        let matches = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| {
                let lower = name.to_lowercase();
                lower.contains("nerd font") || lower.contains("nerdfont")
            })
            .unwrap_or(false);
        if matches {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PaletteOverride;

    #[test]
    fn parse_hex_color_accepts_with_and_without_hash() {
        assert_eq!(parse_hex_color("#ff8800"), Some(Color::Rgb(0xff, 0x88, 0x00)));
        assert_eq!(parse_hex_color("ff8800"), Some(Color::Rgb(0xff, 0x88, 0x00)));
    }

    #[test]
    fn parse_hex_color_rejects_malformed_input() {
        assert_eq!(parse_hex_color("#ff88"), None);
        assert_eq!(parse_hex_color("not-a-color"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn with_overrides_changes_only_set_fields() {
        let base = Palette::light();
        let overrides = PaletteOverride { accent: Some("#ff8800".to_string()), ..Default::default() };
        let overridden = base.with_overrides(&overrides);

        assert_eq!(overridden.accent, Color::Rgb(0xff, 0x88, 0x00));
        assert_eq!(overridden.bg, base.bg);
        assert_eq!(overridden.text, base.text);
    }

    #[test]
    fn with_overrides_ignores_an_unparseable_hex_value() {
        let base = Palette::dark();
        let overrides = PaletteOverride { accent: Some("garbage".to_string()), ..Default::default() };
        let overridden = base.with_overrides(&overrides);
        assert_eq!(overridden.accent, base.accent);
    }
}
