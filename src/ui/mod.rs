pub mod theme;
mod worldmap;

use std::cell::RefCell;

use humansize::{format_size, BINARY};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Sparkline, Wrap,
};
use ratatui::Frame;

use crate::app::{
    App, ClipMode, Focus, Preview, PreviewMode, PromptKind, RowTransfer, Screen, SyncAction, SyncDirection,
    BOOKMARK_FIELDS,
};
use crate::jobs::{JobKind, JobStatus};
use theme::Palette;

/// If `app.recent_write` is currently the local `path` and hasn't fully faded, the 0..1 fade
/// ratio (0 = just written, full `good`; 1 = fully back to normal) — else `None`.
fn recent_local_write_ratio(app: &App, path: &std::path::Path) -> Option<f64> {
    let (current, at) = app.recent_write.as_ref()?;
    match current {
        crate::app::RecentWrite::Local(p) if p == path => {}
        _ => return None,
    }
    let ratio = at.elapsed().as_secs_f64() / crate::app::RECENT_WRITE_FADE.as_secs_f64();
    (ratio < 1.0).then_some(ratio.clamp(0.0, 1.0))
}

/// Remote counterpart to [`recent_local_write_ratio`].
fn recent_remote_write_ratio(app: &App, bucket: &str, key: &str) -> Option<f64> {
    let (current, at) = app.recent_write.as_ref()?;
    match current {
        crate::app::RecentWrite::Remote { bucket: b, key: k } if b == bucket && k == key => {}
        _ => return None,
    }
    let ratio = at.elapsed().as_secs_f64() / crate::app::RECENT_WRITE_FADE.as_secs_f64();
    (ratio < 1.0).then_some(ratio.clamp(0.0, 1.0))
}

/// Blends from `from` toward `base` by `ratio` (0 = `from`, 1 = `base`), for the recent-write fade.
fn fade_toward(base: Color, from: Color, ratio: f64) -> Color {
    match (from, base) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(br, bg, bb)) => {
            let lerp = |f: u8, b: u8| (f as f64 + (b as f64 - f as f64) * ratio).round() as u8;
            Color::Rgb(lerp(fr, br), lerp(fg, bg), lerp(fb, bb))
        }
        _ => base,
    }
}

/// Paints a listing row as its own progress bar: the leading `ratio` of `width` columns takes the
/// filled background, the rest takes the track background (`None` leaves it as the pane's). The
/// span straddling the boundary is split so the edge lands on an exact column, and the row is
/// padded out to `width` first so the fill reaches the pane's right edge.
///
/// Foregrounds are untouched: a transferring row keeps its icon colours, fuzzy-match highlights
/// and selection styling, and gains a background that advances underneath them.
fn progress_fill(spans: Vec<Span<'static>>, width: u16, ratio: f64, filled: Color, track: Option<Color>) -> Vec<Span<'static>> {
    let width = width as usize;
    // Columns are counted in `chars`, matching how `highlight_spans` pads a row.
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let mut spans = spans;
    if used < width {
        let pad = Style::default();
        spans.push(Span::styled(" ".repeat(width - used), pad));
    }

    let boundary = (ratio.clamp(0.0, 1.0) * width as f64).round() as usize;
    let paint = |span: Span<'static>, bg: Option<Color>| match bg {
        Some(bg) => Span::styled(span.content, span.style.bg(bg)),
        None => span,
    };

    let mut out = Vec::with_capacity(spans.len() + 1);
    let mut col = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        if col >= boundary {
            out.push(paint(span, track));
        } else if col + len <= boundary {
            out.push(paint(span, Some(filled)));
        } else {
            let split = boundary - col;
            let head: String = span.content.chars().take(split).collect();
            let tail: String = span.content.chars().skip(split).collect();
            let style = span.style;
            out.push(paint(Span::styled(head, style), Some(filled)));
            out.push(paint(Span::styled(tail, style), track));
        }
        col += len;
    }
    out
}

/// Backgrounds for a row being transferred: the filled part, then the track behind the rest.
///
/// On an unselected row the fill is a tint of `accent` over the pane background, dark enough to
/// leave the filename readable. On the selected row the accent highlight is already the whole
/// background, so the bar inverts: the full accent marks progress and the track dims back toward
/// the pane, which keeps the boundary visible in both palettes (`accent` and `accent_dim` are
/// nearly the same colour in the light theme, so those two alone wouldn't read as a bar).
fn progress_fill_colors(selected: bool, p: &Palette) -> (Color, Option<Color>) {
    if selected {
        (p.accent, Some(fade_toward(p.panel_bg, p.accent, 0.6)))
    } else {
        (fade_toward(p.panel_bg, p.accent, 0.7), None)
    }
}

/// The marker shown before a row's icon: staged copy/cut, a plain "marked" checkbox, or blank.
/// Routed through `theme::glyph` (not literal chars) so it honours the Nerd Font setting like
/// every other icon, and padded the same way the jobs legend/job rows are. A Nerd Font
/// glyph renders wider than its reported width and would collide with the icon right after it.
fn row_mark(marked: bool, clip_mode: Option<ClipMode>, icons: crate::config::IconSet) -> String {
    let pad = if matches!(icons, crate::config::IconSet::Nerdfont) { "  " } else { " " };
    match (marked, clip_mode) {
        (_, Some(ClipMode::Copy)) => format!("{}{pad}", theme::glyph(theme::Glyph::ClipboardCopy, icons)),
        (_, Some(ClipMode::Move)) => format!("{}{pad}", theme::glyph(theme::Glyph::ClipboardMove, icons)),
        (true, None) => format!("{}{pad}", theme::glyph(theme::Glyph::StatusDone, icons)),
        (false, None) => format!(" {pad}"),
    }
}

/// Splits `text` into spans, highlighting characters matched by `needle`'s fuzzy filter with
/// `match_style` (rest keeps `base_style`), then pads to `width` chars.
fn highlight_spans(text: &str, needle: Option<&str>, width: usize, base_style: Style, match_style: Style) -> Vec<Span<'static>> {
    let matched: Vec<usize> = match needle {
        Some(n) if !n.is_empty() => crate::fuzzy::fuzzy_positions(text, n).unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    let mut len = 0;
    for (i, c) in text.chars().enumerate() {
        let is_matched = matched.contains(&i);
        if !run.is_empty() && is_matched != run_matched {
            spans.push(Span::styled(std::mem::take(&mut run), if run_matched { match_style } else { base_style }));
        }
        run_matched = is_matched;
        run.push(c);
        len += 1;
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_matched { match_style } else { base_style }));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }
    if len < width {
        spans.push(Span::styled(" ".repeat(width - len), base_style));
    }
    spans
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let p = app.palette();
    f.render_widget(Block::default().style(Style::default().bg(p.bg)), f.area());

    match app.screen {
        Screen::ConnectionPicker => draw_connection_picker(f, app, &p),
        Screen::BucketPicker => draw_bucket_picker(f, app, &p),
        Screen::Browser => draw_browser(f, app, &p),
    }

    if app.sync.is_some() {
        draw_sync(f, app, &p);
    }
    if let Some(action) = &app.confirm_action {
        draw_confirm_action(f, action, app.confirm_target_name().is_some(), &p);
    }
    if let Some(path) = &app.confirm_bookmark_delete {
        draw_confirm_bookmark_delete(f, path, &p);
    }
    if let Some(prompt) = &app.prompt {
        // Filter prompt renders inline in the pane header (k9s-style), not as a popup.
        if prompt.kind != PromptKind::Filter {
            draw_prompt(f, app, prompt, &p);
        }
    }
    if app.show_events {
        draw_events(f, app, &p);
    }
    if app.show_help {
        draw_help(f, app, &p);
    }
}

fn title_line(p: &Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled("✦ ", Style::default().fg(p.accent)),
        Span::styled("comhad", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
    ])
}

fn draw_connection_picker(f: &mut Frame, app: &App, p: &Palette) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let header = Paragraph::new(title_line(p))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(p.accent_dim)));
    f.render_widget(header, chunks[0]);

    let items: Vec<ListItem> = if app.connections.is_empty() {
        vec![ListItem::new(Span::styled(
            "  no bookmarks yet, press 'a' to add one",
            Style::default().fg(p.muted),
        ))]
    } else {
        app.connections
            .iter()
            .enumerate()
            .map(|(i, (_, conn))| {
                let selected = i == app.conn_selected;
                let marker = if selected { "➜ " } else { "  " };
                let style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p.text)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(format!("{:<20}", conn.name), style),
                    Span::styled(format!(" {}", conn.remote_path), Style::default().fg(p.muted)),
                ]))
                .style(style)
            })
            .collect()
    };

    let list = List::new(items).block(
        Block::default()
            .title(" select a bookmark ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent_dim))
            .style(Style::default().bg(p.panel_bg)),
    );

    // List on the left, an info panel on the right for the highlighted bookmark: the region globe
    // over its config (credentials masked). Shown here (not only on the bucket picker, which is
    // skipped when list-buckets isn't permitted) so it's always seen and follows the selection.
    let body = centered_rect(88, 74, chunks[1]);
    let wide = body.width >= 80 && !app.connections.is_empty();
    if wide {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(body);
        f.render_widget(list, cols[0]);
        // While editing, the info pane becomes the inline editor; otherwise it's the read-only
        // config view with the region globe.
        if let Some(editor) = &app.bookmark_editor {
            draw_bookmark_editor(f, editor, cols[1], p);
        } else if let Some((_, conn)) = app.connections.get(app.conn_selected) {
            let region = conn.region.as_deref().unwrap_or("");
            draw_globe_info(f, cols[1], region, config_body(conn, p), Line::from(" connection "), Style::default().fg(p.accent_dim), p, 0);
        }
    } else {
        f.render_widget(list, body);
    }

    // Too narrow to show the side info pane, but the user pressed `e` to edit: float the editor as
    // a centered popup so it stays visible and reachable.
    if !wide && let Some(editor) = &app.bookmark_editor {
        let popup = centered_rect(90, 90, chunks[1]);
        f.render_widget(Clear, popup);
        draw_bookmark_editor(f, editor, popup, p);
    }

    let hint = if app.bookmark_editor.is_some() {
        "  ↑/↓ field   ←/→ move · switch button   ^R reveal keys   enter next/confirm   esc discard"
    } else {
        "  ↑/↓ move   enter connect   a add   e edit   r profile   x delete   t theme   q quit"
    };
    let footer = Paragraph::new(Line::from(vec![Span::styled(hint, Style::default().fg(p.muted))]));
    f.render_widget(footer, chunks[2]);
}

/// Renders the inline bookmark editor into `area`: one labelled, editable row per field with a
/// block cursor on the focused one, and a Save / Discard button row anchored at the bottom.
fn draw_bookmark_editor(f: &mut Frame, editor: &crate::app::BookmarkEditor, area: Rect, p: &Palette) {
    use crate::app::{EditorFocus, EDITOR_FIELDS, EDITOR_LABELS};

    let block = Block::default()
        .title(Line::from(" edit bookmark "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.panel_bg));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 3 || inner.width < 4 {
        return;
    }

    // Fields fill the top; the button row is pinned to the last line.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(2)])
        .split(inner);

    let mut lines: Vec<Line> = Vec::with_capacity(EDITOR_FIELDS.len());
    for &i in EDITOR_FIELDS.iter() {
        let credential = crate::app::is_credential_field(i);
        let focused = editor.focus == EditorFocus::Field(i);
        let mut spans = vec![Span::styled(
            format!("{}: ", EDITOR_LABELS[i]),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        )];
        spans.extend(field_value_spans(
            &editor.values[i],
            editor.cursor,
            focused,
            credential,
            editor.reveal_secrets,
            editor_field_color(i, p),
            p,
        ));
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rows[0]);

    let button = |label: &str, focused: bool, color: Color| {
        let style = if focused {
            Style::default().fg(p.on_accent).bg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        Span::styled(format!(" {label} "), style)
    };
    let buttons = Line::from(vec![
        button("Save", editor.focus == EditorFocus::Save, p.good),
        Span::raw("   "),
        button("Discard", editor.focus == EditorFocus::Discard, p.bad),
    ])
    .alignment(Alignment::Center);
    f.render_widget(Paragraph::new(buttons), rows[1]);
}

/// Per-field value colour in the inline editor, matching the read-only info panel's palette
/// ([`config_body`]) so the highlight colouring carries over when the edit dialog opens.
fn editor_field_color(index: usize, p: &Palette) -> Color {
    match index {
        1 => p.text,       // Name
        2 => p.dir,        // Endpoint
        3 | 4 => p.muted,  // Access key / Secret key (credentials)
        5 => p.good,       // Remote path (bucket/prefix)
        6 => p.dir,        // Local path
        7 => p.dir,        // Web URL
        8 => p.accent_dim, // Region
        _ => p.text,
    }
}

/// Splits `text` into spans, drawing `${VAR}` environment-variable references in accent so they
/// stand out from literal characters while editing a credential. Literal runs use `base`.
fn env_highlight_spans(text: &str, base: Style, p: &Palette) -> Vec<Span<'static>> {
    let var = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_string(), base));
        }
        match rest[start..].find('}') {
            Some(end) => {
                let end_abs = start + end + 1;
                spans.push(Span::styled(rest[start..end_abs].to_string(), var));
                rest = &rest[end_abs..];
            }
            // Unterminated `${`, treat the remainder as literal.
            None => {
                spans.push(Span::styled(rest[start..].to_string(), base));
                rest = "";
                break;
            }
        }
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), base));
    }
    spans
}

