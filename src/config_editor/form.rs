// ABOUTME: Ratatui rendering for the config editor: sidebar, forms, and modals.
// ABOUTME: Pure drawing from App state; no state mutation happens here.

use super::app::{App, LevelKind, Modal, Pane};
use super::fields::{FieldKind, Section};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

/// Draw one frame of the editor.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let (help_title, help_text) = help_content(app);
    // The floor reserves room for several lines of help even when the current
    // text is short, so moving between fields rarely re-lays-out the screen.
    let help_height =
        (wrapped_line_count(&help_text, area.width.saturating_sub(2)) as u16).clamp(5, 7) + 2;
    let [help, main, status, keys] = Layout::vertical([
        Constraint::Length(help_height),
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let [sidebar, content] =
        Layout::horizontal([Constraint::Length(20), Constraint::Min(20)]).areas(main);

    draw_help(frame, &help_title, &help_text, help);
    draw_sidebar(frame, app, sidebar);
    draw_content(frame, app, content);
    draw_status(frame, app, status);
    draw_keybar(frame, app, keys);

    if let Some(modal) = &app.modal {
        draw_modal(frame, app, modal);
    }
}

/// Title and text for the help pane: the focused field's help, the section
/// intro when the sidebar has focus, or the level intro as a fallback.
fn help_content(app: &App) -> (String, String) {
    if app.pane == Pane::Sidebar {
        let section = app.current_section();
        return (
            format!("Help — {}", section.title()),
            section.intro().to_string(),
        );
    }
    let level = app.levels.last().expect("levels is never empty");
    match app.rows().get(level.cursor) {
        Some(row) => (format!("Help — {}", row.label), row.help.clone()),
        None => (format!("Help — {}", level.title), level.intro.clone()),
    }
}

/// Lines `text` occupies when word-wrapped to `width` columns (greedy wrap,
/// matching ratatui's trimming behavior closely enough to size the pane).
fn wrapped_line_count(text: &str, width: u16) -> usize {
    if width == 0 {
        return 1;
    }
    let width = width as usize;
    let mut lines = 0;
    for raw in text.split('\n') {
        let mut len = 0usize;
        let mut words = 0usize;
        for word in raw.split_whitespace() {
            let wlen = word.chars().count();
            if words > 0 && len + 1 + wlen > width {
                lines += 1;
                len = wlen;
                words = 1;
            } else {
                len += if words > 0 { 1 + wlen } else { wlen };
                words += 1;
            }
        }
        lines += 1;
    }
    lines.max(1)
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = Section::ALL
        .iter()
        .map(|s| ListItem::new(s.title()))
        .collect();
    let highlight = if app.pane == Pane::Sidebar {
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("mqttaudio"))
        .highlight_style(highlight);
    let mut state = ListState::default().with_selected(Some(app.section_idx));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_content(frame: &mut Frame, app: &App, area: Rect) {
    let rows = app.rows();
    let level = app.levels.last().expect("levels is never empty");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(app.breadcrumb());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Collection levels keep their feature explainer visible above the rows.
    let mut list_area = inner;
    let show_intro = !matches!(level.kind, LevelKind::Section(_)) && !level.intro.is_empty();
    if show_intro && inner.height > 5 {
        let intro_height = (wrapped_line_count(&level.intro, inner.width) as u16).min(4);
        let [intro_area, _, rest] = Layout::vertical([
            Constraint::Length(intro_height),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(level.intro.clone())
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
            intro_area,
        );
        list_area = rest;
    }

    if rows.is_empty() {
        if app.level_is_collection() {
            let noun = match &level.kind {
                LevelKind::StructList { meta } => meta.item_noun,
                LevelKind::MacroForm => "parameter",
                LevelKind::Map {
                    value_kind: FieldKind::MapToJson,
                } => "macro",
                _ => "entry",
            };
            let mut lines = vec![Line::from(format!(
                "Nothing here yet. Press 'a' to add your first {}.",
                noun
            ))];
            if matches!(
                level.kind,
                LevelKind::Map {
                    value_kind: FieldKind::MapToJson,
                }
            ) {
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "Example: a macro \"quiet\" with volume 0.1 lets any sender add \
                     \"macro\": \"quiet\" to a command instead of repeating the parameters.",
                ));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                list_area,
            );
        }
        return;
    }

    let label_width = rows.iter().map(|r| r.label.len()).max().unwrap_or(0).max(8);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let value_style = if row.is_set {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let mut spans = vec![
                Span::raw(format!("{:width$}  ", row.label, width = label_width)),
                Span::styled(row.display.clone(), value_style),
            ];
            if !row.is_set {
                spans.push(Span::styled(
                    "  (default)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let highlight = if app.pane == Pane::Form {
        Style::default().bg(Color::Blue).fg(Color::White)
    } else {
        Style::default()
    };
    let cursor = level.cursor.min(items.len().saturating_sub(1));
    let list = List::new(items).highlight_style(highlight);
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn draw_help(frame: &mut Frame, title: &str, text: &str, area: Rect) {
    frame.render_widget(
        Paragraph::new(text.to_string())
            .style(Style::default().fg(Color::Cyan))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(
                        format!(" {} ", title),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )),
            ),
        area,
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    // While a text edit is open, the status line is the input prompt.
    if let Some(edit) = &app.edit {
        let mut spans = vec![Span::styled(
            format!("{}: ", edit.label),
            Style::default().add_modifier(Modifier::BOLD),
        )];
        spans.extend(buffer_with_cursor(&edit.buffer, edit.cursor));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let mut spans = Vec::new();
    if app.dirty {
        spans.push(Span::styled(
            "[modified] ",
            Style::default().fg(Color::Magenta),
        ));
    }
    spans.push(Span::raw(app.status.clone()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_keybar(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(edit) = &app.edit {
        let (text, style) = match &edit.error {
            Some(err) => (err.clone(), Style::default().fg(Color::Red)),
            None => (
                "Enter: apply   Esc: cancel   (empty input unsets optional fields)".to_string(),
                Style::default().fg(Color::DarkGray),
            ),
        };
        frame.render_widget(Paragraph::new(text).style(style), area);
        return;
    }
    let text = if app.modal.is_some() {
        "Esc close"
    } else {
        match app.pane {
            Pane::Sidebar => "↑/↓ section  Enter/→ open  s save  q quit",
            Pane::Form => {
                if app.level_is_collection() {
                    "↑/↓ move  Enter open/edit  a add  d/Del remove  ←/Esc back  s save  q quit"
                } else {
                    "↑/↓ move  Enter edit/toggle  Del/u reset  ←/Esc back  s save  q quit"
                }
            }
        }
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

/// Split an edit buffer into spans with a visible cursor block.
fn buffer_with_cursor(buffer: &str, cursor: usize) -> Vec<Span<'static>> {
    let byte = buffer
        .char_indices()
        .nth(cursor)
        .map(|(i, _)| i)
        .unwrap_or(buffer.len());
    let (before, rest) = buffer.split_at(byte);
    let mut chars = rest.chars();
    let at = chars.next();
    let after: String = chars.collect();
    let mut spans = vec![Span::raw(before.to_string())];
    spans.push(Span::styled(
        at.map(|c| c.to_string()).unwrap_or_else(|| " ".to_string()),
        Style::default().bg(Color::White).fg(Color::Black),
    ));
    spans.push(Span::raw(after));
    spans
}

/// A centered rectangle of at most `width` x `height` inside `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_modal(frame: &mut Frame, _app: &App, modal: &Modal) {
    let area = frame.area();
    match modal {
        Modal::DevicePicker {
            output,
            items,
            cursor,
            ..
        } => {
            let rect = centered(area, 70, (items.len() as u16 + 4).min(area.height));
            frame.render_widget(Clear, rect);
            let title = if *output {
                "Select output device — Enter: choose  t: test  Esc: cancel"
            } else {
                "Select input device — Enter: choose  t: test  Esc: cancel"
            };
            let list_items: Vec<ListItem> = items
                .iter()
                .map(|item| {
                    let name = item.name.as_deref().unwrap_or("(default)");
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            name.to_string(),
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                        Line::from(Span::styled(
                            format!("  {}", item.detail),
                            Style::default().fg(Color::DarkGray),
                        )),
                    ])
                })
                .collect();
            let list = List::new(list_items)
                .block(Block::default().borders(Borders::ALL).title(title))
                .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
            let mut state = ListState::default().with_selected(Some(*cursor));
            frame.render_stateful_widget(list, rect, &mut state);
        }
        Modal::ChoicePicker {
            title,
            hint,
            items,
            cursor,
            ..
        } => {
            let width = 70u16.min(area.width);
            let hint_rows = if hint.is_empty() {
                0
            } else {
                (wrapped_line_count(hint, width.saturating_sub(2)) as u16).min(2)
            };
            let rect = centered(
                area,
                width,
                (items.len() as u16 + 2 + hint_rows).min(area.height),
            );
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!("{} — Enter: choose  Esc: cancel", title));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            let list_area = Rect {
                height: inner.height.saturating_sub(hint_rows),
                ..inner
            };
            let list_items: Vec<ListItem> = items
                .iter()
                .map(|c| {
                    let mut spans = vec![Span::raw(c.label.clone())];
                    if !c.detail.is_empty() {
                        let room =
                            (inner.width as usize).saturating_sub(c.label.chars().count() + 3);
                        spans.push(Span::styled(
                            format!("  {}", ellipsize(&c.detail, room)),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let list = List::new(list_items)
                .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
            let mut state = ListState::default().with_selected(Some(*cursor));
            frame.render_stateful_widget(list, list_area, &mut state);
            if hint_rows > 0 {
                let hint_area = Rect {
                    y: inner.y + inner.height.saturating_sub(hint_rows),
                    height: hint_rows,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(hint.clone())
                        .style(Style::default().fg(Color::DarkGray))
                        .wrap(Wrap { trim: true }),
                    hint_area,
                );
            }
        }
        Modal::OutputTest(state) => {
            let channels = state.session.as_ref().map(|s| s.channels).unwrap_or(0);
            let rect = centered(area, 64, (channels as u16 + 7).min(area.height));
            frame.render_widget(Clear, rect);
            let mut lines = Vec::new();
            lines.push(Line::from(format!("Device: {}", state.device_label)));
            if let Some(session) = &state.session {
                lines.push(Line::from(format!("Negotiated: {}", session.description)));
                lines.push(Line::from(Span::styled(
                    "↑/↓ channel  Enter/Space play  b bass tone  a sweep all  Esc close",
                    Style::default().fg(Color::DarkGray),
                )));
                let peaks = session.channel_peaks();
                for ch in 0..session.channels {
                    let alias = state
                        .aliases
                        .get(ch)
                        .and_then(|a| a.clone())
                        .map(|a| format!(" ({})", a))
                        .unwrap_or_default();
                    let peak = peaks.get(ch).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                    let marker = if ch == state.cursor { ">" } else { " " };
                    lines.push(Line::from(vec![
                        Span::raw(format!("{} ch {:>2}{:<12} ", marker, ch, alias)),
                        Span::styled(
                            level_bar(peak, 24),
                            Style::default().fg(if peak > 0.9 { Color::Red } else { Color::Green }),
                        ),
                    ]));
                }
            }
            if let Some(err) = &state.error {
                lines.push(Line::from(Span::styled(
                    err.clone(),
                    Style::default().fg(Color::Red),
                )));
            }
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Test output device"),
                ),
                rect,
            );
        }
        Modal::InputTest(state) => {
            let channels = state.session.as_ref().map(|s| s.channels).unwrap_or(0);
            let rect = centered(area, 64, (channels as u16 + 6).min(area.height));
            frame.render_widget(Clear, rect);
            let mut lines = Vec::new();
            lines.push(Line::from(format!("Device: {}", state.device_label)));
            lines.push(Line::from(Span::styled(
                "Speak into the microphone — Esc close",
                Style::default().fg(Color::DarkGray),
            )));
            if let Some(session) = &state.session {
                for (ch, peak) in session.peaks().iter().enumerate() {
                    let peak = peak.clamp(0.0, 1.0);
                    lines.push(Line::from(vec![
                        Span::raw(format!("  ch {:>2} ", ch)),
                        Span::styled(
                            level_bar(peak, 32),
                            Style::default().fg(if peak > 0.9 { Color::Red } else { Color::Green }),
                        ),
                    ]));
                }
            }
            if let Some(err) = &state.error {
                lines.push(Line::from(Span::styled(
                    err.clone(),
                    Style::default().fg(Color::Red),
                )));
            }
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Test input device"),
                ),
                rect,
            );
        }
        Modal::SaveDialog { buffer, cursor, .. } => {
            let rect = centered(area, 70, 6);
            frame.render_widget(Clear, rect);
            let mut lines = Vec::new();
            let mut spans = vec![Span::raw("Path: ")];
            spans.extend(buffer_with_cursor(buffer, *cursor));
            lines.push(Line::from(spans));
            lines.push(Line::from(Span::styled(
                "↑/↓ suggested paths  Enter: save  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Save configuration"),
                ),
                rect,
            );
        }
        Modal::Errors { title, errors } => {
            let rect = centered(area, 76, (errors.len() as u16 + 4).min(area.height));
            frame.render_widget(Clear, rect);
            let mut lines: Vec<Line> = errors
                .iter()
                .map(|e| {
                    Line::from(Span::styled(
                        format!("• {}", e),
                        Style::default().fg(Color::Red),
                    ))
                })
                .collect();
            lines.push(Line::from(Span::styled(
                "press any key to continue",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .block(Block::default().borders(Borders::ALL).title(title.clone())),
                rect,
            );
        }
        Modal::ConfirmQuit => {
            let rect = centered(area, 50, 3);
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new("Unsaved changes — quit anyway? (y/n)")
                    .block(Block::default().borders(Borders::ALL).title("Confirm quit")),
                rect,
            );
        }
        Modal::LoadFailed { path, error } => {
            let rect = centered(area, 76, 7);
            frame.render_widget(Clear, rect);
            let lines = vec![
                Line::from(format!("Could not parse {}:", path.display())),
                Line::from(Span::styled(error.clone(), Style::default().fg(Color::Red))),
                Line::from(""),
                Line::from("d: start from defaults (file untouched until saved)   q: quit"),
            ];
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: true }).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Config load failed"),
                ),
                rect,
            );
        }
    }
}

/// Truncate `text` to at most `max` characters, ending with an ellipsis.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// A textual level meter: `width` cells, filled proportionally to `level` 0..=1.
fn level_bar(level: f32, width: usize) -> String {
    let filled = (level * width as f32).round() as usize;
    let mut bar = String::with_capacity(width);
    for i in 0..width {
        bar.push(if i < filled.min(width) { '█' } else { '░' });
    }
    bar
}
