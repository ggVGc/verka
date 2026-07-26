use crate::app::{App, Overlay, View};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
    Frame,
};

const ACCENT: Color = Color::Cyan;
const ERROR: Color = Color::LightRed;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if app.linka_active {
        if let Some(linka) = &app.linka {
            draw_linka(frame, linka, area);
            return;
        }
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);

    let titles: Vec<Line<'_>> = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| {
            let count = app.rows[*view as usize].len();
            Line::from(format!(" {} {} ({count}) ", index + 1, view.label()))
        })
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.view as usize)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Orka · {} ", app.root.display())),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, chunks[0]);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
        .split(chunks[1]);

    let items = app
        .rows()
        .iter()
        .map(|row| {
            ListItem::new(vec![
                Line::styled(
                    row.id.clone(),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Line::from(row.summary.clone()),
            ])
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", app.view.label())),
        )
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut state = ListState::default()
        .with_selected((!app.rows().is_empty()).then_some(app.selected.min(app.rows().len() - 1)));
    frame.render_stateful_widget(list, main[0], &mut state);

    let detail = app
        .selected_row()
        .map(|row| row.detail.as_str())
        .unwrap_or("No items in this view.");
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title(" Details "))
            .wrap(Wrap { trim: false }),
        main[1],
    );

    let status_style = if app.status.to_ascii_lowercase().contains("error")
        || app.status.to_ascii_lowercase().contains("failed")
    {
        Style::default().fg(ERROR)
    } else if app.busy {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if app.busy { " BUSY " } else { " READY " },
                Style::default()
                    .fg(Color::Black)
                    .bg(if app.busy {
                        Color::Yellow
                    } else {
                        Color::Green
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {}  ·  a actions  l live output  L linka  Enter inspect  ←/→ views  r refresh  ? help  q quit",
                app.status
            )),
        ]))
        .style(status_style),
        chunks[2],
    );

    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, overlay);
    }
}

fn draw_linka(frame: &mut Frame, linka: &linka_tui::app::App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)])
        .split(area);
    linka_tui::ui::draw_in(frame, linka, chunks[0]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " LINKA ",
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" embedded in Orka  ·  L or q returns to Orka"),
        ])),
        chunks[1],
    );
}

fn draw_overlay(frame: &mut Frame, overlay: &Overlay) {
    let area = centered(frame.area(), 82, 78);
    frame.render_widget(Clear, area);
    match overlay {
        Overlay::Actions { actions, selected } => {
            let items = actions
                .iter()
                .map(|action| ListItem::new(action.label()))
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Actions "))
                .highlight_symbol("▶ ")
                .highlight_style(Style::default().fg(Color::Black).bg(ACCENT));
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, area, &mut state);
        }
        Overlay::Text {
            title,
            body,
            scroll,
        } => {
            let error = title.starts_with("ERROR");
            frame.render_widget(
                Paragraph::new(body.as_str())
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(if error { ERROR } else { ACCENT }))
                            .title(format!(" {title} · Esc close · j/k scroll ")),
                    )
                    .scroll((*scroll, 0))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Overlay::Live {
            attempt,
            body,
            scroll,
        } => {
            let viewport_height = area.height.saturating_sub(2) as usize;
            let viewport_width = area.width.saturating_sub(2) as usize;
            let tail = wrapped_line_count(body, viewport_width)
                .saturating_sub(viewport_height)
                .min(u16::MAX as usize) as u16;
            let offset = scroll.unwrap_or(tail);
            frame.render_widget(
                Paragraph::new(body.as_str())
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(Color::Green))
                            .title(format!(
                                " Live output · {attempt} · {} · Esc close · End follow ",
                                if scroll.is_none() {
                                    "FOLLOWING"
                                } else {
                                    "PAUSED"
                                }
                            )),
                    )
                    .scroll((offset, 0))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Overlay::Confirm { action, target } => {
            let box_area = centered(frame.area(), 62, 28);
            frame.render_widget(Clear, box_area);
            frame.render_widget(
                Paragraph::new(format!(
                    "{}\n\nTarget: {}\n\nPress y/Enter to continue, n/Esc to cancel.",
                    action.label(),
                    if target.is_empty() {
                        "(global)"
                    } else {
                        target
                    }
                ))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow))
                        .title(" Confirm "),
                )
                .wrap(Wrap { trim: false }),
                box_area,
            );
        }
        Overlay::Form(form) => {
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    form.fields
                        .iter()
                        .map(|_| Constraint::Length(4))
                        .chain(std::iter::once(Constraint::Min(2)))
                        .collect::<Vec<_>>(),
                )
                .split(area);
            frame.render_widget(
                Block::default().borders(Borders::ALL).title(format!(
                    " {} · {} ",
                    form.action.label(),
                    form.target
                )),
                area,
            );
            for (index, field) in form.fields.iter().enumerate() {
                let style = if index == form.selected {
                    Style::default().fg(ACCENT)
                } else {
                    Style::default()
                };
                frame.render_widget(
                    Paragraph::new(field.value.as_str()).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(style)
                            .title(format!(" {} ({}) ", field.label, field.hint)),
                    ),
                    inner[index],
                );
            }
            let footer = form.error.as_deref().unwrap_or(
                "Tab/Enter next field · Enter on last field or Ctrl+Enter submit · Esc cancel",
            );
            frame.render_widget(
                Paragraph::new(footer).style(if form.error.is_some() {
                    Style::default().fg(ERROR)
                } else {
                    Style::default()
                }),
                inner[form.fields.len()],
            );
        }
        Overlay::Help => {
            frame.render_widget(
                Paragraph::new(
                    "ORKA TUI\n\n\
                     Navigate\n\
                       1–7 / ← →     switch state view\n\
                       j k / ↑ ↓     select item\n\
                       g / G         first / last item\n\
                       Enter         inspect selected item\n\
                       l             follow active/selected attempt output\n\
                       r             reload all state\n\n\
                     Linka\n\
                       L             switch to the embedded Linka TUI\n\
                       L / q         (while in Linka) switch back to Orka\n\n\
                     Actions\n\
                       a             context-sensitive action palette\n\
                       Tab           next form field\n\
                       Enter         next field / submit last field\n\
                       Ctrl+Enter    submit form immediately\n\n\
                     Text views\n\
                       j k / ↑ ↓     scroll\n\
                       PgUp/PgDn     scroll by page\n\
                       End / G       resume following live output\n\
                       Esc / q       close overlay\n\n\
                     All popup and refresh errors remain available in the Errors view for the rest of the session.",
                )
                .block(Block::default().borders(Borders::ALL).title(" Help · Esc close "))
                .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn wrapped_line_count(body: &str, width: usize) -> usize {
    if width == 0 {
        return body.lines().count().max(1);
    }
    body.lines()
        .map(|line| Line::from(line).width().max(1).div_ceil(width))
        .sum::<usize>()
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_tail_accounts_for_wrapped_output() {
        assert_eq!(wrapped_line_count("123456\nx", 3), 3);
        assert_eq!(wrapped_line_count("", 80), 1);
    }
}