/// Spans for one editable field value in the inline editor.
///
/// Credential fields (`credential`) are handled specially: while blank they show a dim
/// "unchanged" placeholder (the stored secret is kept, never displayed); once typed they mask as
/// bullets unless `reveal` is on, in which case the literal text shows with `${VAR}` highlighting.
/// The focused field carries a reverse-video block cursor at `cursor` (a trailing block at the end).
fn field_value_spans(
    value: &str,
    cursor: usize,
    focused: bool,
    credential: bool,
    reveal: bool,
    base_color: Color,
    p: &Palette,
) -> Vec<Span<'static>> {
    let base = Style::default().fg(base_color);
    let cursor_style = Style::default().fg(p.bg).bg(p.accent);

    // Untouched credential: a placeholder standing in for the kept-as-is stored secret.
    if credential && value.is_empty() {
        let placeholder = Span::styled("•••••••• (unchanged)".to_string(), Style::default().fg(p.muted).add_modifier(Modifier::DIM));
        return if focused {
            vec![Span::styled("▏".to_string(), Style::default().fg(p.accent)), placeholder]
        } else {
            vec![placeholder]
        };
    }

    let masked = credential && !reveal;
    let chars: Vec<char> = value.chars().collect();

    // Renders a run of the value with whatever styling this field needs (bullets when masked,
    // env-highlighted when a revealed credential, plain otherwise).
    let render_seg = |seg: &[char]| -> Vec<Span<'static>> {
        if masked {
            vec![Span::styled("•".repeat(seg.len()), base)]
        } else if credential {
            env_highlight_spans(&seg.iter().collect::<String>(), base, p)
        } else {
            vec![Span::styled(seg.iter().collect::<String>(), base)]
        }
    };

    if !focused {
        return render_seg(&chars);
    }

    let mut spans = render_seg(&chars[..cursor.min(chars.len())]);
    match chars.get(cursor) {
        Some(&c) => {
            let shown = if masked { '•' } else { c };
            spans.push(Span::styled(shown.to_string(), cursor_style));
            spans.extend(render_seg(&chars[cursor + 1..]));
        }
        None => spans.push(Span::styled(" ".to_string(), cursor_style)),
    }
    spans
}

/// Approximate `(longitude, latitude)` of an AWS region's home city, for the region map dot.
/// Longitude/latitude line up with the [`Canvas`] EPSG:4326 bounds. Unknown regions (or
/// endpoints with no region) return `None`, and the map just shows no dot.
fn region_coords(region: &str) -> Option<(f64, f64)> {
    let point = match region {
        "us-east-1" => (-77.5, 37.9),   // N. Virginia
        "us-east-2" => (-83.0, 40.0),   // Ohio
        "us-west-1" => (-121.9, 37.4),  // N. California
        "us-west-2" => (-123.0, 44.0),  // Oregon
        "ca-central-1" => (-73.6, 45.5), // Montreal
        "sa-east-1" => (-46.6, -23.5),  // São Paulo
        "eu-west-1" => (-6.3, 53.3),    // Ireland
        "eu-west-2" => (-0.1, 51.5),    // London
        "eu-west-3" => (2.35, 48.85),   // Paris
        "eu-central-1" => (8.68, 50.11), // Frankfurt
        "eu-central-2" => (8.54, 47.37), // Zurich
        "eu-north-1" => (18.07, 59.33), // Stockholm
        "eu-south-1" => (9.19, 45.46),  // Milan
        "eu-south-2" => (-3.7, 40.4),   // Spain
        "af-south-1" => (18.42, -33.92), // Cape Town
        "me-south-1" => (50.59, 26.07), // Bahrain
        "me-central-1" => (54.37, 24.45), // UAE
        "ap-south-1" => (72.88, 19.08), // Mumbai
        "ap-south-2" => (78.49, 17.39), // Hyderabad
        "ap-southeast-1" => (103.82, 1.35), // Singapore
        "ap-southeast-2" => (151.21, -33.87), // Sydney
        "ap-southeast-3" => (106.85, -6.21), // Jakarta
        "ap-northeast-1" => (139.69, 35.69), // Tokyo
        "ap-northeast-2" => (126.98, 37.57), // Seoul
        "ap-northeast-3" => (135.5, 34.69), // Osaka
        "ap-east-1" => (114.17, 22.32), // Hong Kong
        "cn-north-1" => (116.4, 39.9),  // Beijing
        "cn-northwest-1" => (106.2, 37.5), // Ningxia
        _ => return None,
    };
    Some(point)
}

/// Smooth 0→1 ramp between two edges (Hermite), for terminator and atmosphere blends.
fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear blend between two RGB triples (channels as 0..255 floats).
fn mix3(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t)
}

/// Cheap deterministic integer hash → [0,1), the basis for the value noise below.
fn hash2(x: i32, y: i32) -> f64 {
    let mut h = (x.wrapping_mul(374_761_393).wrapping_add(y.wrapping_mul(668_265_263))) as u32;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    h as f64 / u32::MAX as f64
}

/// Smoothed value noise in [0,1] via bilinear interpolation of a hashed lattice.
fn value_noise(x: f64, y: f64) -> f64 {
    let (xi, yi) = (x.floor(), y.floor());
    let (xf, yf) = (x - xi, y - yi);
    let (x0, y0) = (xi as i32, yi as i32);
    let (u, v) = (xf * xf * (3.0 - 2.0 * xf), yf * yf * (3.0 - 2.0 * yf));
    let top = hash2(x0, y0) * (1.0 - u) + hash2(x0 + 1, y0) * u;
    let bot = hash2(x0, y0 + 1) * (1.0 - u) + hash2(x0 + 1, y0 + 1) * u;
    top * (1.0 - v) + bot * v
}

/// Four-octave fractal value noise in [0,1], for terrain and cloud texture on the globe.
fn fbm(mut x: f64, mut y: f64) -> f64 {
    let (mut sum, mut amp, mut norm) = (0.0, 0.5, 0.0);
    for _ in 0..4 {
        sum += amp * value_noise(x, y);
        norm += amp;
        x *= 2.0;
        y *= 2.0;
        amp *= 0.5;
    }
    sum / norm
}

/// Renders a bordered "info" panel: a small region globe up top with a details body underneath,
/// used for the browser Info tab and the bookmark/bucket pickers. The globe's off-disk space is
/// the panel background (no hard black box) so it blends into the theme.
///
///
/// `scroll` scrolls the details body and draws a scrollbar when it overflows its area. The
/// bookmark/bucket picker callers (whose `config_body` never overflows) just pass `0`; the
/// browser's Info tab passes `app.info_scroll` since a file with a lot of backend metadata
/// genuinely can run past the pane's height.
#[allow(clippy::too_many_arguments)]
fn draw_globe_info(
    f: &mut Frame,
    area: Rect,
    region: &str,
    body: Text<'static>,
    title: Line<'static>,
    border: Style,
    p: &Palette,
    scroll: u16,
) {
    let block = Block::default().title(title).borders(Borders::ALL).border_style(border).style(Style::default().bg(p.panel_bg));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let body_area = if inner.height >= 12 && !region.is_empty() {
        // A fixed proportion of the pane, independent of how much detail text there is. It used
        // to be sized from the *remaining* space after the details claimed theirs, so the globe
        // visibly grew and shrank depending on how much metadata a given file happened to have.
        // The details area can scroll (see `scroll` below) if it doesn't fit in what's left.
        let globe_h = (inner.height * 3 / 5).min(inner.height.saturating_sub(3));
        render_globe(f, Rect { height: globe_h, ..inner }, region, p);
        Rect { y: inner.y + globe_h, height: inner.height - globe_h, ..inner }
    } else {
        inner
    };

    let total_lines = body.lines.len();
    let viewport = body_area.height as usize;
    let scroll = scroll.min(total_lines.saturating_sub(viewport) as u16);
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }).scroll((scroll, 0)), body_area);
    if total_lines > viewport && viewport > 0 {
        let mut state = ScrollbarState::new(total_lines).position(scroll as usize).viewport_content_length(viewport);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(p.accent_dim))
            .track_style(Style::default().fg(p.panel_bg));
        f.render_stateful_widget(bar, body_area, &mut state);
    }
}

/// Cache key for a rendered globe: region + panel size + a palette fingerprint (so a theme toggle
/// invalidates it). Only one globe is on screen at a time, so a single-entry cache suffices.
type GlobeKey = (String, u16, u16, Color, Color, Color, Color);

thread_local! {
    static GLOBE_CACHE: RefCell<Option<(GlobeKey, Vec<Line<'static>>)>> = const { RefCell::new(None) };
}

/// Draws the globe into `inner` (no surrounding block). Off-globe space is the panel background
/// with a scatter of faint stars; colours come from the palette so it blends with the theme. The
/// pixel raster is memoised per region/size/theme, since it's otherwise recomputed every frame the
/// info pane is on screen but only actually changes when one of those does.
fn render_globe(f: &mut Frame, inner: Rect, region: &str, p: &Palette) {
    if inner.width < 6 || inner.height < 4 {
        return;
    }
    let key: GlobeKey = (region.to_string(), inner.width, inner.height, p.accent, p.good, p.text, p.panel_bg);
    let lines = GLOBE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((cached_key, cached_lines)) = cache.as_ref() {
            if *cached_key == key {
                return cached_lines.clone();
            }
        }
        let lines = compute_globe_lines(inner.width, inner.height, region, p);
        *cache = Some((key, lines.clone()));
        lines
    });
    f.render_widget(Paragraph::new(lines), inner);
}

