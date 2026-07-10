pub mod theme;

use humansize::{format_size, BINARY};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ClipMode, Focus, Preview, PreviewMode, PromptKind, Screen, SyncAction, BOOKMARK_FIELDS};
use crate::jobs::{JobKind, JobStatus};
use theme::Palette;

pub fn draw(f: &mut Frame, app: &mut App) {
    let p = app.theme.palette();
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
        draw_confirm_action(f, action, &p);
    }
    if let Some(path) = &app.confirm_bookmark_delete {
        draw_confirm_bookmark_delete(f, path, &p);
    }
    if let Some(prompt) = &app.prompt {
        // The filter prompt renders inline in the focused pane's header (k9s-style), not as a
        // centered popup.
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
            "  no bookmarks yet — press 'a' to add one",
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
                    Span::styled(format!(" {}", conn.path), Style::default().fg(p.muted)),
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
    f.render_widget(list, centered_rect(70, 60, chunks[1]));

    let footer = Paragraph::new(Line::from(vec![Span::styled(
        "  ↑/↓ move   enter connect   a add   e edit   x delete   t theme   q quit",
        Style::default().fg(p.muted),
    )]));
    f.render_widget(footer, chunks[2]);
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
    f.render_widget(list, centered_rect(70, 60, chunks[1]));

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

    draw_downloads_strip(f, app, chunks[2], p);

    let footer_text = if let Some((msg, is_err)) = &app.status {
        let spans = vec![
            Span::styled(format!("  {msg}"), Style::default().fg(if *is_err { p.bad } else { p.good })),
            Span::styled("  (E for events)", Style::default().fg(p.muted)),
        ];
        Line::from(spans)
    } else {
        // The local pane picks where downloads land; while it's hidden that's not obvious,
        // so spell it out here too (as well as in the download confirm dialog) rather than
        // leaving `L` as a keybinding you'd only find by reading the full help screen.
        let download_hint = if app.show_local { "d↓" } else { "d↓ (L: browse/change dest)" };
        Line::from(Span::styled(
            format!(
                "  ↑/↓ nav  ↵ open  space mark  {download_hint} u↑ s sync  y/x/P copy/cut/paste  D delete  / filter  E events  ? help  q quit"
            ),
            Style::default().fg(p.muted),
        ))
    };
    f.render_widget(Paragraph::new(footer_text), chunks[3]);
}

fn pane_border_style(focused: bool, p: &Palette) -> Style {
    if focused {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.accent_dim)
    }
}

/// When the filter prompt is open for `pane` (and `pane` is focused), returns the k9s-style
/// title that replaces the pane's normal header with a live filter input, e.g. ` local: foo▏ `.
fn active_filter_title(app: &App, pane: Focus) -> Option<String> {
    if app.focus != pane {
        return None;
    }
    let prompt = app.prompt.as_ref()?;
    if prompt.kind != PromptKind::Filter {
        return None;
    }
    let label = if pane == Focus::Local { "local" } else { "s3" };
    Some(format!(" {label}: {}▏ ", prompt.buffer))
}

