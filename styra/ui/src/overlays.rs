use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

#[derive(Clone, Copy)]
pub struct BranchPromptView {
    pub selected: usize,
}

pub fn render_branch(frame: &mut Frame, prompt: BranchPromptView, frame_area: Rect) {
    let width = frame_area.width.saturating_sub(4).min(72);
    let height = 4.min(frame_area.height);
    let area = Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" branch from selected entry · Enter choose · q cancel ");
    let list = List::new([
        ListItem::new(Line::from("entire interaction through this entry")),
        ListItem::new(Line::from("only this entry")),
    ])
    .block(block)
    .highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(prompt.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

#[derive(Clone, Copy)]
pub struct TagPickerView<'a> {
    pub available: &'a [String],
    pub selected: &'a [String],
    pub cursor: usize,
    pub new_tag: Option<&'a str>,
}

pub fn render_tags(frame: &mut Frame, picker: TagPickerView<'_>) {
    let area = frame.area();
    let height = (picker.available.len() as u16 + 6).min(area.height.saturating_sub(2));
    let width = area.width.saturating_sub(8).min(56);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" Interaction tags · Space toggle · n new · Enter save · Esc cancel ");
    let inside = block.inner(popup);
    frame.render_widget(block, popup);
    let rows = picker
        .available
        .iter()
        .map(|tag| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    if picker.selected.contains(tag) {
                        " [x] "
                    } else {
                        " [ ] "
                    },
                    Style::default().fg(palette::ACCENT),
                ),
                Span::raw(tag.clone()),
            ]))
        })
        .collect::<Vec<_>>();
    let input = picker
        .new_tag
        .map(|value| format!("new tag: {value} · Enter add & save"))
        .unwrap_or_else(|| "n adds a new tag".into());
    let list_area = Rect::new(
        inside.x,
        inside.y,
        inside.width,
        inside.height.saturating_sub(1),
    );
    let input_area = Rect::new(
        inside.x,
        inside.y + inside.height.saturating_sub(1),
        inside.width,
        1,
    );
    let list = List::new(rows).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select((!picker.available.is_empty()).then_some(picker.cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
    frame.render_widget(
        Paragraph::new(input).style(Style::default().fg(palette::SUBORDINATE_TEXT)),
        input_area,
    );
}

/// A file citation displayed by the references overlay.
#[derive(Clone, Copy)]
pub struct ReferenceView<'a> {
    pub label: &'a str,
    pub line: Option<u32>,
}

/// Presentation data for the file-reference chooser.
#[derive(Clone, Copy)]
pub struct ReferencesView<'a> {
    pub items: &'a [ReferenceView<'a>],
    pub selected: usize,
}

pub fn render_references(frame: &mut Frame, references: ReferencesView<'_>, frame_area: Rect) {
    const CHROME: u16 = 2;
    const MAX_ROWS: u16 = 12;
    let width = frame_area.width.saturating_sub(4).min(90);
    let height = (references.items.len() as u16)
        .min(MAX_ROWS)
        .saturating_add(CHROME)
        .min(frame_area.height);
    let area = Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            " files in this entry · Enter open · q cancel ",
            Style::default().fg(palette::MUTED_TEXT),
        ));
    let items = references.items.iter().map(|reference| {
        let mut spans = vec![Span::styled(
            reference.label,
            Style::default().fg(palette::TEXT),
        )];
        if let Some(line) = reference.line {
            spans.push(Span::styled(
                format!("  line {line}"),
                Style::default().fg(palette::ADDITIONAL_INFO),
            ));
        }
        ListItem::new(Line::from(spans))
    });
    let list = List::new(items.collect::<Vec<_>>())
        .block(block)
        .highlight_style(
            Style::default()
                .bg(palette::SELECTION_BACKGROUND)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(Some(references.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The visible state of the path-insertion prompt.
#[derive(Clone)]
pub enum InsertPromptView<'a> {
    Typing(&'a str),
    Grant(String),
}

pub fn render_insert(frame: &mut Frame, insert: Option<InsertPromptView<'_>>, area: Rect) {
    match insert {
        None => {}
        Some(InsertPromptView::Typing(text)) => render_insert_typing(frame, text, area),
        Some(InsertPromptView::Grant(host)) => render_insert_grant(frame, &host, area),
    }
}

fn insert_floating(area: Rect, height: u16) -> Rect {
    let width = area.width.saturating_sub(4).min(72);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn render_insert_typing(frame: &mut Frame, text: &str, area: Rect) {
    let prompt = insert_floating(area, 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" path · Tab complete · Enter insert · Esc cancel ");
    let inner = block.inner(prompt);
    frame.render_widget(Clear, prompt);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_owned(),
            Style::default().fg(palette::TEXT),
        )))
        .block(block),
        prompt,
    );
    if inner.width > 0 {
        frame.set_cursor_position(ratatui::layout::Position {
            x: inner.x + (text.chars().count() as u16).min(inner.width - 1),
            y: inner.y,
        });
    }
}

fn render_insert_grant(frame: &mut Frame, host: &str, area: Rect) {
    let prompt = insert_floating(area, 5);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::WARNING))
        .title(Span::styled(
            " outside the sandbox ",
            Style::default().fg(palette::WARNING),
        ))
        .title_bottom(Line::from(Span::styled(
            " for this interaction ",
            Style::default().fg(palette::WARNING),
        )));
    let key = |ch| {
        Span::styled(
            ch,
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let muted = Style::default().fg(palette::MUTED_TEXT);
    let lines = vec![
        Line::from(Span::styled(
            host.to_owned(),
            Style::default().fg(palette::TEXT),
        )),
        Line::default(),
        Line::from(vec![
            key("r"),
            Span::styled(" readable  ", muted),
            key("w"),
            Span::styled(" writable  ", muted),
            key("n"),
            Span::styled(" insert without mounting  ", muted),
            key("Esc"),
            Span::styled(" cancel", muted),
        ]),
    ];
    frame.render_widget(Clear, prompt);
    frame.render_widget(Paragraph::new(lines).block(block), prompt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(prompt: InsertPromptView<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal
            .draw(|frame| render_insert(frame, Some(prompt), frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn path_prompt_shows_the_text_being_typed() {
        assert!(rendered(InsertPromptView::Typing("src/main.rs")).contains("src/main.rs"));
    }

    #[test]
    fn grant_prompt_names_the_host_path() {
        assert!(rendered(InsertPromptView::Grant("/host/private".into())).contains("/host/private"));
    }
}
