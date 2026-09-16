//! Drawing.
//!
//! Every widget here reads state and writes nothing, so a rendered frame is a
//! pure function of the application. That is what lets `TestBackend` assert on
//! the drawn buffer without a terminal.

use incurs_app_model::form::FieldKind;
use incurs_app_model::rows::{raw_json, rows_for};
use incurs_app_model::{RunState, SkillState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Focus};
use crate::theme::Theme;

/// Draws one frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let theme = Theme::default();
    let area = frame.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(9),
            Constraint::Length(1),
        ])
        .split(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(20)])
        .split(rows[0]);

    draw_commands(frame, app, theme, panes[0]);
    draw_form(frame, app, theme, panes[1]);
    draw_result(frame, app, theme, rows[1]);
    draw_status(frame, app, theme, rows[2]);

    if app.help {
        draw_help(frame, theme, area);
    }
}

/// Draws the command list.
fn draw_commands(frame: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let visible = app.visible();
    let focused = app.focus == Focus::Commands;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(row, &index)| {
            let command = &app.session.commands()[index];
            let selected = row == app.row;
            let marker = if selected { "▸ " } else { "  " };
            let mut spans = vec![
                Span::styled(marker, theme.accent),
                Span::styled(
                    command.title.clone(),
                    if selected {
                        theme.text.add_modifier(Modifier::BOLD)
                    } else {
                        theme.text
                    },
                ),
            ];
            if command.destructive {
                spans.push(Span::styled("  ⚠", theme.danger));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = if app.searching || !app.search.is_empty() {
        format!(" Search: {} ", app.search.text())
    } else {
        format!(" Commands ({}) ", visible.len())
    };

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(focused))
                .title(title),
        ),
        area,
    );
}

/// Draws the selected command's form.
fn draw_form(frame: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let focused = app.focus == Focus::Form;
    let Some(command) = app.session.selected() else {
        frame.render_widget(
            Paragraph::new("This application has no commands to show.")
                .style(theme.muted)
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border(focused))
        .title(format!(" {} ", command.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(command.description.clone(), theme.muted)),
        Line::from(""),
    ];

    if command.model.fields.is_empty() {
        lines.push(Line::from(Span::styled(
            "This command needs no information.",
            theme.muted,
        )));
    }

    for (index, field) in command.model.fields.iter().enumerate() {
        let active = focused && index == app.field;
        let value = app.session.state().value(&field.name);

        let label = format!("{:<14}", truncate(&field.label, 14));
        let rendered = match &field.kind {
            FieldKind::Toggle => {
                let on = value.map(|value| value.toggle).unwrap_or(false);
                Span::styled(
                    if on {
                        "[x]".to_string()
                    } else {
                        "[ ]".to_string()
                    },
                    theme.text,
                )
            }
            FieldKind::Select { options } => {
                let chosen = value.map(|value| value.choice.clone()).unwrap_or_default();
                Span::styled(
                    format!(
                        "‹ {} ›",
                        options
                            .iter()
                            .map(|option| if *option == chosen {
                                format!("·{option}·")
                            } else {
                                option.clone()
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    theme.text,
                )
            }
            _ => {
                let text = app
                    .editors
                    .get(&field.name)
                    .map(|editor| editor.text().to_string())
                    .unwrap_or_default();
                Span::styled(format!("[{:<22}]", truncate(&text, 22)), theme.text)
            }
        };

        let marker = if active { "▸ " } else { "  " };
        let mut spans = vec![
            Span::styled(marker, theme.accent),
            Span::styled(label, if active { theme.text } else { theme.muted }),
            rendered,
        ];
        if field.required {
            spans.push(Span::styled(" *", theme.accent));
        }
        lines.push(Line::from(spans));

        if let Some(issue) = value.and_then(|value| value.issue.clone()) {
            lines.push(Line::from(Span::styled(
                format!("    {issue}"),
                theme.danger,
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Draws the result of the most recent call.
fn draw_result(frame: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border(false))
        .title(" Result ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = match app.session.run_state() {
        RunState::Idle => vec![Line::from(Span::styled(
            "Press ^R to run the selected command.",
            theme.muted,
        ))],
        RunState::Running { log, .. } => {
            let mut lines = vec![Line::from(Span::styled(app.run_summary(), theme.accent))];
            lines.extend(
                log.iter()
                    .rev()
                    .take(5)
                    .rev()
                    .map(|line| Line::from(Span::styled(line.clone(), theme.muted))),
            );
            lines
        }
        RunState::Succeeded { data, .. } => {
            if app.raw {
                raw_json(data)
                    .lines()
                    .map(|line| Line::from(line.to_string()))
                    .collect()
            } else {
                let rows = rows_for(data);
                if rows.is_empty() {
                    vec![Line::from(Span::styled(
                        "Finished with nothing to show.",
                        theme.muted,
                    ))]
                } else {
                    rows.into_iter()
                        .map(|row| {
                            let indent = "  ".repeat(row.depth);
                            match row.value {
                                Some(value) => Line::from(vec![
                                    Span::styled(
                                        format!("{indent}{:<16}", truncate(&row.label, 16)),
                                        theme.muted,
                                    ),
                                    Span::styled(value, theme.text),
                                ]),
                                None => Line::from(Span::styled(
                                    format!("{indent}{}", row.label),
                                    theme.text.add_modifier(Modifier::BOLD),
                                )),
                            }
                        })
                        .collect()
                }
            }
        }
        RunState::Failed {
            message,
            code,
            details,
        } => {
            let mut lines = vec![Line::from(Span::styled(message.clone(), theme.danger))];
            lines.extend(
                details
                    .iter()
                    .map(|detail| Line::from(Span::styled(detail.clone(), theme.danger))),
            );
            lines.push(Line::from(Span::styled(code.clone(), theme.muted)));
            lines
        }
        RunState::Cancelled => vec![Line::from(Span::styled(
            "Stopped before it finished.",
            theme.muted,
        ))],
    };

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Draws the status bar.
fn draw_status(frame: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let hint = if app.session.is_running() {
        "^C cancel · ? help"
    } else {
        "tab focus · ↑↓ move · ^R run · ^K skills · / search · ? help · esc quit"
    };
    let message = app
        .status
        .clone()
        .or_else(|| match app.session.skill_state() {
            SkillState::Installing => Some("Installing skills…".to_string()),
            _ => None,
        })
        .unwrap_or_else(|| hint.to_string());

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(message, theme.muted))),
        area,
    );
}

/// Draws the help overlay.
fn draw_help(frame: &mut Frame, theme: Theme, area: Rect) {
    let width = 52.min(area.width.saturating_sub(4));
    let height = 14.min(area.height.saturating_sub(2));
    let overlay = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    let lines = vec![
        Line::from("tab / shift-tab   move between the list and the form"),
        Line::from("↑ ↓               move within a pane"),
        Line::from("← →               change a choice"),
        Line::from("space             flip a switch, or change a choice"),
        Line::from("enter             run the selected command"),
        Line::from("^R                run"),
        Line::from("^C                cancel a run, or quit when idle"),
        Line::from("^K                install agent skills"),
        Line::from("/                 search commands"),
        Line::from("esc               clear the search, or quit"),
        Line::from(""),
        Line::from(Span::styled("Press any key to close.", theme.muted)),
    ];

    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(true))
                .title(" Keys "),
        ),
        overlay,
    );
}

/// Shortens a label to fit a fixed column.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Styles used by the terminal surface.
impl Theme {
    /// Returns the border style for a focused or unfocused pane.
    pub fn border(&self, focused: bool) -> Style {
        if focused { self.accent } else { self.muted }
    }
}