fn draw_local_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Local;
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
                format!(" — filter: {filt}{deep_note}")
            }
            _ => String::new(),
        };
        let sort_badge = app.local_sort.label().map(|l| format!(" ⇅ {l}")).unwrap_or_default();
        format!(" [1] local: {}{filter_badge}{sort_badge} ", app.local_cwd.display())
    });

    let visible = app.visible_local_entries();
    let visible_len = visible.len();
    let total_len = visible_len + deep_extra_len;
    let mut items: Vec<ListItem> = if total_len == 0 {
        vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))]
    } else {
        visible
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let selected = focused && i == app.local_cursor;
                let marked = app.local_marked.contains(&entry.path);
                let clip_mode = app.clip_mode_for_local(&entry.path);
                let icon = theme::icon_for(&entry.name, entry.is_dir);
                let size = if entry.is_dir { String::new() } else { format_size(entry.size, BINARY) };
                let name_color = match clip_mode {
                    Some(ClipMode::Copy) => p.good,
                    Some(ClipMode::Move) => p.bad,
                    None if entry.is_dir => p.dir,
                    None => p.text,
                };
                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };
                let mark = match (marked, clip_mode) {
                    (_, Some(ClipMode::Copy)) => "⧉ ",
                    (_, Some(ClipMode::Move)) => "✂ ",
                    (true, None) => "✓ ",
                    (false, None) => "  ",
                };
                let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
                let modified = entry
                    .modified
                    .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();

                ListItem::new(Line::from(vec![
                    Span::styled(mark, mark_style),
                    Span::styled(format!("{icon} "), base_style),
                    Span::styled(format!("{:<28}", entry.name), base_style),
                    Span::styled(format!("{size:>10}  "), if selected { base_style } else { Style::default().fg(p.muted) }),
                    Span::styled(modified, if selected { base_style } else { Style::default().fg(p.muted) }),
                ]))
            })
            .collect()
    };

    // `/`'s deep matches — found somewhere under the current directory, but not a direct
    // child of it — appended below the normal listing with their relative path shown, since
    // "hello.csv" alone wouldn't say which one this is.
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
            let mark = match (marked, clip_mode) {
                (_, Some(ClipMode::Copy)) => "⧉ ",
                (_, Some(ClipMode::Move)) => "✂ ",
                (true, None) => "✓ ",
                (false, None) => "  ",
            };
            let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(mark, mark_style),
                Span::styled("↳ ", base_style),
                Span::styled(rel, base_style),
            ])));
        }
    }

    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.local_list_state.select(if total_len == 0 { None } else { Some(app.local_cursor) });
    f.render_stateful_widget(list, area, &mut app.local_list_state);
}

fn draw_remote_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Remote;
    let visible = app.visible_entries();
    let deep_extra_len = app.deep_remote.as_ref().map(|d| d.extra.len()).unwrap_or(0);
    let deep_loading = app.deep_remote.as_ref().is_some_and(|d| d.loading);
    let truncated_scan = app.deep_remote.as_ref().is_some_and(|d| d.truncated_scan);
    let title = active_filter_title(app, Focus::Remote).unwrap_or_else(|| {
        let filter_badge = match &app.filter {
            Some(filt) if !filt.is_empty() => {
                let deep_note = if deep_loading {
                    format!(" {} scanning...", theme::spinner(app.spinner_frame))
                } else if deep_extra_len > 0 {
                    format!(", +{deep_extra_len} elsewhere{}", if truncated_scan { " (scan capped)" } else { "" })
                } else {
                    String::new()
                };
                format!(" — filter: {filt}{deep_note}")
            }
            _ => String::new(),
        };
        let sort_badge = app.remote_sort.label().map(|l| format!(" ⇅ {l}")).unwrap_or_default();
        format!(" [2] s3://{}/{}{filter_badge}{sort_badge} ", app.bucket, app.prefix)
    });

    let visible_len = visible.len();
    let total_len = visible_len + deep_extra_len;
    let mut items: Vec<ListItem> = if total_len == 0 {
        vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))]
    } else {
        visible
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let selected = focused && i == app.cursor;
                let marked = app.marked.contains(&entry.key);
                let clip_mode = app.clip_mode_for_remote(&entry.key);
                let icon = theme::icon_for(&entry.name, entry.is_dir);
                let size = if entry.is_dir { String::new() } else { format_size(entry.size.max(0) as u64, BINARY) };
                let name_color = match clip_mode {
                    Some(ClipMode::Copy) => p.good,
                    Some(ClipMode::Move) => p.bad,
                    None if entry.is_dir => p.dir,
                    None => p.text,
                };

                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };

                let mark = match (marked, clip_mode) {
                    (_, Some(ClipMode::Copy)) => "⧉ ",
                    (_, Some(ClipMode::Move)) => "✂ ",
                    (true, None) => "✓ ",
                    (false, None) => "  ",
                };
                let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };

                let modified = entry.last_modified.as_deref().unwrap_or("");

                ListItem::new(Line::from(vec![
                    Span::styled(mark, mark_style),
                    Span::styled(format!("{icon} "), base_style),
                    Span::styled(format!("{:<28}", entry.name), base_style),
                    Span::styled(format!("{size:>10}  "), if selected { base_style } else { Style::default().fg(p.muted) }),
                    Span::styled(modified.to_string(), if selected { base_style } else { Style::default().fg(p.muted) }),
                ]))
            })
            .collect()
    };

    // `/`'s deep matches — found somewhere under the current prefix, but not a direct child
    // of it — appended below the normal listing with their relative key shown, since the
    // basename alone wouldn't say which copy this is (see `hello.csv` at the root vs. under
    // `archive/2024/`).
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
            let mark = match (marked, clip_mode) {
                (_, Some(ClipMode::Copy)) => "⧉ ",
                (_, Some(ClipMode::Move)) => "✂ ",
                (true, None) => "✓ ",
                (false, None) => "  ",
            };
            let mark_style = if selected { base_style } else { Style::default().fg(p.accent) };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(mark, mark_style),
                Span::styled("↳ ", base_style),
                Span::styled(rel.to_string(), base_style),
            ])));
        }
    }

    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.list_state.select(if total_len == 0 { None } else { Some(app.cursor) });
    f.render_stateful_widget(list, area, &mut app.list_state);
}

