use crate::fuzzy_list::{marked, FuzzyList};
use crate::text_prompt::{self, TextPrompt};
use crate::theme;
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
        .border_style(Style::default().fg(theme::ACCENT))
        .title(" branch from selected entry · ? keys ");
    let list = List::new([
        ListItem::new(Line::from("entire interaction through this entry")),
        ListItem::new(Line::from("only this entry")),
    ])
    .block(block)
    .highlight_style(
        Style::default()
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(prompt.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

#[derive(Clone, Copy)]
pub struct LinkMenuView<'a> {
    /// The link the actions apply to, as the reply wrote it.
    pub destination: &'a str,
    pub actions: &'a [&'a str],
    pub selected: usize,
}

pub fn render_link_menu(frame: &mut Frame, menu: LinkMenuView<'_>, frame_area: Rect) {
    let width = frame_area.width.saturating_sub(4).min(72);
    let height = (menu.actions.len() as u16 + 2).min(frame_area.height);
    let area = Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(format!(" {} · ? keys ", menu.destination));
    let list = List::new(
        menu.actions
            .iter()
            .map(|action| ListItem::new(Line::from(*action))),
    )
    .block(block)
    .highlight_style(
        Style::default()
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(menu.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

#[derive(Clone, Copy)]
pub struct TagPickerView<'a> {
    pub available: &'a [String],
    pub selected: &'a [String],
    pub list: &'a FuzzyList,
    pub new_tag: Option<&'a str>,
}

pub fn render_tags(frame: &mut Frame, picker: TagPickerView<'_>) {
    let area = frame.area();
    // Sized to the whole catalog rather than the matches, so the box does not
    // jump about as the filter narrows it.
    let height = (picker.available.len().max(1) as u16 + 6).min(area.height.saturating_sub(2));
    let width = area.width.saturating_sub(8).min(56);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(" Interaction tags · ? keys ");
    if picker.list.is_filtering() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(" /", Style::default().fg(theme::MUTED_TEXT)),
            Span::styled(
                format!("{} ", picker.list.query),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    let inside = block.inner(popup);
    frame.render_widget(block, popup);
    let matches = picker.list.matches(picker.available);
    let rows = if matches.is_empty() {
        let note = if picker.available.is_empty() {
            "no tags yet".to_owned()
        } else {
            format!("no match for {}", picker.list.query)
        };
        vec![ListItem::new(Line::from(Span::styled(
            format!("  {note}"),
            Style::default()
                .fg(theme::MUTED_TEXT)
                .add_modifier(Modifier::DIM),
        )))]
    } else {
        matches
            .iter()
            .map(|found| {
                let tag = &picker.available[found.index];
                let mut spans = vec![Span::styled(
                    if picker.selected.contains(tag) {
                        " [x] "
                    } else {
                        " [ ] "
                    },
                    Style::default().fg(theme::ACCENT),
                )];
                spans.extend(marked(tag, &found.positions));
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>()
    };
    let input = picker
        .new_tag
        .map(|value| format!("new tag: {value} · Enter add & save"))
        .unwrap_or_else(|| "type to filter · ctrl-n adds a new tag".into());
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
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select((!matches.is_empty()).then(|| picker.list.selected.min(matches.len() - 1)));
    frame.render_stateful_widget(list, list_area, &mut state);
    frame.render_widget(
        Paragraph::new(input).style(Style::default().fg(theme::SUBORDINATE_TEXT)),
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
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::styled(
            " files in this entry · ? keys ",
            Style::default().fg(theme::MUTED_TEXT),
        ));
    let items = references.items.iter().map(|reference| {
        let mut spans = vec![Span::styled(
            reference.label,
            Style::default().fg(theme::TEXT),
        )];
        if let Some(line) = reference.line {
            spans.push(Span::styled(
                format!("  line {line}"),
                Style::default().fg(theme::ADDITIONAL_INFO),
            ));
        }
        ListItem::new(Line::from(spans))
    });
    let list = List::new(items.collect::<Vec<_>>())
        .block(block)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTION_BACKGROUND)
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
    /// The host path no mount carries, and whether granting it restarts the
    /// interaction — true for an idle one, whose sandbox has to be relaunched
    /// to take a new mount.
    Grant {
        host: String,
        restarts: bool,
    },
}

pub fn render_insert(frame: &mut Frame, insert: Option<InsertPromptView<'_>>, area: Rect) {
    match insert {
        None => {}
        Some(InsertPromptView::Typing(text)) => render_insert_typing(frame, text, area),
        Some(InsertPromptView::Grant { host, restarts }) => {
            render_insert_grant(frame, &host, restarts, area)
        }
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
    let prompt = TextPrompt::new(" path · Tab complete · Enter insert · Esc cancel ", text);
    text_prompt::render(frame, &prompt, area);
}

fn render_insert_grant(frame: &mut Frame, host: &str, restarts: bool, area: Rect) {
    let prompt = insert_floating(area, if restarts { 6 } else { 5 });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::WARNING))
        .title(Span::styled(
            " outside the sandbox ",
            Style::default().fg(theme::WARNING),
        ))
        .title_bottom(Line::from(Span::styled(
            " for this interaction ",
            Style::default().fg(theme::WARNING),
        )));
    let key = |ch| {
        Span::styled(
            ch,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let muted = Style::default().fg(theme::MUTED_TEXT);
    let mut lines = vec![
        Line::from(Span::styled(
            host.to_owned(),
            Style::default().fg(theme::TEXT),
        )),
        Line::default(),
    ];
    // Said before the answer rather than after it: mounting is what restarts
    // the agent, and `n` is the way to avoid that.
    if restarts {
        lines.push(Line::from(Span::styled(
            "mounting restarts the interaction; the conversation resumes",
            Style::default().fg(theme::WARNING),
        )));
    }
    lines.push(Line::from(vec![
        key("r"),
        Span::styled(" readable  ", muted),
        key("w"),
        Span::styled(" writable  ", muted),
        key("n"),
        Span::styled(" insert without mounting  ", muted),
        key("Esc"),
        Span::styled(" cancel", muted),
    ]));
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
        let screen = rendered(InsertPromptView::Grant {
            host: "/host/private".into(),
            restarts: false,
        });
        assert!(screen.contains("/host/private"));
        assert!(!screen.contains("restarts"));
    }

    #[test]
    fn grant_prompt_warns_when_mounting_restarts_the_interaction() {
        let screen = rendered(InsertPromptView::Grant {
            host: "/host/private".into(),
            restarts: true,
        });
        assert!(screen.contains("mounting restarts the interaction"));
    }
}