/// The expensive part of [`render_globe`]: rasterises the shaded sphere (plus stars and label) into
/// position-independent lines, so the result can be cached and re-rendered anywhere.
fn compute_globe_lines(width: u16, height: u16, region: &str, p: &Palette) -> Vec<Line<'static>> {
    let region_point = region_coords(region);
    let has_dot = region_point.is_some();
    let (true_lon_deg, true_lat_deg) = region_point.unwrap_or((0.0, 20.0));
    // The camera looks at a point offset from the true coordinate (not at the coordinate itself)
    // so the marker lands up-and-right of dead centre instead of the flat, staged-looking middle.
    // The marker's screen position is then computed with a real forward projection of the true
    // lat/lon against this rotated camera (see `dot_cx`/`dot_cy` below), so it's still exactly
    // where the datacenter is, not just a pixel shift that happens to land nearby.
    const DOT_OFFSET_LAT_DEG: f64 = 8.0;
    const DOT_OFFSET_LON_DEG: f64 = 14.0;
    let (lat0_deg, lon0_deg) = (true_lat_deg - DOT_OFFSET_LAT_DEG, true_lon_deg - DOT_OFFSET_LON_DEG);
    let (lat0, lon0) = (lat0_deg.to_radians(), lon0_deg.to_radians());

    // Pixel grid: one column per cell, two stacked pixels per cell (upper via fg, lower via bg).
    let pw = width as f64;
    let ph = height as f64 * 2.0;
    let (pcx, pcy) = (pw / 2.0, ph / 2.0);
    let radius = (pw.min(ph) / 2.0) * 0.94; // 15% bigger than the original 0.82 margin

    // Light direction (top-left, toward the viewer) for the diffuse sphere shading.
    let light = {
        let (a, b, c) = (-0.4_f64, 0.5, 0.77);
        let n = (a * a + b * b + c * c).sqrt();
        (a / n, b / n, c / n)
    };

    // Every globe colour is derived from the palette so it can't clash with the theme.
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
        _ => (128.0, 128.0, 128.0),
    };
    let scale = |c: (f64, f64, f64), f: f64| (c.0 * f, c.1 * f, c.2 * f);
    // The globe surface uses a FIXED Earth palette, identical in light and dark themes. Deriving
    // these from the app palette turned clouds/ice/highlights near-black in light mode. Only the
    // off-globe space blends with the (theme-aware) panel background.
    let deep_ocean = (22.0, 58.0, 124.0);
    let shallow_ocean = (66.0, 152.0, 198.0);
    let land_lo = (36.0, 100.0, 52.0);
    let land_hi = (110.0, 178.0, 96.0);
    let cloud = (232.0, 238.0, 244.0);
    let ice = (222.0, 232.0, 240.0);
    let night = (18.0, 22.0, 38.0);
    let atmo = (98.0, 158.0, 216.0);
    let space = rgb(p.panel_bg);
    let space_color = Color::Rgb(space.0 as u8, space.1 as u8, space.2 as u8);

    // Colour of a single pixel, or `None` for empty space outside the globe disk.
    let pixel = |px: f64, py: f64| -> Option<Color> {
        let (nx, ny) = ((px - pcx) / radius, (py - pcy) / radius);
        let rho2 = nx * nx + ny * ny;
        if rho2 > 1.0 {
            return None; // space
        }
        let (x, y) = (nx, -ny); // screen up = north
        let z = (1.0 - rho2).max(0.0).sqrt();
        let rho = rho2.sqrt();
        let ndotl = x * light.0 + y * light.1 + z * light.2;

        // Inverse orthographic projection back to latitude/longitude.
        let (lat, lon) = if rho < 1e-6 {
            (lat0, lon0)
        } else {
            let c = rho.min(1.0).asin();
            let (sinc, cosc) = (c.sin(), c.cos());
            let lat = (cosc * lat0.sin() + y * sinc * lat0.cos() / rho).clamp(-1.0, 1.0).asin();
            let lon = lon0 + (x * sinc).atan2(rho * cosc * lat0.cos() - y * sinc * lat0.sin());
            (lat, lon)
        };
        let (latd, lond) = (lat.to_degrees(), lon.to_degrees());

        // Surface: two-tone land / depth-varied ocean, textured with fractal noise.
        let mut base = if worldmap::is_land(latd, lond) {
            mix3(land_lo, land_hi, fbm(lond * 0.09 + 4.0, latd * 0.09))
        } else {
            let depth = fbm(lond * 0.06 + 20.0, latd * 0.06 + 3.0);
            mix3(deep_ocean, shallow_ocean, depth * depth)
        };
        // Polar ice fringe, then a drifting cloud layer over everything.
        base = mix3(base, ice, smoothstep(66.0, 76.0, latd.abs()));
        let cl = fbm(lond * 0.11 + 50.0, latd * 0.13 + 7.0);
        base = mix3(base, cloud, smoothstep(0.52, 0.70, cl) * 0.85);

        // Day/night terminator: blend from a dark side (near the panel bg) to a lit day side.
        let lit = smoothstep(-0.20, 0.32, ndotl);
        let day = {
            let b = 0.72 + 0.45 * ndotl.max(0.0);
            (base.0 * b, base.1 * b, base.2 * b)
        };
        let nightc = mix3(scale(base, 0.10), night, 0.6);
        let mut col = mix3(nightc, day, lit);
        // Atmospheric glow on the lit limb.
        col = mix3(col, atmo, smoothstep(0.82, 1.0, rho) * 0.45 * (0.35 + 0.65 * lit));

        Some(Color::Rgb(
            col.0.clamp(0.0, 255.0) as u8,
            col.1.clamp(0.0, 255.0) as u8,
            col.2.clamp(0.0, 255.0) as u8,
        ))
    };

    // Forward-project the *true* coordinate onto the (rotated) camera view. Standard orthographic
    // projection, the inverse of what `pixel` above does, so the marker is the actual datacenter
    // position on screen, not an approximation.
    let (dot_cx, dot_cy) = {
        let (lat_r, lon_r) = (true_lat_deg.to_radians(), true_lon_deg.to_radians());
        let dlon = lon_r - lon0;
        let x = lat_r.cos() * dlon.sin();
        let y = lat0.cos() * lat_r.sin() - lat0.sin() * lat_r.cos() * dlon.cos();
        (pcx + x * radius, pcy - y * radius)
    };
    let dot_r = (radius * 0.045).clamp(0.9, 1.6);
    let is_dot = |px: f64, py: f64| has_dot && ((px - dot_cx).powi(2) + (py - dot_cy).powi(2)).sqrt() <= dot_r;

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(height as usize);
    for cy in 0..height {
        let mut spans: Vec<Span> = Vec::with_capacity(width as usize);
        for cx in 0..width {
            let pxf = cx as f64 + 0.5;
            let (ty, by) = (cy as f64 * 2.0 + 0.5, cy as f64 * 2.0 + 1.5);
            let color_of = |py: f64| -> Color {
                if is_dot(pxf, py) {
                    p.bad // region marker in the theme's red
                } else if let Some(c) = pixel(pxf, py) {
                    c
                } else {
                    space_color
                }
            };
            spans.push(Span::styled("▀", Style::default().fg(color_of(ty)).bg(color_of(by))));
        }
        lines.push(Line::from(spans));
    }

    // A handful of stars scattered at random over the empty space, a silver/gold mix (the
    // palette's grey and amber), kept clear of the panel edges. Random each time the globe is
    // rasterised (i.e. each cache miss), so they're not a fixed pattern.
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(region.len() as u64);
    let mut rng = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        seed >> 33
    };
    // A cell counts as space only if both of its stacked pixels are outside the globe disk.
    let is_space = |cx: u16, cy: u16| {
        let pxf = cx as f64 + 0.5;
        let dist = |py: f64| ((pxf - pcx).powi(2) + (py - pcy).powi(2)).sqrt();
        dist(cy as f64 * 2.0 + 0.5) > radius + 1.0 && dist(cy as f64 * 2.0 + 1.5) > radius + 1.0
    };
    if width > 6 && height > 4 {
        let mut placed = 0;
        for _ in 0..200 {
            if placed >= 8 {
                break;
            }
            let cx = 2 + (rng() as u16 % (width - 4));
            let cy = 1 + (rng() as u16 % (height - 2));
            if !is_space(cx, cy) {
                continue;
            }
            let star_color = if rng() & 1 == 0 { p.muted } else { p.dir }; // silver / gold
            let star_char = if rng() & 1 == 0 { "▀" } else { "▄" }; // upper / lower pixel
            if let Some(span) = lines.get_mut(cy as usize).and_then(|l| l.spans.get_mut(cx as usize)) {
                *span = Span::styled(star_char, Style::default().fg(star_color).bg(space_color));
            }
            placed += 1;
        }
    }

    // The region name, printed just above the (offset) dot on a panel-bg strip so it blends in.
    if !region.is_empty() {
        let label: Vec<char> = format!(" {region} ").chars().collect();
        let dot_col = dot_cx.round() as usize;
        let dot_row = (dot_cy / 2.0).round() as usize; // half-block rows cover 2 pixel-rows each
        let start = dot_col.saturating_sub(label.len() / 2);
        let label_row = dot_row.saturating_sub(1);
        if let Some(line) = lines.get_mut(label_row) {
            for (i, ch) in label.iter().enumerate() {
                if let Some(span) = line.spans.get_mut(start + i) {
                    *span =
                        Span::styled(ch.to_string(), Style::default().fg(p.text).bg(space_color).add_modifier(Modifier::BOLD));
                }
            }
        }
    }

    lines
}

/// Bookmark connection details for a picker's info panel: credentials shown only as a fixed-width
/// mask. The secret's value is never read here, so its length can't leak, the dots are a constant.
fn config_body(conn: &crate::config::Connection, p: &Palette) -> Text<'static> {
    let label = |s: &str| Span::styled(s.to_string(), Style::default().fg(p.accent).add_modifier(Modifier::BOLD));
    let value = |s: String, c: Color| Span::styled(s, Style::default().fg(c));

    let mut lines = vec![
        Line::from(vec![label("Name: "), Span::styled(conn.name.clone(), Style::default().fg(p.text).add_modifier(Modifier::BOLD))]),
        Line::from(vec![label("Endpoint: "), value(conn.endpoint.clone(), p.dir)]),
        // Fixed mask, constant length. The real credential values are never touched.
        Line::from(vec![label("Access key: "), value("••••••••".to_string(), p.muted)]),
        Line::from(vec![label("Secret key: "), value("••••••••".to_string(), p.muted)]),
        Line::from(vec![label("Remote path: "), value(conn.remote_path.clone(), p.good)]),
    ];
    if let Some(local) = &conn.local_path {
        lines.push(Line::from(vec![label("Local: "), value(local.clone(), p.dir)]));
    }
    if let Some(url) = &conn.web_url {
        lines.push(Line::from(vec![label("Web: "), value(url.clone(), p.dir)]));
    }
    lines.push(Line::from(vec![label("Region: "), value(conn.region.clone().unwrap_or_else(|| "auto-detect".to_string()), p.accent_dim)]));
    Text::from(lines)
}

fn draw_bucket_picker(f: &mut Frame, app: &App, p: &Palette) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let header = Paragraph::new(title_line(p))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(p.accent_dim)));
    f.render_widget(header, chunks[0]);

    let items: Vec<ListItem> = app
        .buckets
        .iter()
        .enumerate()
        .map(|(i, bucket)| {
            let selected = i == app.bucket_selected;
            let marker = if selected { "➜ " } else { "  " };
            let style = if selected {
                Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(p.text)
            };
            ListItem::new(Line::from(vec![Span::styled(marker, style), Span::styled(bucket.clone(), style)]))
                .style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" select a bucket ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent_dim))
            .style(Style::default().bg(p.panel_bg)),
    );

    // List on the left, an info panel on the right: the region globe over the connection's config
    // (credentials masked), so what you're about to browse is spelled out. On a narrow terminal the
    // panel is dropped and the list keeps the centered full-width layout.
    let body = centered_rect(88, 74, chunks[1]);
    if body.width >= 80 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(body);
        f.render_widget(list, cols[0]);
        if let Some(conn) = &app.connection {
            let region = app.client.as_ref().map(|c| c.region()).unwrap_or("");
            draw_globe_info(f, cols[1], region, config_body(conn, p), Line::from(" connection "), Style::default().fg(p.accent_dim), p, 0);
        }
    } else {
        f.render_widget(list, body);
    }

    let footer = Paragraph::new(Line::from(vec![Span::styled(
        "  ↑/↓ move   enter open   esc back   t theme",
        Style::default().fg(p.muted),
    )]));
    f.render_widget(footer, chunks[2]);
}

fn draw_browser(f: &mut Frame, app: &mut App, p: &Palette) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(6), Constraint::Length(1)])
        .split(area);

    let name = app.connection.as_ref().map(|c| c.name.as_str()).unwrap_or("?");
    let spin = if app.loading { theme::spinner(app.spinner_frame) } else { "✦" };
    let header_line = Line::from(vec![
        Span::styled(format!("{spin} "), Style::default().fg(p.accent)),
        Span::styled("comhad", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(name.to_string(), Style::default().fg(p.text)),
        Span::raw("  "),
        Span::styled(app.bucket.clone(), Style::default().fg(p.muted)),
    ]);
    let header = Paragraph::new(header_line)
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(p.accent_dim)));
    f.render_widget(header, chunks[0]);

    match (app.show_local, app.show_preview) {
        (true, true) => {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(34), Constraint::Percentage(34), Constraint::Percentage(32)])
                .split(chunks[1]);
            draw_local_pane(f, app, body[0], p);
            draw_remote_pane(f, app, body[1], p);
            draw_preview_pane(f, app, body[2], p);
        }
        (true, false) => {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);
            draw_local_pane(f, app, body[0], p);
            draw_remote_pane(f, app, body[1], p);
        }
        (false, true) => {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(chunks[1]);
            draw_remote_pane(f, app, body[0], p);
            draw_preview_pane(f, app, body[1], p);
        }
        (false, false) => {
            draw_remote_pane(f, app, chunks[1], p);
        }
    }

    draw_jobs_pane(f, app, chunks[2], p);

    draw_footer(f, app, chunks[3], p);
}

/// The bottom line: a status toast when there is one, otherwise the short keybind bar (just the
/// essentials, the full keymap lives behind `?`).
///
/// On top of either, the key that was just pressed is echoed for `KEY_FLASH_LIFETIME`: if it's
/// one of the bar's own actions the entry lights up green in place, and if it isn't, it appears
/// as a green chip pinned to the right edge and then disappears again. Between the two, every
/// press that does something has a visible trace, which is what makes a recording followable.
fn draw_footer(f: &mut Frame, app: &App, area: Rect, p: &Palette) {
    use crate::keys::BrowserAction as A;

    // The essentials only. Each glyph comes from the live keybind table rather than a literal,
    // so a remapped bind advertises the key that actually works; an unbound action drops off
    // the bar rather than pointing at a dead key.
    const BAR: &[(A, &str)] = &[
        (A::Help, "help"),
        (A::Filter, "filter"),
        (A::ToggleLocal, "local"),
        (A::PreviewTab, "preview"),
        (A::InfoTab, "info"),
        (A::Quit, "quit"),
    ];
    let bar: Vec<(A, String, &str)> = BAR
        .iter()
        .filter_map(|(action, label)| {
            let key = app.keybinds.browser_key(*action)?;
            Some((*action, crate::keys::key_label(key), *label))
        })
        .collect();

    let flashed = app.key_flash.as_ref();
    let lit = flashed.and_then(|f| f.action);
    // The chip is for presses the bar can't show: an action that isn't on it, or any action at
    // all while a status toast has taken the line over.
    let needs_chip = flashed
        .is_some_and(|f| app.status.is_some() || !f.action.is_some_and(|a| bar.iter().any(|(action, ..)| *action == a)));

    let line = if let Some((msg, is_err)) = &app.status {
        Line::from(vec![
            Span::styled(format!("  {msg}"), Style::default().fg(if *is_err { p.bad } else { p.good })),
            Span::styled("  (E for events)", Style::default().fg(p.muted)),
        ])
    } else {
        let mut spans = Vec::with_capacity(bar.len() * 2);
        for (action, glyph, label) in &bar {
            let on = lit == Some(*action);
            let key_style = if on {
                Style::default().fg(p.good).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(p.accent_dim).add_modifier(Modifier::BOLD)
            };
            let lbl_style =
                if on { Style::default().fg(p.good).add_modifier(Modifier::BOLD) } else { Style::default().fg(p.muted) };
            spans.push(Span::styled(format!("  {glyph} "), key_style));
            spans.push(Span::styled((*label).to_string(), lbl_style));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);

    if !needs_chip {
        return;
    }
    let Some(flash) = flashed else { return };
    let text = format!(" {} · {} ", flash.key, flash.label);
    let width = text.chars().count() as u16;
    if width + 2 > area.width {
        return; // too narrow to sit beside the bar without covering it
    }
    let chip = Rect { x: area.x + area.width - width, y: area.y, width, height: area.height };
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().bg(p.good).fg(p.on_accent).add_modifier(Modifier::BOLD))),
        chip,
    );
}

fn pane_border_style(focused: bool, p: &Palette) -> Style {
    if focused {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.accent_dim)
    }
}

/// When the filter prompt is open for `pane` (and focused), returns the k9s-style title that
/// replaces the pane header with a live filter input, e.g. ` local: foo▏ `.
fn active_filter_title(app: &App, pane: Focus) -> Option<String> {
    if app.focus != pane {
        return None;
    }
    let prompt = app.prompt.as_ref()?;
    if prompt.kind != PromptKind::Filter {
        return None;
    }
    let label = if pane == Focus::Local { "local" } else { "s3" };
    let (before, after) = prompt.split_at_cursor();
    Some(format!(" {label}: {before}▏{after} "))
}

