pub mod theme;

use humansize::{format_size, BINARY};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, Preview, PromptKind, Screen, BOOKMARK_FIELDS};
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

    if let Some(path) = &app.confirm_bookmark_delete {
        draw_confirm_bookmark_delete(f, path, &p);
    }
    if let Some(prompt) = &app.prompt {
        draw_prompt(f, app, prompt, &p);
    }
    if app.show_diagnostics {
        draw_diagnostics(f, app, &p);
    }
    if app.show_help {
        draw_help(f, &p);
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
            "  ↑/↓ nav  ↵/l open  h up  space mark  d download  r rename  / filter  tab/1-4 focus  p preview  L local  o web  t theme  c switch  q quit",
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

fn draw_local_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Local;
    let title = format!(" [1] local: {} ", app.local_cwd.display());

    let items: Vec<ListItem> = if app.local_entries.is_empty() {
        vec![ListItem::new(Span::styled("  (empty)", Style::default().fg(p.muted)))]
    } else {
        app.local_entries
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
    app.local_list_state.select(if app.local_entries.is_empty() { None } else { Some(app.local_cursor) });
    f.render_stateful_widget(list, area, &mut app.local_list_state);
}

fn draw_remote_pane(f: &mut Frame, app: &mut App, area: Rect, p: &Palette) {
    let focused = app.focus == Focus::Remote;
    let visible = app.visible_entries();
    let title = match &app.filter {
        Some(filt) if !filt.is_empty() => format!(" [2] s3://{}/{} — filter: {filt} ", app.bucket, app.prefix),
        _ => format!(" [2] s3://{}/{} ", app.bucket, app.prefix),
    };

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
    let (title, body, color) = match &app.preview {
        Preview::Empty => (" preview ".to_string(), "(nothing selected)".to_string(), p.muted),
        Preview::Loading => (" preview ".to_string(), format!("{} loading...", theme::spinner(app.spinner_frame)), p.muted),
        Preview::Directory => (" preview ".to_string(), "(directory)".to_string(), p.muted),
        Preview::TooLarge { size } => (
            " preview ".to_string(),
            format!("file too large to preview ({})", format_size(*size, BINARY)),
            p.muted,
        ),
        Preview::Binary { size } => {
            (" preview ".to_string(), format!("binary file, {}", format_size(*size, BINARY)), p.muted)
        }
        Preview::Error(err) => (" preview ".to_string(), format!("preview error: {err}"), p.bad),
        Preview::Text { text, size, truncated } => {
            let title = if *truncated {
                format!(" preview ({}, showing first bytes) ", format_size(*size, BINARY))
            } else {
                format!(" preview ({}) ", format_size(*size, BINARY))
            };
            (title, text.clone(), p.text)
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

fn draw_help(f: &mut Frame, p: &Palette) {
    let area = centered_rect(65, 80, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        "↑/k, ↓/j     move cursor",
        "→/l/enter    open directory",
        "←/h/bksp     go up a directory",
        "space        mark/unmark item",
        "d            download marked/hovered s3 object(s)",
        "r            rename",
        "/            filter the s3 pane",
        "p            toggle the preview pane",
        "L            toggle the local filesystem pane (off by default)",
        "tab          switch focus forward through local / s3 / preview / transfers panes",
        "shift+tab    switch focus backward through the panes",
        "1-4          jump focus directly to local / s3 / preview / transfers",
        "u            upload marked/hovered local file(s) into the s3 pane's directory",
        "             (needs the local pane on — L — or just drag a file onto the window)",
        "o            open bookmark's web_url in your browser",
        "",
        "on the transfers pane (focus it with tab or 4):",
        "↑/k, ↓/j     move between transfers",
        "↵/l          open the transfer's local file/folder with the default app",
        "f            reveal the transfer's local file/folder in Finder",
        "",
        "E            show full error details (after a failure)",
        "t            toggle light/dark theme",
        "c            switch bookmark",
        "q            quit",
        "esc          cancel / clear filter / clear marks",
        "?            toggle this help",
        "",
        "on the bookmark list:",
        "a            add a bookmark",
        "e            edit the selected bookmark",
        "x            delete the selected bookmark",
    ];
    let widget = Paragraph::new(lines.join("\n")).block(
        Block::default()
            .title(" comhad help ")
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