/// Builds pane 3's title: `[3]` plus both tab labels (`Preview` / `Info`), whichever is
/// active highlighted, plus an optional trailing detail (file size, truncation note) —
/// both tabs are always shown so `p`/`i` read as "select this tab" rather than a hidden
/// toggle you'd only discover by accident.
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

    // Images render through a stateful `ratatui_image` widget (it needs `&mut` access to its
    // encoded protocol state), so handle that case up front and fall through to the plain
    // `Paragraph` path — shared by every other preview kind — for everything else. Title is
    // built from an immutable borrow first, since it needs all of `app`, not just `preview`.
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

    // Shown on every "can't preview content" state, so `i` is discoverable rather than a
    // hidden keybinding — but not on `Empty` (nothing hovered, so nothing to show info for
    // either) or `Text` (content itself, handled separately below and never centered/hinted).
    const INFO_HINT: &str = "press i for info";

    let is_content = matches!(&app.preview, Preview::Text { .. });
    let (detail, body, color): (String, Text, Color) = match &app.preview {
        Preview::Empty => (String::new(), Text::raw("(nothing selected)"), p.muted),
        Preview::Loading => {
            (String::new(), Text::raw(format!("{} loading...", theme::spinner(app.spinner_frame))), p.muted)
        }
        Preview::Directory => (String::new(), Text::raw(format!("(directory)\n\n{INFO_HINT}")), p.muted),
        Preview::TooLarge { size } => (
            String::new(),
            Text::raw(format!("file too large to preview ({})\n\n{INFO_HINT}", format_size(*size, BINARY))),
            p.muted,
        ),
        Preview::Binary { size } => (
            String::new(),
            Text::raw(format!("binary file, {}\n\n{INFO_HINT}", format_size(*size, BINARY))),
            p.muted,
        ),
        Preview::Error(err) => {
            (String::new(), Text::raw(format!("preview error: {err}\n\n{INFO_HINT}")), p.bad)
        }
        // Handled by the early return above.
        Preview::Image { .. } => unreachable!(),
        Preview::Info(info) => (String::new(), info_body(info), p.text),
        Preview::Text { text, size, truncated, highlight } => {
            let detail = if *truncated {
                format!("{}, showing first bytes", format_size(*size, BINARY))
            } else {
                format_size(*size, BINARY)
            };
            // Highlighted spans when the type is recognized, otherwise plain text.
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
    let mut widget = Paragraph::new(center_lines(body, area, !is_content))
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0))
        .style(Style::default().fg(color))
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(pane_border_style(focused, p))
                .style(Style::default().bg(p.panel_bg)),
        );
    if !is_content {
        widget = widget.alignment(Alignment::Center);
    }
    f.render_widget(widget, area);
}