/// Builds the remote pane's breadcrumb title plus its clickable regions: `(start_col, end_col,
/// jump_prefix)` in columns relative to the start of the title (col 0 = right after the pane's
/// left border). Clicking the bucket jumps to the root; clicking a path segment jumps to the
/// full prefix through that segment, even if elision hid some of the segments before it.
fn remote_breadcrumb_title(
    bucket: &str,
    prefix: &str,
    suffix: &str,
    max_width: u16,
    p: &Palette,
) -> (Line<'static>, Vec<(u16, u16, String)>) {
    let head = format!(" [2] s3://{bucket}");
    let fixed = head.chars().count() + suffix.chars().count() + 2; // trailing space + closing slash
    let budget = (max_width as usize).saturating_sub(fixed);

    // Deepest segments matter most (that's where the cursor is), so keep as many trailing
    // segments as fit and drop from the front first, marking the drop with a "/…" segment.
    // The pane border can't wrap a title onto a second line, so this is the elision instead.
    let segments: Vec<&str> = prefix.split('/').filter(|s| !s.is_empty()).collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for segment in segments.iter().rev() {
        let width = segment.chars().count() + 1; // leading '/'
        if used + width > budget && !kept.is_empty() {
            break;
        }
        used += width;
        kept.push(segment);
    }
    kept.reverse();
    let elided = kept.len() < segments.len();
    let kept_start = segments.len() - kept.len();

    let mut spans = vec![Span::styled(" [2] ", Style::default().fg(p.text)), Span::styled("s3://", Style::default().fg(p.muted))];
    let mut col = " [2] s3://".chars().count() as u16;
    let bucket_start = col;
    spans.push(Span::styled(bucket.to_string(), Style::default().fg(p.accent).add_modifier(Modifier::BOLD)));
    col += bucket.chars().count() as u16;
    let mut hits = vec![(bucket_start, col, String::new())];

    if elided {
        spans.push(Span::styled("/…", Style::default().fg(p.muted)));
        col += 2;
    }
    for (i, segment) in kept.iter().enumerate() {
        spans.push(Span::styled("/", Style::default().fg(p.muted)));
        col += 1;
        let start = col;
        spans.push(Span::styled(segment.to_string(), Style::default().fg(p.dir)));
        col += segment.chars().count() as u16;
        let jump_prefix = format!("{}/", segments[..kept_start + i + 1].join("/"));
        hits.push((start, col, jump_prefix));
    }
    if !prefix.is_empty() {
        spans.push(Span::styled("/", Style::default().fg(p.muted)));
    }
    spans.push(Span::styled(format!("{suffix} "), Style::default().fg(p.muted)));
    (Line::from(spans), hits)
}

/// Shortens `s` to at most `max_chars`, keeping the head and tail and eliding the middle. This
/// preserves both ends of a path (drive/root and the current leaf) rather than just cutting off
/// the tail, which would hide exactly the part someone's looking at (the deepest directory).
fn elide_middle(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let keep = max_chars - 1; // reserve one slot for the ellipsis itself
    let head = keep - keep / 2;
    let tail = keep / 2;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Fits `fixed_prefix` + `variable` + `fixed_suffix` into `max_width` columns by eliding the
/// middle of `variable` only. Used for pane titles built around a user-controlled path.
fn elide_variable_segment(fixed_prefix: &str, variable: &str, fixed_suffix: &str, max_width: u16) -> String {
    let fixed_len = fixed_prefix.chars().count() + fixed_suffix.chars().count();
    let budget = (max_width as usize).saturating_sub(fixed_len);
    format!("{fixed_prefix}{}{fixed_suffix}", elide_middle(variable, budget.max(1)))
}

/// Appends a greyed-out `+ name` ghost row per staged clipboard item, previewing what pasting
/// into the focused pane would add. Shown regardless of which pane the items came from.
fn ghost_rows(app: &App, focused: bool, p: &Palette) -> Vec<ListItem<'static>> {
    if !focused {
        return Vec::new();
    }
    let Some(clip) = &app.clip else { return Vec::new() };
    let glyph = match clip.mode {
        ClipMode::Copy => theme::glyph(theme::Glyph::ClipboardCopy, app.icons),
        ClipMode::Move => theme::glyph(theme::Glyph::ClipboardMove, app.icons),
    };
    clip.ghost_names()
        .into_iter()
        .map(|name| {
            ListItem::new(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(format!("+{glyph} "), Style::default().fg(p.muted)),
                Span::styled(name, Style::default().fg(p.muted).add_modifier(Modifier::ITALIC)),
            ]))
        })
        .collect()
}

/// Rows for files still landing in this directory, appended below the listing. A download's
/// destination doesn't join the listing until the refresh that follows `Done`, so without these
/// the pane you're watching the file arrive in is the one place it doesn't appear. Each fills
/// like a real row, so the progress reads where the bytes are actually going.
///
/// Unlike ghost rows these show whether or not the pane is focused: a transfer is happening
/// regardless of where you're looking, and it's the pane the file belongs to.
fn incoming_rows(app: &App, row_width: u16, p: &Palette) -> Vec<ListItem<'static>> {
    app.incoming_local_rows()
        .into_iter()
        .map(|(name, progress)| {
            let style = Style::default().fg(p.text);
            let glyph = match progress {
                Some(_) => theme::glyph(theme::Glyph::JobDownload, app.icons).to_string(),
                None => theme::spinner(app.spinner_frame).to_string(),
            };
            let mut spans = vec![
                Span::styled("  ", Style::default()),
                Span::styled(format!("{glyph} "), Style::default().fg(p.accent)),
                Span::styled(name, style),
            ];
            if let Some(ratio) = progress {
                let (filled, track) = progress_fill_colors(false, p);
                spans = progress_fill(spans, row_width, ratio, filled, track);
            }
            ListItem::new(Line::from(spans))
        })
        .collect()
}

/// Draws a vertical scrollbar down the right border of a pane, but only when the content
/// actually overflows the viewport. `area` is the pane's full rect (borders included); the bar
/// is inset by one row top and bottom so it sits between the corner glyphs.
/// What a listing row is worth showing in the minimap gutter, most important last: a cell that
/// stands for several rows takes the highest of them, so one marked row in a run of forty is never
/// swallowed by its neighbours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
enum RowMark {
    #[default]
    Plain,
    Marked,
    Staged,
    Transferring,
}

/// A row's gutter mark, from the state the row's styling already worked out. Ranked, not combined:
/// a row that is both staged and in flight is shown as in flight, the more urgent of the two.
fn row_gutter_mark(marked: bool, clip_mode: Option<ClipMode>, transferring: bool) -> RowMark {
    if transferring {
        RowMark::Transferring
    } else if clip_mode.is_some() {
        RowMark::Staged
    } else if marked {
        RowMark::Marked
    } else {
        RowMark::Plain
    }
}

/// How many times the viewport a listing must exceed before the scrollbar is replaced by a
/// minimap. Below this you can see most of the list by scrolling once, and a map of it would be
/// noise next to the thing it maps.
const MINIMAP_OVERFLOW_FACTOR: usize = 3;

/// The scroll indicator for a browser pane: a plain scrollbar, or — once the listing runs several
/// screens long — a minimap of the whole thing.
///
/// The minimap answers what a scrollbar can't in a prefix with thousands of objects: not "where am
/// I" but "where is what I care about". Marked rows, staged copies and cuts, and anything in
/// flight show as pips at their position in the *whole* listing, so a scattered selection is
/// visible as a shape before you go looking for it. The current viewport is a lit band behind
/// them, which is the scrollbar's job done in the same column.
fn render_pane_gutter(f: &mut Frame, area: Rect, offset: usize, marks: &[RowMark], p: &Palette) {
    let viewport = area.height.saturating_sub(2) as usize; // minus the top and bottom borders
    if marks.len() <= viewport || viewport == 0 {
        return;
    }
    if marks.len() < viewport * MINIMAP_OVERFLOW_FACTOR {
        render_pane_scrollbar(f, area, offset, marks.len(), p);
        return;
    }

    let x = area.right().saturating_sub(1);
    let thumb_bg = fade_toward(p.panel_bg, p.accent, 0.75);
    for row in 0..viewport {
        // Each cell stands for the slice of the listing that maps onto it.
        let start = row * marks.len() / viewport;
        let end = ((row + 1) * marks.len() / viewport).max(start + 1).min(marks.len());
        let mark = marks[start..end].iter().copied().max().unwrap_or_default();

        let (glyph, fg) = match mark {
            RowMark::Transferring => ("█", p.accent),
            RowMark::Staged => ("█", p.dir),
            RowMark::Marked => ("█", p.good),
            RowMark::Plain => ("│", p.accent_dim),
        };
        // The viewport band, drawn as a background so it reads underneath the pips rather than
        // hiding whichever ones happen to fall inside it.
        let in_view = end > offset && start < offset + viewport;
        let mut style = Style::default().fg(fg);
        if in_view {
            style = style.bg(thumb_bg);
        } else if mark == RowMark::Plain {
            style = style.fg(p.muted);
        }
        let cell = Rect { x, y: area.y + 1 + row as u16, width: 1, height: 1 };
        f.render_widget(Paragraph::new(Line::from(Span::styled(glyph, style))), cell);
    }
}

fn render_pane_scrollbar(f: &mut Frame, area: Rect, offset: usize, total: usize, p: &Palette) {
    let viewport = area.height.saturating_sub(2) as usize; // minus the top and bottom borders
    if total <= viewport || viewport == 0 {
        return;
    }
    let mut state = ScrollbarState::new(total).position(offset).viewport_content_length(viewport);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_style(Style::default().fg(p.accent_dim))
        .track_style(Style::default().fg(p.panel_bg));
    f.render_stateful_widget(bar, area.inner(Margin { vertical: 1, horizontal: 0 }), &mut state);
}

fn draw_local_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Local;
    app.local_pane_area = area;
    app.list_viewport = area.height.saturating_sub(2); // rows visible, for page-jump keys
    let deep_extra_len = app.deep_local.as_ref().map(|d| d.extra.len()).unwrap_or(0);
    let truncated_scan = app.deep_local.as_ref().is_some_and(|d| d.truncated_scan);
    let title = active_filter_title(app, Focus::Local).unwrap_or_else(|| {
        let filter_badge = match &app.local_filter {
            Some(filt) if !filt.is_empty() => {
                let deep_note = if deep_extra_len > 0 {
                    format!(", +{deep_extra_len} elsewhere{}", if truncated_scan { " (scan capped)" } else { "" })
                } else {
                    String::new()
                };
                format!(", filter: {filt}{deep_note}")
            }
            _ => String::new(),
        };
        let sort_pad = if matches!(app.icons, crate::config::IconSet::Nerdfont) { "  " } else { " " };
        let sort_badge = app
            .local_sort
            .label()
            .map(|l| format!(" {}{sort_pad}{l}", theme::glyph(theme::Glyph::Sort, app.icons)))
            .unwrap_or_default();
        let visual_badge = if focused && app.visual_anchor.is_some() { " -- VISUAL --" } else { "" };
        let suffix = format!("{filter_badge}{sort_badge}{visual_badge} ");
        elide_variable_segment(" [1] local: ", &app.local_cwd.display().to_string(), &suffix, area.width)
    });

    let row_width = area.width.saturating_sub(2); // minus the left and right borders
    let visible = app.visible_local_entries();
    let visible_len = visible.len();
    let total_len = visible_len + deep_extra_len;
    let (mut items, mut marks): (Vec<ListItem>, Vec<RowMark>) = if total_len == 0 {
        (vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))], vec![RowMark::Plain])
    } else {
        visible
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let selected = focused && i == app.local_cursor;
                let marked = app.local_marked.contains(&entry.path);
                let clip_mode = app.clip_mode_for_local(&entry.path);
                let icon = theme::icon_for(&entry.name, entry.is_dir, app.icons);
                let size = if entry.is_dir { String::new() } else { format_size(entry.size, BINARY) };
                let name_color = match clip_mode {
                    Some(ClipMode::Copy) => p.good,
                    Some(ClipMode::Move) => p.bad,
                    None if entry.is_dir => p.dir,
                    None => p.text,
                };
                let name_color = match recent_local_write_ratio(app, &entry.path) {
                    Some(ratio) if clip_mode.is_none() => fade_toward(name_color, p.good, ratio),
                    _ => name_color,
                };
                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };
                let mark = row_mark(marked, clip_mode, app.icons);
                let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
                let match_style = if selected {
                    base_style.add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
                };
                let modified = entry
                    .modified
                    .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();

                let transfer = app.local_row_transfer(&entry.path);
                let icon_style = match icon.color {
                    Some(c) if !selected && clip_mode.is_none() => base_style.fg(c),
                    _ => base_style,
                };
                // A row being read (an upload's source) swaps its file-type icon for the same
                // spinner the jobs pane uses, so it reads as busy without implying it's filling.
                let (glyph, icon_style) = match transfer {
                    Some(RowTransfer::Active) => (
                        theme::spinner(app.spinner_frame).to_string(),
                        if selected { base_style } else { Style::default().fg(p.accent) },
                    ),
                    _ => (icon.glyph.to_string(), icon_style),
                };
                let mut spans = vec![Span::styled(mark, mark_style), Span::styled(format!("{glyph} "), icon_style)];
                spans.extend(highlight_spans(&entry.name, app.local_filter.as_deref(), 28, base_style, match_style));
                spans.push(Span::styled(format!("{size:>10}  "), if selected { base_style } else { Style::default().fg(p.muted) }));
                spans.push(Span::styled(modified, if selected { base_style } else { Style::default().fg(p.muted) }));

                // A row being written fills in place, so a download's progress shows on the file
                // it's landing as rather than only on the matching entry in the jobs pane.
                if let Some(RowTransfer::Progress(ratio)) = transfer {
                    let (filled, track) = progress_fill_colors(selected, p);
                    spans = progress_fill(spans, row_width, ratio, filled, track);
                }
                (ListItem::new(Line::from(spans)), row_gutter_mark(marked, clip_mode, transfer.is_some()))
            })
            .unzip()
    };

    // Deep matches from `/`, not direct children, appended below with their relative path shown.
    if let Some(deep) = &app.deep_local {
        for (i, entry) in deep.extra.iter().enumerate() {
            let idx = visible_len + i;
            let selected = focused && idx == app.local_cursor;
            let rel = entry.path.strip_prefix(&app.local_cwd).unwrap_or(&entry.path).display().to_string();
            let marked = app.local_marked.contains(&entry.path);
            let clip_mode = app.clip_mode_for_local(&entry.path);
            let name_color = match clip_mode {
                Some(ClipMode::Copy) => p.good,
                Some(ClipMode::Move) => p.bad,
                _ => p.dir,
            };
            let base_style = if selected {
                Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(name_color)
            };
            let mark = row_mark(marked, clip_mode, app.icons);
            let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
            let match_style = if selected {
                base_style.add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
            };
            let mut spans = vec![Span::styled(mark, mark_style), Span::styled("↳ ", base_style)];
            spans.extend(highlight_spans(&rel, app.local_filter.as_deref(), 0, base_style, match_style));
            items.push(ListItem::new(Line::from(spans)));
            marks.push(row_gutter_mark(marked, clip_mode, app.local_row_transfer(&entry.path).is_some()));
        }
    }
    let incoming = incoming_rows(app, row_width, p);
    let incoming_len = incoming.len();
    items.extend(incoming);
    items.extend(ghost_rows(app, focused, p));
    // Incoming rows are live transfers, so they earn a gutter mark; ghosts get filled in below.
    marks.resize(marks.len() + incoming_len, RowMark::Transferring);
    // Ghosts preview where a paste will land; they aren't listing rows, but the gutter maps row
    // positions, so they need placeholders to keep the two in step.
    marks.resize(items.len(), RowMark::Plain);

    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.local_list_state.select(if total_len == 0 { None } else { Some(app.local_cursor) });
    f.render_stateful_widget(list, area, &mut app.local_list_state);
    render_pane_gutter(f, area, app.local_list_state.offset(), &marks, p);
}

