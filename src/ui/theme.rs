use ratatui::style::Color;

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
}

pub const SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(frame: usize) -> &'static str {
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

/// Deliberately a small set of old (pre-2016 Unicode), single-codepoint, default-emoji-
/// presentation glyphs — no variation selectors (`\u{fe0f}`), which is what caused border
/// and column misalignment: several "nicer" icons (🖼️ 🗂️ ☁️ 👁) default to *text* presentation
/// and only render as emoji with a trailing variation selector, so their on-screen width
/// disagrees with what ratatui/unicode-width computes depending on the font and terminal.
pub fn icon_for(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "📁";
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
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