fn draw_downloads_strip(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Transfers;
    let items: Vec<ListItem> = if app.jobs.is_empty() {
        vec![ListItem::new(Span::styled("  no transfers yet", Style::default().fg(p.muted)))]
    } else {
        app.jobs
            .iter()
            .rev()
            .enumerate()
            .map(|(i, job)| {
                let selected = focused && i == app.jobs_cursor;
                let kind_icon = match job.kind {
                    JobKind::Download => "↓",
                    JobKind::Upload => "↑",
                    // A zip job is really a bundled download — same direction as a plain
                    // download, doubled to signal "several files, not one".
                    JobKind::Zip => "⇊",
                };
                let (status_icon, color) = match &job.status {
                    JobStatus::Running => (theme::spinner(app.spinner_frame), p.accent),
                    JobStatus::Done => ("✅", p.good),
                    JobStatus::Failed(_) => ("❌", p.bad),
                };
                // A full-block bar glyph paints solid over whatever background it's drawn on,
                // so on a finished (and thus always 100%-filled) job it would blank out the
                // selection highlight with a wall of white — once done, the checkmark already
                // says "complete", so just show the final size instead of a stale full bar.
                let detail = match &job.status {
                    JobStatus::Failed(err) => err.clone(),
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
                // Keep the detail text in its status color even when selected — a bg tint is
                // enough of a highlight without fighting the bar glyphs for the foreground.
                let detail_style =
                    if selected { Style::default().fg(color).bg(p.accent) } else { Style::default().fg(color) };

                ListItem::new(Line::from(vec![
                    Span::styled(marker, base_style),
                    Span::raw(format!("{status_icon} {kind_icon} ")),
                    Span::styled(format!("{:<24}", job.label), base_style),
                    Span::styled(format!(" {detail}"), detail_style),
                ]))
            })
            .collect()
    };

    let title = if focused {
        format!(" [4] transfers ({}) — enter open, f reveal in finder ", app.jobs.len())
    } else {
        format!(" [4] transfers ({}) ", app.jobs.len())
    };
    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.jobs_list_state.select(if app.jobs.is_empty() { None } else { Some(app.jobs_cursor) });
    f.render_stateful_widget(list, area, &mut app.jobs_list_state);
}

fn draw_sync(f: &mut Frame, app: &mut App, p: &Palette) {
    let dir_label = app.sync.as_ref().map(|s| s.direction.label().to_string()).unwrap_or_default();
    let arrow = app.sync.as_ref().map(|s| s.direction.arrow().to_string()).unwrap_or_default();
    let dest_is_local = app.sync.as_ref().map(|s| s.direction) == Some(crate::app::SyncDirection::RemoteToLocal);
    let actionable = app.sync.as_ref().map(|s| s.actionable()).unwrap_or(0);
    let total = app.sync.as_ref().map(|s| s.entries.len()).unwrap_or(0);

    let Some(state) = app.sync.as_mut() else { return };

    // Size the dialog to its content (with a screen margin) rather than forcing full height —
    // a short diff gets a short dialog. Panel interior rows = dialog height − 4 (outer borders
    // + panel borders); the rest is margin.
    let screen = f.area();
    let content_rows = state.entries.len().max(1);
    let max_rows = (screen.height as usize).saturating_sub(8).max(1);
    let rows = content_rows.min(max_rows);
    let dialog_h = (rows as u16) + 4;
    let dialog_w = (screen.width as u32 * 90 / 100) as u16;
    let area = Rect {
        x: screen.x + screen.width.saturating_sub(dialog_w) / 2,
        y: screen.y + screen.height.saturating_sub(dialog_h) / 2,
        width: dialog_w,
        height: dialog_h,
    };
    f.render_widget(Clear, area);

    let title = format!(" sync — {dir_label}   ({actionable} of {total} to transfer) ");
    let footer =
        " ↑/↓ move · tab/d flip · enter run · esc close · +add · ~change · =same · -extra (skipped) ";
    let outer = Block::default()
        .title(title)
        .title_bottom(footer)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.panel_bg).fg(p.text));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // Two bordered panels with a direction-arrow gutter between them.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(5), Constraint::Min(0)])
        .split(inner);
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
        // Unchanged and extra-on-destination are both no-ops (extras are never deleted), so
        // both are greyed out — the `=` vs `-` icon carries the "exists on one side only"
        // distinction without implying a change that isn't happening.
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

    // Renders one side's cell. `display` is whether this side shows the file at all — a plain
    // add appears on the *destination* side too (in green), projecting the post-sync state, so
    // you can see the file "pop up" on the other panel.
    let side_line =
        |display: bool, icon: char, name: &str, size: Option<u64>, mtime: Option<i64>, width: usize, color: Color, selected: bool| {
            let text = if display {
                let sz = size.map(|s| format_size(s, BINARY)).unwrap_or_default();
                let ts = fmt_mtime(mtime);
                let name_w = width.saturating_sub(2 + 10 + 17).max(4);
                format!("{icon} {:<name_w$} {sz:>9} {ts:>16}", truncate(name, name_w))
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
        left_lines.push(Line::styled("  both sides are empty", Style::default().fg(p.muted)));
    }
    for (i, entry) in state.entries.iter().enumerate().skip(state.offset).take(rows) {
        let selected = i == state.cursor;
        let color = color_for(entry.action);
        let icon = icon_for(entry.action);
        let is_add = entry.action == SyncAction::Add;

        // Local (left) side: shown if it has the file, or if it's an add landing locally.
        let local_display = entry.local_size.is_some() || (is_add && dest_is_local);
        let (l_size, l_mtime) = if entry.local_size.is_some() {
            (entry.local_size, entry.local_mtime)
        } else {
            (entry.remote_size, entry.remote_mtime) // projected incoming values
        };
        left_lines.push(side_line(local_display, icon, &entry.rel, l_size, l_mtime, left_w, color, selected));

        // Remote (right) side: mirror.
        let remote_display = entry.remote_size.is_some() || (is_add && !dest_is_local);
        let (r_size, r_mtime) = if entry.remote_size.is_some() {
            (entry.remote_size, entry.remote_mtime)
        } else {
            (entry.local_size, entry.local_mtime)
        };
        right_lines.push(side_line(remote_display, icon, &entry.rel, r_size, r_mtime, right_w, color, selected));

        // Extra files never move, so their gutter shows a dot rather than a direction arrow.
        let glyph = if entry.action == SyncAction::ExtraSkipped { "·" } else { arrow.as_str() };
        let style = if selected {
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        mid_lines.push(Line::from(Span::styled(format!("{glyph:^5}"), style)));
    }

    let (left_label, right_label) = if dest_is_local {
        (" local (dest) ", " remote (source) ")
    } else {
        (" local (source) ", " remote (dest) ")
    };
    let panel = |label: &str| {
        Block::default()
            .title(label.to_string())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent_dim))
    };
    f.render_widget(Paragraph::new(left_lines).block(panel(left_label)), left_rect);
    f.render_widget(Paragraph::new(mid_lines), mid_rect);
    f.render_widget(Paragraph::new(right_lines).block(panel(right_label)), right_rect);
}