fn draw_remote_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Remote;
    app.remote_pane_area = area;
    app.list_viewport = area.height.saturating_sub(2); // rows visible, for page-jump keys
    let deep_extra_len = app.deep_remote.as_ref().map(|d| d.extra.len()).unwrap_or(0);
    let deep_loading = app.deep_remote.as_ref().is_some_and(|d| d.loading);
    let truncated_scan = app.deep_remote.as_ref().is_some_and(|d| d.truncated_scan);
    let (title, breadcrumb_hits) =
        active_filter_title(app, Focus::Remote).map(|s| (Line::from(s), Vec::new())).unwrap_or_else(|| {
        let filter_badge = match &app.filter {
            Some(filt) if !filt.is_empty() => {
                let deep_note = if deep_loading {
                    format!(" {} scanning...", theme::spinner(app.spinner_frame))
                } else if deep_extra_len > 0 {
                    format!(", +{deep_extra_len} elsewhere{}", if truncated_scan { " (scan capped)" } else { "" })
                } else {
                    String::new()
                };
                format!(", filter: {filt}{deep_note}")
            }
            _ => String::new(),
        };
        let sort_pad = if matches!(app.icons, crate::config::IconSet::Nerdfont) { "  " } else { " " };
        let sort_badge = app
            .remote_sort
            .label()
            .map(|l| format!(" {}{sort_pad}{l}", theme::glyph(theme::Glyph::Sort, app.icons)))
            .unwrap_or_default();
        let visual_badge = if focused && app.visual_anchor.is_some() { " -- VISUAL --" } else { "" };
        remote_breadcrumb_title(&app.bucket, &app.prefix, &format!("{filter_badge}{sort_badge}{visual_badge}"), area.width, p)
    });
    // Title row sits right after the left border, at the pane's top edge.
    app.breadcrumb_hits =
        breadcrumb_hits.into_iter().map(|(start, end, prefix)| (area.x + 1 + start, area.x + 1 + end, prefix)).collect();

    let row_width = area.width.saturating_sub(2); // minus the left and right borders
    let visible = app.visible_entries();
    let visible_len = visible.len();
    let total_len = visible_len + deep_extra_len;
    let (mut items, mut marks): (Vec<ListItem>, Vec<RowMark>) = if total_len == 0 {
        (vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))], vec![RowMark::Plain])
    } else {
        visible
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let selected = focused && i == app.cursor;
                let marked = app.marked.contains(&entry.key);
                let clip_mode = app.clip_mode_for_remote(&entry.key);
                let icon = theme::icon_for(&entry.name, entry.is_dir, app.icons);
                let size = if entry.is_dir { String::new() } else { format_size(entry.size.max(0) as u64, BINARY) };
                let name_color = match clip_mode {
                    Some(ClipMode::Copy) => p.good,
                    Some(ClipMode::Move) => p.bad,
                    None if entry.is_dir => p.dir,
                    None => p.text,
                };
                let name_color = match recent_remote_write_ratio(app, &app.bucket, &entry.key) {
                    Some(ratio) if clip_mode.is_none() => fade_toward(name_color, p.good, ratio),
                    _ => name_color,
                };

                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };

                let mark = row_mark(marked, clip_mode, app.icons);
                let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
                let match_style = if selected {
                    base_style.add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
                };

                let modified = entry.last_modified.as_deref().unwrap_or("");

                let transfer = app.remote_row_transfer(&entry.key);
                let icon_style = match icon.color {
                    Some(c) if !selected && clip_mode.is_none() => base_style.fg(c),
                    _ => base_style,
                };
                // See the local pane: the object being read spins rather than filling, so a
                // download shows its source here and its progress over in the local pane.
                let (glyph, icon_style) = match transfer {
                    Some(RowTransfer::Active) => (
                        theme::spinner(app.spinner_frame).to_string(),
                        if selected { base_style } else { Style::default().fg(p.accent) },
                    ),
                    _ => (icon.glyph.to_string(), icon_style),
                };
                let mut spans = vec![Span::styled(mark, mark_style), Span::styled(format!("{glyph} "), icon_style)];
                spans.extend(highlight_spans(&entry.name, app.filter.as_deref(), 28, base_style, match_style));
                spans.push(Span::styled(format!("{size:>10}  "), if selected { base_style } else { Style::default().fg(p.muted) }));
                spans.push(Span::styled(modified.to_string(), if selected { base_style } else { Style::default().fg(p.muted) }));

                if let Some(RowTransfer::Progress(ratio)) = transfer {
                    let (filled, track) = progress_fill_colors(selected, p);
                    spans = progress_fill(spans, row_width, ratio, filled, track);
                }
                (ListItem::new(Line::from(spans)), row_gutter_mark(marked, clip_mode, transfer.is_some()))
            })
            .unzip()
    };

    // Deep matches from `/`, not direct children, appended below with their relative key shown.
    if let Some(deep) = &app.deep_remote {
        for (i, entry) in deep.extra.iter().enumerate() {
            let idx = visible_len + i;
            let selected = focused && idx == app.cursor;
            let rel = entry.key.strip_prefix(&app.prefix).unwrap_or(&entry.key);
            let marked = app.marked.contains(&entry.key);
            let clip_mode = app.clip_mode_for_remote(&entry.key);
            let name_color = match clip_mode {
                Some(ClipMode::Copy) => p.good,
                Some(ClipMode::Move) => p.bad,
                _ => p.dir,
            };
            let base_style = if selected {
                Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(name_color)
            };
            let mark = row_mark(marked, clip_mode, app.icons);
            let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
            let match_style = if selected {
                base_style.add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
            };
            let mut spans = vec![Span::styled(mark, mark_style), Span::styled("↳ ", base_style)];
            spans.extend(highlight_spans(rel, app.filter.as_deref(), 0, base_style, match_style));
            items.push(ListItem::new(Line::from(spans)));
            marks.push(row_gutter_mark(marked, clip_mode, app.remote_row_transfer(&entry.key).is_some()));
        }
    }
    items.extend(ghost_rows(app, focused, p));
    // Ghosts preview where a paste will land; they aren't listing rows, but the gutter maps row
    // positions, so they need placeholders to keep the two in step.
    marks.resize(items.len(), RowMark::Plain);

    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.list_state.select(if total_len == 0 { None } else { Some(app.cursor) });
    f.render_stateful_widget(list, area, &mut app.list_state);
    render_pane_gutter(f, area, app.list_state.offset(), &marks, p);
}

/// Builds pane 3's title: `[3]` plus both tab labels with the active one highlighted, plus an
/// optional trailing detail, both tabs always shown so `p`/`i` read as discoverable selectors.
fn preview_tabs_title(app: &App, p: &Palette, detail: &str) -> Line<'static> {
    let tab_style = |active: bool| {
        if active {
            Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.muted)
        }
    };
    let mut spans = vec![
        Span::raw(" [3] "),
        Span::styled(" Preview ", tab_style(app.preview_mode == PreviewMode::Content)),
        Span::raw(" "),
        Span::styled(" Info ", tab_style(app.preview_mode == PreviewMode::Info)),
        Span::raw(" "),
    ];
    if !detail.is_empty() {
        spans.push(Span::raw(format!("{detail} ")));
    }
    Line::from(spans)
}

fn draw_preview_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Preview;
    app.preview_pane_area = area;

    // Images need a stateful `ratatui_image` widget (mutable access to encoded protocol
    // state); everything else falls through to the plain `Paragraph` path below.
    if let Preview::Image { size, .. } = &app.preview {
        let title = preview_tabs_title(app, p, &format_size(*size, BINARY));
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if let Preview::Image { state, .. } = &mut app.preview {
            f.render_stateful_widget(ratatui_image::StatefulImage::default(), inner, state.as_mut());
        }
        return;
    }

    // Info tab: the region globe on top, the item's metadata below. Rendering it here (not only on
    // the bucket picker, which is skipped when list-buckets isn't permitted) makes the globe
    // reliably reachable from the browser with `i`.
    if let Preview::Info(info) = &app.preview {
        let title = preview_tabs_title(app, p, "");
        let region = app.client.as_ref().map(|c| c.region()).unwrap_or("");
        draw_globe_info(f, area, region, info_body(info, area.width, p), title, pane_border_style(focused, p), p, app.info_scroll);
        return;
    }

    // Shown on every "can't preview content" state so `i` is discoverable; not on `Empty` or `Text`.
    // Pinned to its own fixed bottom row below (not appended into `body`'s text). Appended text
    // could get pushed off the bottom of a short pane or a long error message with no way to
    // scroll to it, the same class of bug the confirm dialog's buttons had.
    const INFO_HINT: &str = "press i for info";
    let show_hint = matches!(&app.preview, Preview::Directory | Preview::TooLarge { .. } | Preview::Binary { .. } | Preview::Error(_));

    let is_content = matches!(&app.preview, Preview::Text { .. });
    let (detail, body, color): (String, Text, Color) = match &app.preview {
        Preview::Empty => (String::new(), Text::raw("(nothing selected)"), p.muted),
        Preview::Loading => {
            (String::new(), Text::raw(format!("{} loading...", theme::spinner(app.spinner_frame))), p.muted)
        }
        Preview::Directory => (String::new(), Text::raw("(directory)"), p.muted),
        Preview::TooLarge { size, map } => (
            String::new(),
            structure_body(&format!("file too large to preview ({})", format_size(*size, BINARY)), map.as_ref(), area.width, p),
            p.muted,
        ),
        Preview::Binary { size, map } => (
            String::new(),
            structure_body(&format!("binary file, {}", format_size(*size, BINARY)), map.as_ref(), area.width, p),
            p.muted,
        ),
        Preview::Error(err) => (String::new(), Text::raw(format!("preview error: {err}")), p.bad),
        // Handled by the early return above.
        Preview::Image { .. } => unreachable!(),
        Preview::Info(info) => (String::new(), info_body(info, area.width, p), p.text),
        Preview::Text { text, size, truncated, highlight } => {
            let detail = if *truncated {
                format!("{}, showing first bytes", format_size(*size, BINARY))
            } else {
                format_size(*size, BINARY)
            };
            let body = match highlight {
                Some(lines) => Text::from(
                    lines
                        .iter()
                        .map(|spans| {
                            Line::from(
                                spans
                                    .iter()
                                    .map(|s| {
                                        let (r, g, b) = s.rgb;
                                        Span::styled(s.text.clone(), Style::default().fg(Color::Rgb(r, g, b)))
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>(),
                ),
                None => Text::raw(text.clone()),
            };
            (detail, body, p.text)
        }
    };

    let title = preview_tabs_title(app, p, &detail);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(pane_border_style(focused, p))
        .style(Style::default().bg(p.panel_bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let (body_area, hint_area) = if show_hint && inner.height >= 2 {
        let parts =
            Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(0), Constraint::Length(1)]).split(inner);
        (parts[0], Some(parts[1]))
    } else {
        (inner, None)
    };

    // Line count before `body` is moved, so a text preview can show how far down it is. Wrapping
    // can push the true count a little higher, but it's close enough to read as a position hint.
    let content_lines = body.lines.len();
    let mut widget = Paragraph::new(center_lines(body, body_area.height, !is_content))
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0))
        .style(Style::default().fg(color));
    if !is_content {
        widget = widget.alignment(Alignment::Center);
    }
    f.render_widget(widget, body_area);
    if let Some(hint_area) = hint_area {
        f.render_widget(
            Paragraph::new(Line::from(INFO_HINT).alignment(Alignment::Center)).style(Style::default().fg(p.muted)),
            hint_area,
        );
    }
    if is_content {
        render_pane_scrollbar(f, area, app.preview_scroll as usize, content_lines, p);
    }
}

/// Bar glyphs for the entropy chart, lowest to highest.
const ENTROPY_BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The widest the entropy chart is drawn, regardless of pane width: past this it stops reading as
/// a shape and starts reading as noise, and the map only has [`SAMPLE_WINDOWS`] real windows
/// behind it anyway.
const ENTROPY_CHART_MAX_COLS: usize = 56;

/// One column of the entropy chart: bar height and colour both track entropy, so the shape reads
/// at a glance and the colour survives a terminal that renders the block glyphs at odd heights.
fn entropy_span(bits: f32, p: &Palette) -> Span<'static> {
    let level = ((bits / 8.0 * ENTROPY_BARS.len() as f32) as usize).min(ENTROPY_BARS.len() - 1);
    let color = match bits {
        b if b < 2.0 => p.muted,
        b if b < 6.5 => p.accent_dim,
        _ => p.accent,
    };
    Span::styled(ENTROPY_BARS[level], Style::default().fg(color))
}

/// The body shown for a file the pane can't render: why not, and then — once the sampler has been
/// across it — what its bytes look like.
///
/// The chart is a per-window entropy profile (see [`crate::app::structure`]), which is the most
/// this can honestly say about a file it never read as content: where the header stops, whether
/// the body is compressed, whether the tail is padding. The caption says outright that it's a
/// substitute for a preview and how it was sampled, so it can't be mistaken for the file itself.
fn structure_body(headline: &str, map: Option<&crate::app::StructureMap>, pane_width: u16, p: &Palette) -> Text<'static> {
    let muted = Style::default().fg(p.muted);
    let mut lines = vec![Line::from(Span::styled(headline.to_string(), muted))];

    let Some(map) = map else {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("sampling byte structure...", muted)));
        return Text::from(lines);
    };

    // Borders, plus a column of breathing room on each side.
    let cols = (pane_width.saturating_sub(4) as usize).min(ENTROPY_CHART_MAX_COLS);
    lines.push(Line::from(Span::styled("showing its byte structure instead:", muted)));
    lines.push(Line::raw(""));
    lines.push(Line::from(map.resampled(cols).iter().map(|bits| entropy_span(*bits, p)).collect::<Vec<_>>()));
    lines.push(Line::from(Span::styled(format!("byte entropy, {}", map.coverage()), muted)));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} · mean {:.1} of 8 bits/byte", map.verdict(), map.mean()),
        Style::default().fg(p.text),
    )));
    lines.push(Line::from(vec![
        Span::styled("▁ empty  ", Style::default().fg(p.muted)),
        Span::styled("▄ structured  ", Style::default().fg(p.accent_dim)),
        Span::styled("█ dense", Style::default().fg(p.accent)),
    ]));
    Text::from(lines)
}

