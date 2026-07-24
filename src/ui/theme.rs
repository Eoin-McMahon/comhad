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
    pub fn dark() -> Self {
        Self {
            bg: Color::Rgb(0x1a, 0x18, 0x16),
            panel_bg: Color::Rgb(0x21, 0x1f, 0x1c),
            accent: Color::Rgb(0xd9, 0x77, 0x57), // Claude terracotta
            accent_dim: Color::Rgb(0x8a, 0x53, 0x40),
            on_accent: Color::Rgb(0x1a, 0x18, 0x16),
            text: Color::Rgb(0xe8, 0xe1, 0xd8),
            muted: Color::Rgb(0x8a, 0x82, 0x78),
            good: Color::Rgb(0x8f, 0xb2, 0x76),
            bad: Color::Rgb(0xd9, 0x6a, 0x5c),
            dir: Color::Rgb(0xe3, 0xa8, 0x5a),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: Color::Rgb(0xfa, 0xf7, 0xf2),
            panel_bg: Color::Rgb(0xf1, 0xec, 0xe3),
            accent: Color::Rgb(0xc1, 0x5f, 0x42), // Claude terracotta, darkened for contrast
            accent_dim: Color::Rgb(0xd9, 0xb3, 0xa4),
            on_accent: Color::Rgb(0xfa, 0xf7, 0xf2),
            text: Color::Rgb(0x2b, 0x27, 0x22),
            muted: Color::Rgb(0x7a, 0x72, 0x67),
            good: Color::Rgb(0x4c, 0x7a, 0x36),
            bad: Color::Rgb(0xb8, 0x3a, 0x2b),
            dir: Color::Rgb(0xa3, 0x6a, 0x14),
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
/// Awesome sets bundled with Nerd Fonts.
pub fn icon_for(name: &str, is_dir: bool, icons: IconSet) -> &'static str {
    if is_dir {
        return match icons {
            IconSet::Unicode | IconSet::Auto => "📁",
            IconSet::Nerdfont => "\u{f07b}", // nf-fa-folder
        };
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    if icons == IconSet::Nerdfont {
        return nerdfont_file_icon(ext.as_deref());
    }
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp") => "📷",
        Some("mp4" | "mov" | "avi" | "mkv" | "webm") => "🎬",
        Some("mp3" | "wav" | "flac" | "aac" | "ogg") => "🎵",
        Some("zip" | "tar" | "gz" | "7z" | "rar" | "bz2") => "📦",
        Some("pdf") => "📕",
        Some("md" | "txt" | "rst") => "📝",
        Some("rs" | "py" | "js" | "ts" | "go" | "java" | "c" | "cpp" | "sh") => "💻",
        _ => "📄",
    }
}

/// Per-language logo for Nerd Font mode. Codepoints are transcribed from the Nerd Fonts
/// cheat sheet (nerdfonts.com/cheat-sheet) from memory rather than rendered and checked here
/// — the common ones (rust/python/js/ts/html/css) are long-stable and high-confidence, but
/// spot-check the rest against the cheat sheet and swap the codepoint on that one line if a
/// glyph looks wrong in your terminal.
fn nerdfont_file_icon(ext: Option<&str>) -> &'static str {
    match ext {
        Some("rs") => "\u{e7a8}",                              // nf-dev-rust
        Some("py") => "\u{e606}",                               // nf-seti-python
        Some("js" | "mjs" | "cjs") => "\u{e74e}",                // nf-seti-javascript
        Some("ts" | "tsx") => "\u{e628}",                        // nf-seti-typescript
        Some("go") => "\u{e627}",                                // nf-seti-go
        Some("java") => "\u{e738}",                              // nf-dev-java
        Some("c") => "\u{e61e}",                                 // nf-custom-c
        Some("cpp" | "cc" | "cxx" | "h" | "hpp") => "\u{e61d}",  // nf-custom-cpp
        Some("sh" | "bash" | "zsh") => "\u{f489}",               // nf-oct-terminal
        Some("html" | "htm") => "\u{e736}",                      // nf-dev-html5
        Some("css" | "scss" | "sass") => "\u{e749}",             // nf-dev-css3
        Some("json") => "\u{e60b}",                              // nf-seti-json
        Some("md" | "markdown") => "\u{e73e}",                   // nf-dev-markdown
        Some("txt" | "rst") => "\u{f0f6}",                       // nf-fa-file_text_o
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp") => "\u{f1c5}", // nf-fa-file_image_o
        Some("mp4" | "mov" | "avi" | "mkv" | "webm") => "\u{f1c8}", // nf-fa-file_video_o
        Some("mp3" | "wav" | "flac" | "aac" | "ogg") => "\u{f1c7}", // nf-fa-file_audio_o
        Some("zip" | "tar" | "gz" | "7z" | "rar" | "bz2") => "\u{f1c6}", // nf-fa-file_archive_o
        Some("pdf") => "\u{f1c1}",                               // nf-fa-file_pdf_o
        _ => "\u{f016}",                                         // nf-fa-file_o
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