/// Pads `body` with blank lines above it so it sits vertically centered in `area` (its own
/// horizontal centering comes from the caller setting `Alignment::Center` on the widget). A
/// no-op when `should_center` is false — real file content stays top-aligned since you're
/// reading it top to bottom, not glancing at a short status message.
fn center_lines(body: Text<'_>, area: Rect, should_center: bool) -> Text<'_> {
    if !should_center {
        return body;
    }
    let inner_h = area.height.saturating_sub(2) as usize; // account for the pane's own borders
    let vpad = inner_h.saturating_sub(body.lines.len()) / 2;
    if vpad == 0 {
        return body;
    }
    let mut lines = vec![Line::raw(""); vpad];
    lines.extend(body.lines);
    Text::from(lines)
}

/// Builds the `i` info view's body: name, location, size, last-modified, and whatever
/// backend-specific `extra` metadata (S3's ETag/Content-Type/etc) came back — rendered
/// generically since a future non-S3 backend may return different fields entirely.
fn info_body(info: &crate::app::InfoDetails) -> Text<'static> {
    let mut lines = vec![
        Line::from(vec![Span::styled("Name: ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(info.name.clone())]),
        Line::from(vec![Span::styled("Key: ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(info.key.clone())]),
    ];
    if let Some(loc) = &info.remote_location {
        lines.push(Line::from(vec![
            Span::styled("Location: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(loc.clone()),
        ]));
    }
    if let Some(path) = &info.local_path {
        lines.push(Line::from(vec![
            Span::styled("Path: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(path.clone()),
        ]));
    }
    if info.is_dir {
        lines.push(Line::raw(""));
        lines.push(Line::raw("(directory — no object metadata)"));
        return Text::from(lines);
    }
    if let Some(size) = info.size {
        lines.push(Line::from(vec![
            Span::styled("Size: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format_size(size, BINARY)),
        ]));
    }
    if let Some(modified) = &info.last_modified {
        lines.push(Line::from(vec![
            Span::styled("Last Modified: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(modified.clone()),
        ]));
    }
    for (label, value) in &info.extra {
        lines.push(Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(value.clone()),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("press i to preview content", Style::default().add_modifier(Modifier::ITALIC)));
    Text::from(lines)
}

/// Formats a Unix-seconds timestamp as a local `YYYY-MM-DD HH:MM`, or empty for `None`.
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
        PromptKind::UploadPath => " upload local path (drag a file here, or paste one) ".to_string(),
        PromptKind::Filter => " filter ".to_string(),
        PromptKind::BookmarkField => match &app.bookmark_wizard {
            Some(w) => {
                let (label, _, optional) = BOOKMARK_FIELDS[w.field_index];
                let opt = if optional { ", optional — enter to skip" } else { "" };
                format!(" bookmark field {}/{}: {label}{opt} ", w.field_index + 1, BOOKMARK_FIELDS.len())
            }
            None => " bookmark ".to_string(),
        },
    };
    let shown = if prompt.mask { "•".repeat(prompt.buffer.chars().count()) } else { prompt.buffer.clone() };
    let text = format!("{shown}▏");
    let widget = Paragraph::new(text).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent))
            .style(Style::default().bg(p.panel_bg).fg(p.text)),
    );
    f.render_widget(widget, area);
}

fn draw_confirm_action(f: &mut Frame, action: &crate::app::ConfirmAction, p: &Palette) {
    let area = centered_rect(50, 25, f.area());
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> =
        action.prompt.lines().map(|l| Line::from(l.to_string()).alignment(Alignment::Center)).collect();
    if let Some(dest) = &action.destination {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            dest.clone(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ).alignment(Alignment::Center));
    }
    lines.push(Line::raw(""));

    let button = |label: &str, selected: bool, color: Color| {
        let style = if selected {
            Style::default().fg(p.on_accent).bg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        Span::styled(format!(" {label} "), style)
    };
    lines.push(
        Line::from(vec![
            button("Yes", action.yes_selected, p.good),
            Span::raw("   "),
            button("No", !action.yes_selected, p.bad),
        ])
        .alignment(Alignment::Center),
    );
    lines.push(Line::raw(""));
    lines.push(Line::styled("tab/←→ select · enter confirm · y/n shortcuts", Style::default().fg(p.muted)).alignment(Alignment::Center));

    // `trim: true` strips leading/trailing whitespace off each *line* — which, since the
    // Yes/No line's leading space is also the line's leading whitespace, was eating the
    // button's left padding (its background started right at the "Y", with no space before
    // it, while the trailing space survived since it isn't at the line's edge).
    let widget = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" confirm ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent))
            .style(Style::default().bg(p.panel_bg).fg(p.text)),
    );
    f.render_widget(widget, area);
}

fn draw_confirm_bookmark_delete(f: &mut Frame, path: &str, p: &Palette) {
    let area = centered_rect(50, 20, f.area());
    f.render_widget(Clear, area);
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let body = format!("delete bookmark '{name}'?\n\n[y] confirm   [n/esc] cancel");
    let widget = Paragraph::new(body).wrap(Wrap { trim: true }).block(
        Block::default()
            .title(" confirm delete bookmark ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.bad))
            .style(Style::default().bg(p.panel_bg).fg(p.text)),
    );
    f.render_widget(widget, area);
}

/// Formats a duration as a short "Ns ago" / "Nm ago" / "Nh ago" — precise enough for a log
/// that's only ever showing the last few minutes of a session, not a full timestamp.
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

/// The `E` events log — every `set_status`/`set_error` this session, newest first, with the
/// full error chain/connection diagnostics shown indented under any event that has one.
fn draw_events(f: &mut Frame, app: &App, p: &Palette) {
    let area = centered_rect(75, 70, f.area());
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();
    if app.events.is_empty() {
        lines.push(Line::styled("no events yet", Style::default().fg(p.muted)));
    } else {
        for event in app.events.iter().rev() {
            let color = if event.is_error { p.bad } else { p.good };
            let icon = if event.is_error { "✖" } else { "✔" };
            let when = fmt_elapsed(event.at.elapsed());
            lines.push(Line::from(vec![
                Span::styled(format!("{when:>8}  "), Style::default().fg(p.muted)),
                Span::styled(format!("{icon} "), Style::default().fg(color)),
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
            .title(" events — ↑/↓ scroll · any other key closes ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent))
            .style(Style::default().bg(p.panel_bg).fg(p.text)),
    );
    f.render_widget(widget, area);
}

fn draw_help(f: &mut Frame, app: &App, p: &Palette) {
    let area = centered_rect(65, 80, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        "navigation",
        "  ↑/k, ↓/j     move cursor",
        "  →/l/enter    open directory",
        "  ←/h/bksp     go up a directory",
        "  space        mark/unmark item",
        "  tab          switch focus forward through local / s3 / preview / transfers",
        "  shift+tab    switch focus backward through the panes",
        "  1-4          jump focus directly to local / s3 / preview / transfers",
        "",
        "sorting & filtering (act on the focused pane)",
        "  F1           sort by name    — cycles off → ascending → descending",
        "  F2           sort by size",
        "  F3           sort by modified",
        "  /            filter the focused pane by name — ↑/↓ move through results while",
        "               still typing, no need to press enter first",
        "               also scans recursively (once) and appends anything matching found",
        "               elsewhere, path shown, distinct color — so e.g. hello.csv at the",
        "               root and hello.csv under a nested folder both show up",
        "",
        "transfers",
        "  d            download marked/hovered s3 object(s)   (s3 pane only)",
        "  u            upload marked/hovered local file(s)    (local pane only)",
        "               (drag a file onto the window to upload without the local pane)",
        "  r            rename the hovered item",
        "  s            sync dialog — diff local ⇄ remote, transfer missing/newer",
        "               (in the dialog: tab/d flips direction, enter runs, never deletes)",
        "",
        "clipboard (move/copy) & delete",
        "  y            copy (yank) marked/hovered item(s) — stage for paste",
        "  x            cut marked/hovered item(s) — stage for paste (moves on paste)",
        "  P            paste the staged item(s) into the focused pane's current location",
        "               (works within a pane, across panes, and between local/s3)",
        "  Y            copy the hovered item's s3://bucket/key or local path to the OS clipboard",
        "  D            permanently delete marked/hovered item(s) — no undo",
        "",
        "on the transfers pane (focus it with tab or 4):",
        "  ↑/k, ↓/j     move between transfers",
        "  ↵/l          open the transfer's local file/folder with the default app",
        "  f            reveal the transfer's local file/folder in Finder",
        "",
        "panes & view",
        "  p            pane 3: select the Preview tab (file content)",
        "  i            pane 3: select the Info tab (name, key, size, last-modified, ETag,",
        "               etc — works even when there's nothing to content-preview)",
        "               pressing the tab that's already active hides the pane; the other",
        "               key just switches tabs without hiding it",
        "  L            toggle the local filesystem pane (off by default)",
        "  o            open bookmark's web_url in your browser",
        "  t            toggle light/dark theme",
        "",
        "session",
        "  E            events log — every status message this session (uploads, downloads,",
        "               failures, ...), newest first, with full detail under any error",
        "               (the footer toast itself clears after a few seconds either way)",
        "  c            switch bookmark",
        "  esc          cancel / clear filter / clear marks",
        "  q            quit",
        "",
        "on the bookmark list:",
        "  a            add a bookmark",
        "  e            edit the selected bookmark",
        "  x            delete the selected bookmark",
    ];
    let widget = Paragraph::new(lines.join("\n"))
        .scroll((app.help_scroll, 0))
        .block(
            Block::default()
                .title(" comhad help ")
                .title_bottom(" ↑/↓ scroll · any other key closes ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.accent))
                .style(Style::default().bg(p.panel_bg).fg(p.text)),
        );
    f.render_widget(widget, area);
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