fn draw_jobs_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Jobs;
    app.jobs_pane_area = area;
    let items: Vec<ListItem> = if app.jobs.is_empty() {
        vec![ListItem::new(Span::styled("  no jobs yet", Style::default().fg(p.muted)))]
    } else {
        app.jobs
            .iter()
            .rev()
            .enumerate()
            .map(|(i, job)| {
                let selected = focused && i == app.jobs_cursor;
                let kind_icon = match job.kind {
                    JobKind::Download => theme::glyph(theme::Glyph::JobDownload, app.icons),
                    JobKind::Upload => theme::glyph(theme::Glyph::JobUpload, app.icons),
                    // Bundled download, same glyph as an archive: "several files, not one".
                    JobKind::Zip => theme::glyph(theme::Glyph::JobBundle, app.icons),
                    // Same glyphs as the staged-clipboard copy/cut rows.
                    JobKind::LocalCopy | JobKind::RemoteCopy => theme::glyph(theme::Glyph::ClipboardCopy, app.icons),
                    JobKind::LocalMove | JobKind::RemoteMove => theme::glyph(theme::Glyph::ClipboardMove, app.icons),
                    JobKind::RemoteDelete => theme::glyph(theme::Glyph::JobDelete, app.icons),
                };
                let is_copy_or_move =
                    matches!(job.kind, JobKind::LocalCopy | JobKind::LocalMove | JobKind::RemoteCopy | JobKind::RemoteMove);
                // Same-store: never left the local filesystem or bucket, vs. a real network transfer.
                let is_same_store = is_copy_or_move || job.kind == JobKind::RemoteDelete;
                let (status_icon, color) = match &job.status {
                    JobStatus::Running => (theme::spinner(app.spinner_frame), p.accent),
                    // "arrived" reads better than a plain checkmark for a same-store copy/move.
                    JobStatus::Done if is_copy_or_move => (theme::glyph(theme::Glyph::StatusArrived, app.icons), p.good),
                    JobStatus::Done => (theme::glyph(theme::Glyph::StatusDone, app.icons), p.good),
                    JobStatus::Cancelled => (theme::glyph(theme::Glyph::StatusCancelled, app.icons), p.muted),
                    JobStatus::Failed(_) => (theme::glyph(theme::Glyph::StatusFailed, app.icons), p.bad),
                };
                // Skip the (always full) bar once done, it'd paint over the selection highlight.
                let detail = match &job.status {
                    JobStatus::Failed(err) => err.clone(),
                    JobStatus::Cancelled => "cancelled".to_string(),
                    JobStatus::Done => format_size(job.done_bytes, BINARY),
                    _ if job.total_bytes > 0 => {
                        let pct = (job.progress_ratio() * 100.0) as u32;
                        let bar_width = 16usize;
                        let filled = ((job.progress_ratio() * bar_width as f64).round() as usize).min(bar_width);
                        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_width - filled));
                        format!("{bar} {pct:>3}%")
                    }
                    _ => format_size(job.done_bytes, BINARY),
                };

                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p.text)
                };
                let marker = if selected { "➜ " } else { "  " };
                // Keep detail text in its status color even when selected, bg tint is highlight enough.
                let detail_style =
                    if selected { Style::default().fg(color).bg(p.accent) } else { Style::default().fg(color) };
                let status_style =
                    if selected { Style::default().fg(color).bg(p.accent) } else { Style::default().fg(color) };
                // Color the kind icon by category (network vs. same-store) rather than status.
                let kind_color = if is_same_store { p.dir } else { p.accent_dim };
                let kind_style = if selected { Style::default().fg(kind_color).bg(p.accent) } else { Style::default().fg(kind_color) };

                // A running transfer's bar is drawn as a two-tone gauge (filled vs. track), same
                // █/░ glyphs as before but coloured. On a selected row we fall back to the plain
                // one-colour `detail` string so the fill doesn't fight the highlight background.
                let detail_spans: Vec<Span> = match &job.status {
                    JobStatus::Running if job.total_bytes > 0 && !selected => {
                        let pct = (job.progress_ratio() * 100.0) as u32;
                        let bar_width = 16usize;
                        let filled = ((job.progress_ratio() * bar_width as f64).round() as usize).min(bar_width);
                        let fill_color = if is_same_store { p.dir } else { p.good };
                        vec![
                            Span::styled(" ", detail_style),
                            Span::styled("█".repeat(filled), Style::default().fg(fill_color)),
                            Span::styled("░".repeat(bar_width - filled), Style::default().fg(p.muted)),
                            Span::styled(format!(" {pct:>3}%"), Style::default().fg(color)),
                        ]
                    }
                    _ => vec![Span::styled(format!(" {detail}"), detail_style)],
                };

                // Nerd Font action glyphs (copy/cut/trash/arrows) render wider than their reported
                // width, so they overpaint a single trailing space and collide with the next icon
                // or the label. Give each a two-column slot in Nerd Font mode to keep them apart.
                let pad = if matches!(app.icons, crate::config::IconSet::Nerdfont) { "  " } else { " " };
                let mut line_spans = vec![
                    Span::styled(marker, base_style),
                    Span::styled(format!("{status_icon}{pad}"), status_style),
                    Span::styled(format!("{kind_icon}{pad}"), kind_style),
                    Span::styled(format!("{:<24}", job.label), base_style),
                ];
                line_spans.extend(detail_spans);
                ListItem::new(Line::from(line_spans))
            })
            .collect()
    };

    // The icon-legend and keybind hints that used to live here moved to the `?` help dialog's
    // "jobs pane" section. This title alone was getting busy, and the focused border color
    // already signals focus without repeating it in text; the icon meanings and `enter`/`f`
    // shortcuts are exactly the kind of thing "the full keymap lives behind `?`" already covers
    // for the rest of the app.
    let title = Line::from(format!(" [4] jobs ({}) ", app.jobs.len()));
    let row_count = items.len();
    let outer = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(pane_border_style(focused, p))
        .style(Style::default().bg(p.panel_bg));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // While anything is transferring, reserve the top row of the pane for a live throughput
    // sparkline; the job list flows underneath. Once idle the trace clears and the list reclaims
    // the row.
    let list_area = if !app.throughput_history.is_empty() && inner.height >= 3 {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);
        draw_throughput_sparkline(f, app, parts[0], p);
        parts[1]
    } else {
        inner
    };

    let list = List::new(items);
    app.jobs_list_state.select(if app.jobs.is_empty() { None } else { Some(app.jobs_cursor) });
    f.render_stateful_widget(list, list_area, &mut app.jobs_list_state);
    render_pane_scrollbar(f, area, app.jobs_list_state.offset(), row_count, p);
}

/// A live transfer-rate sparkline with the current speed pinned to the right. Filled only while
/// `throughput_history` is non-empty (i.e. transfers are active or just finished decaying).
///
/// Aggregate rate over time, which is the one thing neither the job list next to it nor a row's
/// own fill can show, so it belongs with the queue and appears nowhere else.
fn draw_throughput_sparkline(f: &mut Frame, app: &App, area: Rect, p: &Palette) {
    let history = &app.throughput_history;
    let label = format!(" {}/s ", format_size(app.current_throughput(), BINARY));
    let label_w = label.chars().count() as u16;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(label_w)])
        .split(area);
    let spark = Sparkline::default().data(history.iter()).style(Style::default().fg(p.accent));
    f.render_widget(spark, cols[0]);
    let lbl = Paragraph::new(Line::from(Span::styled(
        label,
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
    )));
    f.render_widget(lbl, cols[1]);
}

