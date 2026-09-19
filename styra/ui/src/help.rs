use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub enum HelpRow<'a> {
    Section(&'a str),
    Binding { keys: &'a str, action: &'a str },
    Blank,
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    rows: &[HelpRow<'_>],
    close_key: &str,
    scroll: u16,
) -> u16 {
    let heading = Style::default()
        .fg(palette::ACCENT)
        .add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(palette::WARNING)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(palette::MUTED_TEXT);
    let mut lines = rows
        .iter()
        .map(|row| match row {
            HelpRow::Section(name) => Line::from(Span::styled((*name).to_owned(), heading)),
            HelpRow::Binding { keys, action } => Line::from(vec![
                Span::styled(format!("  {keys:<20}"), key),
                Span::raw((*action).to_owned()),
            ]),
            HelpRow::Blank => Line::default(),
        })
        .collect::<Vec<_>>();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(" j/k to scroll · {close_key} to close "),
        muted,
    )));
    let visible = area.height.saturating_sub(2);
    let limit = (rows.len() as u16 + 2).saturating_sub(visible);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" styra · keybinds ");
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(limit), 0)),
        area,
    );
    limit
}
