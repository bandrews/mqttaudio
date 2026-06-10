// ABOUTME: Ratatui rendering for the config editor: sidebar, forms, and modals.
// ABOUTME: Pure drawing from App state; no state mutation happens here.

use super::app::{App, Modal, Pane};
use super::fields::Section;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

/// Draw one frame of the editor.
pub fn draw(frame: &mut Frame, app: &App) {
    let [main, help, status, keys] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [sidebar, content] =
        Layout::horizontal([Constraint::Length(20), Constraint::Min(20)]).areas(main);

    draw_sidebar(frame, app, sidebar);
    draw_content(frame, app, content);
    draw_help(frame, app, help);
    draw_status(frame, app, status);
    draw_keybar(frame, app, keys);

    if let Some(modal) = &app.modal {
        draw_modal(frame, app, modal);
    }
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
    let label_width = rows.iter().map(|r| r.label.len()).max().unwrap_or(0).max(8);
    let items: Vec<ListItem> = if rows.is_empty() {
        vec![ListItem::new(Span::styled(
            "(empty — press 'a' to add)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        rows.iter()
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
            .collect()
    };
    let highlight = if app.pane == Pane::Form {
        Style::default().bg(Color::Blue).fg(Color::White)
    } else {
        Style::default()
    };
    let cursor = app
        .levels
        .last()
        .map(|l| l.cursor)
        .unwrap_or(0)
        .min(items.len().saturating_sub(1));
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.breadcrumb()),
        )
        .highlight_style(highlight);
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(edit) = &app.edit {
        let mut spans = vec![Span::styled(
            format!("{}: ", edit.label),
            Style::default().add_modifier(Modifier::BOLD),
        )];
        spans.extend(buffer_with_cursor(&edit.buffer, edit.cursor));
        let mut lines = vec![Line::from(spans)];
        if let Some(err) = &edit.error {
            lines.push(Line::from(Span::styled(
                err.clone(),
                Style::default().fg(Color::Red),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "Enter: apply   Esc: cancel   (empty input unsets optional fields)",
                Style::default().fg(Color::DarkGray),
            )));
        }
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let help = app
        .rows()
        .get(app.levels.last().map(|l| l.cursor).unwrap_or(0))
        .map(|r| r.help.clone())
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(help)
            .style(Style::default().fg(Color::Cyan))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
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
    let text = if app.edit.is_some() {
        "Enter apply  Esc cancel"
    } else if app.modal.is_some() {
        "Esc close"
    } else {
        match app.pane {
            Pane::Sidebar => "↑/↓ section  Enter/→ open  s save  q quit",
            Pane::Form => {
                "↑/↓ move  Enter edit/toggle  Del/u reset  a add  ←/Esc back  s save  q quit"
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

/// A textual level meter: `width` cells, filled proportionally to `level` 0..=1.
fn level_bar(level: f32, width: usize) -> String {
    let filled = (level * width as f32).round() as usize;
    let mut bar = String::with_capacity(width);
    for i in 0..width {
        bar.push(if i < filled.min(width) { '█' } else { '░' });
    }
    bar
}