/// Sync dialog: a direction banner (which pill is lit shows source vs. destination, the arrow
/// shows which way), a colour-coded stat row that doubles as the add/update/same/skip legend
/// (real counts, not just static text), a divider rule, then the two-panel diff with scrollbars
/// and role-coloured borders (bright = source, dim = destination, matching the banner pills).
/// Keybinds live only in the border's `title_bottom`, structurally separated from the legend
/// (which used to share both the same muted colour and the same line, hard to tell apart).
fn draw_sync(f: &mut Frame, app: &mut App, p: &Palette) {
    let direction = app.sync.as_ref().map(|s| s.direction);
    let dest_is_local = direction == Some(SyncDirection::RemoteToLocal);
    let actionable = app.sync.as_ref().map(|s| s.actionable()).unwrap_or(0);
    let icons = app.icons; // captured before the &mut borrow of app.sync below

    let Some(state) = app.sync.as_mut() else { return };
    let Some(direction) = direction else { return };

    let (mut add_count, mut update_count, mut same_count, mut skip_count) = (0, 0, 0, 0);
    for entry in &state.entries {
        match entry.action {
            SyncAction::Add => add_count += 1,
            SyncAction::Update => update_count += 1,
            SyncAction::Unchanged => same_count += 1,
            SyncAction::ExtraSkipped => skip_count += 1,
        }
    }

    // Size the dialog to its content rather than forcing full height, a short diff gets a short
    // dialog. Panel interior rows = dialog height − HEADER_ROWS − 4 (outer + panel borders).
    const HEADER_ROWS: u16 = 3; // direction banner, stat/legend row, divider rule
    let screen = f.area();
    let content_rows = state.entries.len().max(1);
    let max_rows = (screen.height as usize).saturating_sub(HEADER_ROWS as usize + 8).max(1);
    let rows = content_rows.min(max_rows);
    let dialog_h = (rows as u16 + HEADER_ROWS + 4).min(screen.height);
    let dialog_w = ((screen.width as u32 * 92 / 100) as u16).min(screen.width);
    let area = Rect {
        x: screen.x + screen.width.saturating_sub(dialog_w) / 2,
        y: screen.y + screen.height.saturating_sub(dialog_h) / 2,
        width: dialog_w,
        height: dialog_h,
    };
    f.render_widget(Clear, area);

    let footer = " ↑/↓ move · tab/h/l/d flip direction · enter run · esc close ";
    let outer = Block::default()
        .title(" sync ")
        .title_bottom(footer)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.panel_bg).fg(p.text));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    let (banner_row, stats_row, rule_row, body_row) = (sections[0], sections[1], sections[2], sections[3]);

    // A lit pill marks the source ("data leaves from here"), a plain outlined one the
    // destination. The big arrow between them is the only thing that changes with direction;
    // LOCAL/REMOTE always sit in the same left/right spots the panels below use.
    let source_style = Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD);
    let dest_style = Style::default().fg(p.accent_dim).add_modifier(Modifier::BOLD);
    let (local_style, remote_style, arrow, verb) = match direction {
        SyncDirection::LocalToRemote => (source_style, dest_style, "═══════▶", "upload"),
        SyncDirection::RemoteToLocal => (dest_style, source_style, "◀═══════", "download"),
    };
    let banner = Line::from(vec![
        Span::styled(" LOCAL ", local_style),
        Span::raw("  "),
        Span::styled(arrow, Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(" REMOTE ", remote_style),
        Span::raw("   "),
        Span::styled(format!("{verb} · {actionable} to transfer"), Style::default().fg(p.muted)),
    ])
    .alignment(Alignment::Center);
    f.render_widget(Paragraph::new(banner), banner_row);

    // Live counts double as the +/~/=/- legend: the colour teaches the meaning and the number is
    // actually useful, instead of a separate static "+ = add" text explanation.
    let stat = |count: usize, label: &str, color: Color| -> [Span<'static>; 2] {
        [
            Span::styled(format!("● {count} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{label}   "), Style::default().fg(p.muted)),
        ]
    };
    let stats_spans: Vec<Span> = [
        stat(add_count, "add", p.good),
        stat(update_count, "update", p.dir),
        stat(same_count, "unchanged", p.muted),
        stat(skip_count, "skip (kept, never deleted)", p.bad),
    ]
    .into_iter()
    .flatten()
    .collect();
    f.render_widget(Paragraph::new(Line::from(stats_spans)).alignment(Alignment::Center), stats_row);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled("─".repeat(rule_row.width as usize), Style::default().fg(p.accent_dim)))),
        rule_row,
    );

    // Two bordered panels with a slim colour-coded gutter between them.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(3), Constraint::Min(0)])
        .split(body_row);
    let (left_rect, mid_rect, right_rect) = (cols[0], cols[1], cols[2]);

    // Keep the cursor on screen.
    if state.cursor < state.offset {
        state.offset = state.cursor;
    } else if state.cursor >= state.offset + rows {
        state.offset = state.cursor + 1 - rows;
    }

    let color_for = |action: SyncAction| match action {
        SyncAction::Add => p.good,
        SyncAction::Update => p.dir,
        // Both no-ops (extras are never deleted); the `=`/`-` icon carries the distinction.
        SyncAction::Unchanged | SyncAction::ExtraSkipped => p.muted,
    };
    let icon_for = |action: SyncAction| match action {
        SyncAction::Add => '+',
        SyncAction::Update => '~',
        SyncAction::Unchanged => '=',
        SyncAction::ExtraSkipped => '-',
    };

    let left_w = left_rect.width.saturating_sub(2) as usize;
    let right_w = right_rect.width.saturating_sub(2) as usize;

    // Renders one side's cell. `display` is whether this side shows the file at all, an add
    // also appears on the destination side (in green), projecting the post-sync state.
    let side_line =
        |display: bool, icon: char, name: &str, size: Option<u64>, mtime: Option<i64>, width: usize, color: Color, selected: bool| {
            let text = if display {
                let sz = size.map(|s| format_size(s, BINARY)).unwrap_or_default();
                let ts = fmt_mtime(mtime);
                // A per-type file glyph after the +/~/=/- status char. It carries the row's
                // sync-status colour (part of the same styled span), not its devicon colour, so
                // the add/update/skip signal stays coherent across the row.
                let ficon = theme::icon_for(name, false, icons).glyph;
                let name_w = width.saturating_sub(2 + 2 + 10 + 17).max(4);
                format!("{icon} {ficon} {:<name_w$} {sz:>9} {ts:>16}", truncate(name, name_w))
            } else {
                String::new()
            };
            let text = format!("{text:<width$}"); // pad so a selected row's highlight fills the panel
            let style = if selected {
                Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };
            Line::from(Span::styled(text, style))
        };

    let mut left_lines = Vec::new();
    let mut right_lines = Vec::new();
    let mut mid_lines = vec![Line::raw("")]; // one blank to drop below the panels' top border

    if state.entries.is_empty() {
        let msg = Line::styled("  both sides are already in sync", Style::default().fg(p.muted));
        left_lines.push(msg.clone());
        right_lines.push(msg);
    }
    for (i, entry) in state.entries.iter().enumerate().skip(state.offset).take(rows) {
        let selected = i == state.cursor;
        let color = color_for(entry.action);
        let icon = icon_for(entry.action);
        let is_add = entry.action == SyncAction::Add;

        let local_display = entry.local_size.is_some() || (is_add && dest_is_local);
        let (l_size, l_mtime) = if entry.local_size.is_some() {
            (entry.local_size, entry.local_mtime)
        } else {
            (entry.remote_size, entry.remote_mtime) // projected incoming values
        };
        left_lines.push(side_line(local_display, icon, &entry.rel, l_size, l_mtime, left_w, color, selected));

        let remote_display = entry.remote_size.is_some() || (is_add && !dest_is_local);
        let (r_size, r_mtime) = if entry.remote_size.is_some() {
            (entry.remote_size, entry.remote_mtime)
        } else {
            (entry.local_size, entry.local_mtime)
        };
        right_lines.push(side_line(remote_display, icon, &entry.rel, r_size, r_mtime, right_w, color, selected));

        // Extras never move, so the gutter shows a dot; everything else points which way this
        // row specifically is going, coloured by its own action (not a single flat accent). The
        // banner above already says the *overall* direction once, so this repetition instead
        // carries new information: which rows are actually moving.
        let glyph = match entry.action {
            SyncAction::ExtraSkipped => "·",
            _ if dest_is_local => "◀",
            _ => "▶",
        };
        let style = if selected {
            Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        };
        mid_lines.push(Line::from(Span::styled(format!("{glyph:^3}"), style)));
    }

    // Bright border = source ("data leaves from here"), dim = destination, same pairing as the
    // banner pills above, so the whole dialog reads as one consistent flow rather than the panels
    // and the banner using unrelated colour languages.
    let (left_label, right_label, left_border, right_border) = if dest_is_local {
        (" local (dest) ", " remote (source) ", p.accent_dim, p.accent)
    } else {
        (" local (source) ", " remote (dest) ", p.accent, p.accent_dim)
    };
    let panel = |label: &str, color: Color| {
        Block::default().title(label.to_string()).borders(Borders::ALL).border_style(Style::default().fg(color))
    };
    f.render_widget(Paragraph::new(left_lines).block(panel(left_label, left_border)), left_rect);
    f.render_widget(Paragraph::new(mid_lines), mid_rect);
    f.render_widget(Paragraph::new(right_lines).block(panel(right_label, right_border)), right_rect);
    render_pane_scrollbar(f, left_rect, state.offset, state.entries.len(), p);
    render_pane_scrollbar(f, right_rect, state.offset, state.entries.len(), p);
}

/// Pads `body` with blank lines so it sits vertically centered in a viewport `inner_h` rows tall
/// (horizontal centering comes from the caller's `Alignment::Center`). No-op when `should_center`
/// is false, real file content stays top-aligned.
fn center_lines(body: Text<'_>, inner_h: u16, should_center: bool) -> Text<'_> {
    if !should_center {
        return body;
    }
    let inner_h = inner_h as usize;
    let vpad = inner_h.saturating_sub(body.lines.len()) / 2;
    if vpad == 0 {
        return body;
    }
    let mut lines = vec![Line::raw(""); vpad];
    lines.extend(body.lines);
    Text::from(lines)
}

/// Builds the `i` info view's body: name, location, size, last-modified, plus backend-specific
/// `extra` metadata (S3's ETag/Content-Type/etc), rendered generically for future backends.
/// Band colours for the composition bar, in rank order. Drawn from the palette rather than fixed
/// hues so a custom theme recolours the chart with everything else; `muted` sits last because the
/// final band is usually `other`, which shouldn't compete with the named types above it.
fn composition_colors(p: &Palette) -> [Color; 6] {
    [p.accent, p.good, p.dir, p.accent_dim, p.bad, p.muted]
}

/// The composition breakdown: one stacked bar, then a legend row per file type.
///
/// The bar answers "what is this prefix made of" at a glance — the question a total in bytes can't
/// — and the legend puts numbers on it, since a band's width tells you a share but not a size.
fn composition_lines(composition: &crate::app::Composition, width: u16, p: &Palette) -> Vec<Line<'static>> {
    let colors = composition_colors(p);
    let bar_width = (width.saturating_sub(2) as usize).min(48);
    let columns = composition.columns(bar_width);

    let bar: Vec<Span> = composition
        .slices
        .iter()
        .zip(&columns)
        .filter(|(_, cols)| **cols > 0)
        .enumerate()
        .map(|(i, (_, cols))| Span::styled("█".repeat(*cols), Style::default().fg(colors[i % colors.len()])))
        .collect();

    let mut lines = vec![
        Line::from(Span::styled("Composition: ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD))),
        Line::from(bar),
    ];
    for (i, slice) in composition.slices.iter().enumerate() {
        let color = colors[i % colors.len()];
        lines.push(Line::from(vec![
            Span::styled("█ ", Style::default().fg(color)),
            Span::styled(format!("{:<12}", truncate(&slice.label, 12)), Style::default().fg(p.text)),
            Span::styled(format!("{:>5.1}%  ", composition.share(slice) * 100.0), Style::default().fg(color)),
            Span::styled(format!("{:>9}  ", format_size(slice.bytes, BINARY)), Style::default().fg(p.good)),
            Span::styled(format!("{} files", slice.files), Style::default().fg(p.muted)),
        ]));
    }
    lines
}

fn info_body(info: &crate::app::InfoDetails, width: u16, p: &Palette) -> Text<'static> {
    // Labels share one bold accent colour so they read as a scannable column; each value takes a
    // colour keyed to what it is (paths, sizes, counts, timestamps), so rows are easy to tell apart.
    let label = |s: &str| Span::styled(s.to_string(), Style::default().fg(p.accent).add_modifier(Modifier::BOLD));
    let value = |s: String, c: Color| Span::styled(s, Style::default().fg(c));

    let mut lines = vec![
        Line::from(vec![label("Name: "), Span::styled(info.name.clone(), Style::default().fg(p.text).add_modifier(Modifier::BOLD))]),
        Line::from(vec![label("Key: "), value(info.key.clone(), p.text)]),
    ];
    if let Some(loc) = &info.remote_location {
        lines.push(Line::from(vec![label("Location: "), value(loc.clone(), p.dir)]));
    }
    if let Some(path) = &info.local_path {
        lines.push(Line::from(vec![label("Path: "), value(path.clone(), p.dir)]));
    }
    if info.is_dir {
        lines.push(Line::from(vec![label("Kind: "), value("directory".to_string(), p.muted)]));
    }
    // Directories carry aggregates (total size over the tree, newest mtime, file/object count)
    // rather than object metadata; files carry their own. Same fields either way.
    if let Some(size) = info.size {
        let l = if info.is_dir { "Total size: " } else { "Size: " };
        lines.push(Line::from(vec![label(l), value(format_size(size, BINARY), p.good)]));
    }
    if let Some(modified) = &info.last_modified {
        let l = if info.is_dir { "Newest file: " } else { "Last Modified: " };
        lines.push(Line::from(vec![label(l), value(modified.clone(), p.accent_dim)]));
    }
    for (key, val) in &info.extra {
        let vcolor = if key == "objects" || key == "files" { p.good } else { p.text };
        lines.push(Line::from(vec![label(&format!("{key}: ")), value(val.clone(), vcolor)]));
    }
    if let Some(composition) = &info.composition {
        lines.push(Line::raw(""));
        lines.extend(composition_lines(composition, width, p));
    }
    if !info.is_dir {
        lines.push(Line::raw(""));
        lines.push(Line::styled("press p to preview content", Style::default().fg(p.muted).add_modifier(Modifier::ITALIC)));
    }
    Text::from(lines)
}

/// Formats a Unix-seconds timestamp as local `YYYY-MM-DD HH:MM`, or empty for `None`.
fn fmt_mtime(secs: Option<i64>) -> String {
    match secs {
        Some(s) => chrono::DateTime::from_timestamp(s, 0)
            .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let keep = max.saturating_sub(1);
        format!("{}…", s.chars().take(keep).collect::<String>())
    }
}

fn draw_prompt(f: &mut Frame, app: &App, prompt: &crate::app::Prompt, p: &Palette) {
    let area = centered_rect(60, 15, f.area());
    f.render_widget(Clear, area);
    let title = match prompt.kind {
        PromptKind::Rename => " rename to ".to_string(),
        PromptKind::ConfirmTargetName => " save as ".to_string(),
        PromptKind::UploadPath => " upload local path (drag a file here, or paste one) ".to_string(),
        PromptKind::Filter => " filter ".to_string(),
        PromptKind::ProfileRename => " profile (s3 / s3_private_link, blank to unset) ".to_string(),
        PromptKind::BookmarkField => match &app.bookmark_wizard {
            Some(w) => {
                let (label, _, optional) = BOOKMARK_FIELDS[w.field_index];
                let opt = if optional { ", optional, enter to skip" } else { "" };
                format!(" bookmark field {}/{}: {label}{opt} ", w.field_index + 1, BOOKMARK_FIELDS.len())
            }
            None => " bookmark ".to_string(),
        },
    };
    let (before, after) = prompt.split_at_cursor();
    let text = if prompt.mask {
        format!("{}▏{}", "•".repeat(before.chars().count()), "•".repeat(after.chars().count()))
    } else {
        format!("{before}▏{after}")
    };
    let widget = Paragraph::new(text).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent))
            .style(Style::default().bg(p.panel_bg).fg(p.text)),
    );
    f.render_widget(widget, area);
}

/// Splits `old`/`new` into a shared prefix, a shared suffix, and the differing middle: the
/// classic filename-rename diff shape (`report[_final].txt`). Returns `(prefix_len, suffix_len)`
/// in chars; both are bounded by the shorter string's length so the caller's slicing can't panic.
fn char_diff_bounds(old: &[char], new: &[char]) -> (usize, usize) {
    let max_common = old.len().min(new.len());
    let mut prefix = 0;
    while prefix < max_common && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < max_common - prefix && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix] {
        suffix += 1;
    }
    (prefix, suffix)
}

/// Renders `old` → `new` as a two-line inline diff for the rename confirmation: unchanged parts
/// dim, the removed middle struck through in red, the added middle bold green.
fn rename_diff_lines(old: &str, new: &str, p: &Palette) -> Vec<Line<'static>> {
    let (o, n): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let (prefix, suffix) = char_diff_bounds(&o, &n);
    let dim = Style::default().fg(p.muted);
    let seg = |chars: &[char], from: usize, to: usize| -> String { chars[from..to].iter().collect() };
    vec![
        Line::from(vec![
            Span::styled("- ", Style::default().fg(p.bad)),
            Span::styled(seg(&o, 0, prefix), dim),
            Span::styled(
                seg(&o, prefix, o.len() - suffix),
                Style::default().fg(p.bad).add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT),
            ),
            Span::styled(seg(&o, o.len() - suffix, o.len()), dim),
        ])
        .alignment(Alignment::Center),
        Line::from(vec![
            Span::styled("+ ", Style::default().fg(p.good)),
            Span::styled(seg(&n, 0, prefix), dim),
            Span::styled(seg(&n, prefix, n.len() - suffix), Style::default().fg(p.good).add_modifier(Modifier::BOLD)),
            Span::styled(seg(&n, n.len() - suffix, n.len()), dim),
        ])
        .alignment(Alignment::Center),
    ]
}

