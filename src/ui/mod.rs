pub mod theme;

use humansize::{format_size, BINARY};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, Preview, PromptKind, Screen, SyncAction, BOOKMARK_FIELDS};
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
        draw_confirm_action(f, &action.prompt, &p);
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
    if app.show_diagnostics {
        draw_diagnostics(f, app, &p);
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
        let mut spans = vec![Span::styled(format!("  {msg}"), Style::default().fg(if *is_err { p.bad } else { p.good }))];
        if *is_err && app.last_error.is_some() {
            spans.push(Span::styled("  (E for details)", Style::default().fg(p.muted)));
        }
        Line::from(spans)
    } else {
        Line::from(Span::styled(
            "  ↑/↓ nav  ↵ open  space mark  d download  u upload  s sync  / filter  tab focus  ? help  q quit",
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
    let title = active_filter_title(app, Focus::Local).unwrap_or_else(|| {
        let filter_badge = match &app.local_filter {
            Some(filt) if !filt.is_empty() => format!(" — filter: {filt}"),
            _ => String::new(),
        };
        let sort_badge = app.local_sort.label().map(|l| format!(" ⇅ {l}")).unwrap_or_default();
        format!(" [1] local: {}{filter_badge}{sort_badge} ", app.local_cwd.display())
    });

    let visible = app.visible_local_entries();
    let visible_len = visible.len();
    let items: Vec<ListItem> = if visible.is_empty() {
        vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))]
    } else {
        visible
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let selected = focused && i == app.local_cursor;
                let marked = app.local_marked.contains(&entry.path);
                let icon = theme::icon_for(&entry.name, entry.is_dir);
                let size = if entry.is_dir { String::new() } else { format_size(entry.size, BINARY) };
                let name_color = if entry.is_dir { p.dir } else { p.text };
                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };
                let mark = if marked { "✓ " } else { "  " };
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

    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.local_list_state.select(if visible_len == 0 { None } else { Some(app.local_cursor) });
    f.render_stateful_widget(list, area, &mut app.local_list_state);
}

fn draw_remote_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Remote;
    let visible = app.visible_entries();
    let title = active_filter_title(app, Focus::Remote).unwrap_or_else(|| {
        let filter_badge = match &app.filter {
            Some(filt) if !filt.is_empty() => format!(" — filter: {filt}"),
            _ => String::new(),
        };
        let sort_badge = app.remote_sort.label().map(|l| format!(" ⇅ {l}")).unwrap_or_default();
        format!(" [2] s3://{}/{}{filter_badge}{sort_badge} ", app.bucket, app.prefix)
    });

    let visible_len = visible.len();
    let items: Vec<ListItem> = if visible.is_empty() {
        vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))]
    } else {
        visible
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let selected = focused && i == app.cursor;
                let marked = app.marked.contains(&entry.key);
                let icon = theme::icon_for(&entry.name, entry.is_dir);
                let size = if entry.is_dir { String::new() } else { format_size(entry.size.max(0) as u64, BINARY) };
                let name_color = if entry.is_dir { p.dir } else { p.text };

                let base_style = if selected {
                    Style::default().fg(p.on_accent).bg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };

                let mark = if marked { "✓ " } else { "  " };
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

    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(pane_border_style(focused, p))
            .style(Style::default().bg(p.panel_bg)),
    );
    app.list_state.select(if visible_len == 0 { None } else { Some(app.cursor) });
    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_preview_pane(f: &mut Frame, app: &App, area: Rect, p: &Palette) {
    let (title, body, color): (String, Text, Color) = match &app.preview {
        Preview::Empty => (" preview ".to_string(), Text::raw("(nothing selected)"), p.muted),
        Preview::Loading => {
            (" preview ".to_string(), Text::raw(format!("{} loading...", theme::spinner(app.spinner_frame))), p.muted)
        }
        Preview::Directory => (" preview ".to_string(), Text::raw("(directory)"), p.muted),
        Preview::TooLarge { size } => (
            " preview ".to_string(),
            Text::raw(format!("file too large to preview ({})", format_size(*size, BINARY))),
            p.muted,
        ),
        Preview::Binary { size } => {
            (" preview ".to_string(), Text::raw(format!("binary file, {}", format_size(*size, BINARY))), p.muted)
        }
        Preview::Error(err) => (" preview ".to_string(), Text::raw(format!("preview error: {err}")), p.bad),
        Preview::Text { text, size, truncated, highlight } => {
            let title = if *truncated {
                format!(" preview ({}, showing first bytes) ", format_size(*size, BINARY))
            } else {
                format!(" preview ({}) ", format_size(*size, BINARY))
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
            (title, body, p.text)
        }
    };

    let focused = app.focus == Focus::Preview;
    let title = format!(" [3] {} ", title.trim());
    let widget = Paragraph::new(body)
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
                    JobKind::Download => "v",
                    JobKind::Upload => "^",
                    JobKind::Zip => "z",
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

fn draw_confirm_action(f: &mut Frame, prompt: &str, p: &Palette) {
    let area = centered_rect(50, 20, f.area());
    f.render_widget(Clear, area);
    let body = format!("{prompt}\n\n[y] yes   [n/esc] cancel");
    let widget = Paragraph::new(body).wrap(Wrap { trim: true }).alignment(Alignment::Center).block(
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

fn draw_diagnostics(f: &mut Frame, app: &App, p: &Palette) {
    let area = centered_rect(75, 70, f.area());
    f.render_widget(Clear, area);
    let text = app.last_error.clone().unwrap_or_else(|| "no error recorded".to_string());
    let widget = Paragraph::new(text).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" diagnostics — press any key to close ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.bad))
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
        "  /            filter the focused pane by name",
        "",
        "transfers",
        "  d            download marked/hovered s3 object(s)   (s3 pane only)",
        "  u            upload marked/hovered local file(s)    (local pane only)",
        "               (drag a file onto the window to upload without the local pane)",
        "  r            rename the hovered item",
        "  s            sync dialog — diff local ⇄ remote, transfer missing/newer",
        "               (in the dialog: tab/d flips direction, enter runs, never deletes)",
        "",
        "on the transfers pane (focus it with tab or 4):",
        "  ↑/k, ↓/j     move between transfers",
        "  ↵/l          open the transfer's local file/folder with the default app",
        "  f            reveal the transfer's local file/folder in Finder",
        "",
        "panes & view",
        "  p            toggle the preview pane",
        "  L            toggle the local filesystem pane (off by default)",
        "  o            open bookmark's web_url in your browser",
        "  t            toggle light/dark theme",
        "",
        "session",
        "  E            show full error details (after a failure)",
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