/// Draws the "are you sure?" dialog. The Yes/No buttons and their hint are pinned to fixed rows
/// at the bottom via `Layout` rather than flowing inline after the message text. A long
/// filename/path in the message can make the message area wrap to more lines than it has room
/// for, but that just clips/scrolls the message itself; the buttons always stay in their own
/// reserved rows and can never be pushed out of frame by it.
fn draw_confirm_action(f: &mut Frame, action: &crate::app::ConfirmAction, renamable: bool, p: &Palette) {
    let screen = f.area();

    let mut message_lines: Vec<Line> =
        action.prompt.lines().map(|l| Line::from(l.to_string()).alignment(Alignment::Center)).collect();
    if let Some(dest) = &action.destination {
        message_lines.push(Line::raw(""));
        message_lines.push(
            Line::styled(dest.clone(), Style::default().fg(p.accent).add_modifier(Modifier::BOLD))
                .alignment(Alignment::Center),
        );
    }
    if let Some((old, new)) = &action.rename_diff {
        message_lines.push(Line::raw(""));
        message_lines.extend(rename_diff_lines(old, new, p));
    }

    let button = |label: &str, selected: bool, color: Color| {
        let style = if selected {
            Style::default().fg(p.on_accent).bg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        Span::styled(format!(" {label} "), style)
    };
    let buttons_line = Line::from(vec![
        button("Yes", action.yes_selected, p.good),
        Span::raw("   "),
        button("No", !action.yes_selected, p.bad),
    ])
    .alignment(Alignment::Center);
    let hint = if renamable {
        "tab/←→/h/l select · enter confirm · y/n shortcuts · r rename target"
    } else {
        "tab/←→/h/l select · enter confirm · y/n shortcuts"
    };
    let hint_line = Line::styled(hint, Style::default().fg(p.muted)).alignment(Alignment::Center);

    // Width is a generous fixed fraction of the screen (never more than it, minus a margin);
    // height fits the message as wrapped at that width, plus the fixed rows below it, capped to
    // what the screen actually has room for.
    let width = ((screen.width as u32 * 60 / 100) as u16).clamp(30, screen.width.saturating_sub(4).max(30));
    let content_width = width.saturating_sub(2).max(1); // borders
    // A safe over-estimate of wrapped rows (word-wrap can't need more rows than one per
    // `content_width` chars, plus one). No need for exact prediction, since the buttons below
    // are pinned to their own fixed rows regardless; this only sizes the box generously enough
    // that the message rarely needs to clip.
    let message_rows: u16 = message_lines
        .iter()
        .map(|line| {
            let chars: u16 = line.spans.iter().map(|s| s.content.chars().count() as u16).sum();
            chars / content_width + 1
        })
        .sum();
    let message = Paragraph::new(message_lines).wrap(Wrap { trim: false });
    const FIXED_ROWS: u16 = 4; // blank · buttons · blank · hint
    let max_height = screen.height.saturating_sub(2).max(FIXED_ROWS + 3);
    let height = (message_rows + FIXED_ROWS + 2).min(max_height); // +2 borders

    let area = centered_rect_exact(width, height, screen);
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" confirm ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
    f.render_widget(message, rows[0]);
    f.render_widget(Paragraph::new(buttons_line), rows[2]);
    f.render_widget(Paragraph::new(hint_line), rows[4]);
}

/// Like [`draw_confirm_action`], the `[y] confirm  [n/esc] cancel` line is pinned to its own
/// fixed bottom row rather than flowing after the message. A long bookmark filename must never
/// be able to wrap far enough to push it out of frame.
fn draw_confirm_bookmark_delete(f: &mut Frame, path: &str, p: &Palette) {
    let screen = f.area();
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let message = format!("delete bookmark '{name}'?");

    let width = ((screen.width as u32 * 50 / 100) as u16).clamp(24, screen.width.saturating_sub(4).max(24));
    let content_width = width.saturating_sub(2).max(1);
    let message_rows = (message.chars().count() as u16 / content_width) + 1;
    let height = (message_rows + 3).min(screen.height.saturating_sub(2).max(4)); // +1 blank, +1 instructions, +2 borders

    let area = centered_rect_exact(width, height, screen);
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" confirm delete bookmark ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.bad))
        .style(Style::default().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)])
        .split(inner);
    f.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), rows[0]);
    f.render_widget(Paragraph::new("[y] confirm   [n/esc] cancel"), rows[2]);
}

/// Formats a duration as "Ns ago" / "Nm ago" / "Nh ago", enough precision for a session log.
fn fmt_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// The `E` events log, every `set_status`/`set_error` this session, newest first, with any
/// error chain/diagnostics indented underneath.
fn draw_events(f: &mut Frame, app: &App, p: &Palette) {
    let area = centered_rect(75, 70, f.area());
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();
    if app.events.is_empty() {
        lines.push(Line::styled("no events yet", Style::default().fg(p.muted)));
    } else {
        for event in app.events.iter().rev() {
            let color = if event.is_error { p.bad } else { p.good };
            let icon = theme::glyph(if event.is_error { theme::Glyph::StatusFailed } else { theme::Glyph::StatusDone }, app.icons);
            let pad = if matches!(app.icons, crate::config::IconSet::Nerdfont) { "  " } else { " " };
            let when = fmt_elapsed(event.at.elapsed());
            lines.push(Line::from(vec![
                Span::styled(format!("{when:>8}  "), Style::default().fg(p.muted)),
                Span::styled(format!("{icon}{pad}"), Style::default().fg(color)),
                Span::styled(event.message.clone(), Style::default().fg(color)),
            ]));
            if let Some(detail) = &event.detail {
                for line in detail.lines() {
                    lines.push(Line::styled(format!("            {line}"), Style::default().fg(p.muted)));
                }
            }
        }
    }

    let widget = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.events_scroll, 0)).block(
        Block::default()
            .title(" events, ↑/↓ scroll · any other key closes ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent))
            .style(Style::default().bg(p.panel_bg).fg(p.text)),
    );
    f.render_widget(widget, area);
}

fn draw_help(f: &mut Frame, app: &App, p: &Palette) {
    let area = centered_rect(58, 82, f.area());
    f.render_widget(Clear, area);

    let header = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(p.accent).add_modifier(Modifier::BOLD)));
    let row = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<11}"), Style::default().fg(p.good).add_modifier(Modifier::BOLD)),
            Span::styled(d.to_string(), Style::default().fg(p.text)),
        ])
    };
    // Like `row`, but for a line that packs multiple key/description pairs separated by " · ".
    // Every key gets the same green as the leading one, instead of the whole rest of the line
    // being plain text (which left the embedded keys uncoloured).
    let row_multi = |pairs: &[(&str, &str)]| {
        let mut spans = Vec::new();
        for (i, (k, d)) in pairs.iter().enumerate() {
            if i == 0 {
                spans.push(Span::styled(format!("  {k:<11}"), Style::default().fg(p.good).add_modifier(Modifier::BOLD)));
            } else {
                spans.push(Span::styled("   ·   ".to_string(), Style::default().fg(p.muted)));
                spans.push(Span::styled(k.to_string(), Style::default().fg(p.good).add_modifier(Modifier::BOLD)));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(d.to_string(), Style::default().fg(p.text)));
        }
        Line::from(spans)
    };
    let blank = Line::raw("");

    let lines: Vec<Line> = vec![
        header("navigation"),
        row_multi(&[("j / k", "move cursor"), ("h / l", "up / open directory")]),
        row_multi(&[("gg / G", "top / bottom"), ("ctrl-d/u", "half page"), ("ctrl-f/b", "full page")]),
        row("space / v", "mark item  /  visual-mode range select"),
        row("V", "mark everything from the last mark to the cursor"),
        row("tab / 1-4", "cycle / jump focus: local · s3 · preview · jobs"),
        row("shift-click", "mark everything back to the anchor; ctrl/cmd-click picks one"),
        blank.clone(),
        header("sort & filter"),
        row("F1/F2/F3", "sort by name / size / modified"),
        row("/", "fuzzy filter (recurses); ↑/↓ move results while typing"),
        blank.clone(),
        header("transfers"),
        row("d / u", "download / upload marked (or hovered) item(s)"),
        row("s", "sync dialog, diff local ⇄ remote, never deletes"),
        row("r", "rename the hovered item"),
        row("O", "open in $EDITOR, remote changes upload back automatically"),
        blank.clone(),
        header("jobs pane (4, focused)"),
        row("enter / l", "open the selected job's local file/folder"),
        row("f", "reveal it in Finder"),
        row("c / r", "cancel the hovered job  /  run it again"),
        row("d / C", "dismiss the hovered finished job  /  all finished jobs"),
        row("L", "point the local pane at where it's downloading to"),
        blank.clone(),
        header("clipboard & delete"),
        row("y / x / P", "copy / cut / paste, any pane or backend"),
        row("D", "delete marked/hovered, no undo"),
        row("Y / U", "copy path  /  temporary share link"),
        blank.clone(),
        header("panes & view"),
        row("p / i", "pane 3: preview / info tab (same key again hides it)"),
        row("L", "toggle the local filesystem pane"),
        row("o / t", "open bookmark web_url  /  toggle light-dark theme"),
        blank.clone(),
        header("session"),
        row("E", "events log, every action this session, newest first"),
        row("b / q", "switch bookmark  /  quit"),
        row("esc", "clear the filter, else clear marks and the clipboard"),
        blank.clone(),
        header("bookmark list"),
        row("a / e / x", "add / edit / delete bookmark"),
        row("r", "rename the selected bookmark's profile"),
    ];
    let line_count = lines.len();

    f.render_widget(
        Paragraph::new(lines).scroll((app.help_scroll, 0)).block(
            Block::default()
                .title(" comhad help ")
                .title_bottom(" ↑/↓ scroll · any other key closes ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent))
                .style(Style::default().bg(p.panel_bg)),
        ),
        area,
    );
    render_pane_scrollbar(f, area, app.help_scroll as usize, line_count, p);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Centers a box of an exact `width`×`height` (clamped to fit `r`), rather than a percentage of
/// it, for dialogs sized to their own content instead of a fixed fraction of the screen.
fn centered_rect_exact(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    Rect {
        x: r.x + (r.width - width) / 2,
        y: r.y + (r.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The background of each column of a filled row, as a flat vector, for asserting on the
    /// shape of the bar rather than on how the spans happen to be chunked.
    fn column_backgrounds(spans: &[Span<'static>]) -> Vec<Option<Color>> {
        spans.iter().flat_map(|s| s.content.chars().map(move |_| s.style.bg)).collect()
    }

    fn row() -> Vec<Span<'static>> {
        vec![Span::raw("abcde"), Span::raw("fghij")]
    }

    #[test]
    fn progress_fill_splits_the_row_at_the_ratio() {
        let filled = Color::Rgb(1, 1, 1);
        let track = Color::Rgb(2, 2, 2);
        let out = progress_fill(row(), 10, 0.3, filled, Some(track));

        let bgs = column_backgrounds(&out);
        assert_eq!(bgs.len(), 10, "no columns are gained or lost");
        assert_eq!(bgs[..3], [Some(filled); 3], "the leading 30% is filled");
        assert_eq!(bgs[3..], [Some(track); 7], "the rest is track");
    }

    #[test]
    fn progress_fill_pads_a_short_row_out_to_the_full_width() {
        let filled = Color::Rgb(1, 1, 1);
        let out = progress_fill(row(), 20, 1.0, filled, None);

        let bgs = column_backgrounds(&out);
        assert_eq!(bgs.len(), 20, "the row is padded so the bar reaches the pane edge");
        assert!(bgs.iter().all(|bg| *bg == Some(filled)), "a complete transfer fills every column");
    }

    #[test]
    fn progress_fill_leaves_the_track_alone_when_none() {
        let out = progress_fill(row(), 10, 0.5, Color::Rgb(1, 1, 1), None);

        assert_eq!(column_backgrounds(&out)[5..], [None; 5], "the pane background shows through");
    }

    #[test]
    fn progress_fill_clamps_out_of_range_ratios() {
        let filled = Color::Rgb(1, 1, 1);
        let empty = progress_fill(row(), 10, -1.0, filled, None);
        let full = progress_fill(row(), 10, 2.0, filled, None);

        assert!(column_backgrounds(&empty).iter().all(|bg| bg.is_none()));
        assert!(column_backgrounds(&full).iter().all(|bg| *bg == Some(filled)));
    }

    #[test]
    fn a_gutter_cell_takes_the_most_important_row_it_covers() {
        let rows = [RowMark::Plain, RowMark::Marked, RowMark::Plain, RowMark::Transferring, RowMark::Staged];

        assert_eq!(rows.iter().copied().max(), Some(RowMark::Transferring));
        assert_eq!(rows[..3].iter().copied().max(), Some(RowMark::Marked), "one marked row in a run still shows");
    }

    #[test]
    fn gutter_marks_rank_in_flight_over_staged_over_marked() {
        assert_eq!(row_gutter_mark(true, Some(ClipMode::Copy), true), RowMark::Transferring);
        assert_eq!(row_gutter_mark(true, Some(ClipMode::Move), false), RowMark::Staged);
        assert_eq!(row_gutter_mark(true, None, false), RowMark::Marked);
        assert_eq!(row_gutter_mark(false, None, false), RowMark::Plain);
    }

    #[test]
    fn progress_fill_keeps_foreground_styling() {
        let styled = vec![Span::styled("abcde", Style::default().fg(Color::Rgb(9, 9, 9)))];
        let out = progress_fill(styled, 5, 0.4, Color::Rgb(1, 1, 1), None);

        assert!(
            out.iter().all(|s| s.style.fg == Some(Color::Rgb(9, 9, 9))),
            "the fill only touches backgrounds, so icon and match colours survive"
        );
    }
}
