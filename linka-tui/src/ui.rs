use crate::app::{
    candidate_state_label, state_label, App, Focus, NodeKind, Overlay, View, ACTIONS,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Tabs, Wrap,
    },
    Frame,
};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const ERROR: Color = Color::LightRed;

pub fn draw(frame: &mut Frame, app: &App) {
    draw_in(frame, app, frame.area());
}

/// Draw the Linka interface inside `area` instead of the whole frame, so that
/// a host application (such as orka-tui) can embed it alongside its own chrome.
pub fn draw_in(frame: &mut Frame, app: &App, area: Rect) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);

    draw_tabs(frame, app, root[0]);
    draw_body(frame, app, root[1]);
    draw_footer(frame, app, root[2]);

    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, overlay);
    }
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let selected = View::ALL
        .iter()
        .position(|view| *view == app.view)
        .unwrap_or(0);
    let titles = View::ALL
        .iter()
        .map(|view| {
            let suffix = if *view == View::Errors && !app.errors.is_empty() {
                format!(" ({})", app.errors.len())
            } else {
                String::new()
            };
            Line::from(format!(" {}{} ", view.label(), suffix))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Linka ")
                    .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
        area,
    );
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    if app.view == View::Errors {
        draw_errors(frame, app, area);
        return;
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
        .split(area);
    draw_items(frame, app, columns[0]);
    draw_detail(frame, app, columns[1]);
}

fn draw_items(frame: &mut Frame, app: &App, area: Rect) {
    let items = if app.view == View::Candidates {
        app.candidates
            .iter()
            .map(|candidate| {
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            candidate_state_label(candidate),
                            status_style(candidate_state_label(candidate)),
                        ),
                        Span::raw("  "),
                        Span::styled(candidate.record.id.to_string(), Style::default().fg(ACCENT)),
                    ]),
                    Line::styled(
                        format!(
                            "  {}  {} → {}",
                            candidate.record.node, candidate.record.branch, candidate.record.target
                        ),
                        Style::default().fg(MUTED),
                    ),
                ])
            })
            .collect::<Vec<_>>()
    } else {
        app.visible_node_indices()
            .iter()
            .map(|index| {
                let node = &app.nodes[*index];
                let state = state_label(&node.state);
                let kind = node.kind();
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(kind.glyph(), kind_style(kind)),
                        Span::raw(" "),
                        Span::styled(state.clone(), status_style(&state)),
                        Span::raw("  "),
                        Span::styled(&node.id, Style::default().fg(ACCENT)),
                    ]),
                    Line::styled(
                        format!("  {}", node.title),
                        Style::default().fg(Color::White),
                    ),
                ])
            })
            .collect::<Vec<_>>()
    };
    let mut state = ListState::default().with_selected(Some(app.selected));
    let border = if app.focus == Focus::Items {
        ACCENT
    } else {
        MUTED
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border))
                    .title(format!(" {} · {} ", app.view.label(), app.item_count())),
            )
            .highlight_symbol("▸ ")
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(35, 45, 55))
                    .add_modifier(Modifier::BOLD),
            ),
        area,
        &mut state,
    );
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
        .split(area);
    let content = if let Some(node) = app.selected_node() {
        let mut lines = vec![
            kv("id", &node.id),
            kv(
                "kind",
                format!("{} {}", node.kind().glyph(), node.kind().label()),
            ),
            kv("status", state_label(&node.state)),
            kv("author", node.meta.author.as_str()),
            kv(
                "assignee",
                node.meta.assignee.map(AuthorLabel::label).unwrap_or("any"),
            ),
            kv(
                "outcome",
                format!("{:?}", node.state.outcome).to_lowercase(),
            ),
            kv(
                "currency",
                format!("{:?}", node.state.currency).to_lowercase(),
            ),
            kv(
                "integration",
                format!("{:?}", node.state.integration).to_lowercase(),
            ),
        ];
        if let Some(output) = &node.output {
            lines.push(kv("output", output));
        }
        if !node.state.blockers.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled("Blockers", heading()));
            for blocker in &node.state.blockers {
                lines.push(Line::raw(format!(
                    "  {} · {:?}",
                    blocker.id, blocker.reason
                )));
            }
        }
        if !node.state.staleness.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled("Staleness", heading()));
            for reason in &node.state.staleness {
                lines.push(Line::raw(format!("  {reason:?}")));
            }
        }
        if !node.attachments.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("Attachments", heading()),
                Span::styled("  (A to browse)", Style::default().fg(MUTED)),
            ]));
            for item in &node.attachments {
                lines.push(Line::raw(format!(
                    "  {}/{} · {} bytes{}",
                    item.namespace,
                    item.key,
                    item.size,
                    item.media_type
                        .as_ref()
                        .map(|media| format!(" · {media}"))
                        .unwrap_or_default()
                )));
            }
        }
        if !node.notes.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled("Result notes", heading()));
            lines.extend(
                node.notes
                    .lines()
                    .map(|line| Line::raw(format!("  {line}"))),
            );
        }
        lines
    } else if let Some(candidate) = app.selected_candidate() {
        let record = &candidate.record;
        let mut lines = vec![
            kv("id", record.id.to_string()),
            kv("status", candidate_state_label(candidate)),
            kv("source", record.node.to_string()),
            kv("branch", &record.branch),
            kv("target", &record.target),
            kv("artifact", &record.artifact.id),
        ];
        if let Some(external) = &record.external {
            lines.push(kv(
                "external",
                format!("{}/{}", external.namespace, external.id),
            ));
        }
        match &record.state {
            linka::CandidateState::Pending => {}
            linka::CandidateState::Accepted {
                author,
                notes,
                verification,
                ..
            } => {
                lines.push(kv("decision", format!("accepted by {}", author.as_str())));
                lines.push(kv("verification", verification.to_string()));
                if !notes.is_empty() {
                    lines.push(kv("notes", notes));
                }
            }
            linka::CandidateState::Rejected {
                author,
                notes,
                verification,
                ..
            } => {
                lines.push(kv("decision", format!("rejected by {}", author.as_str())));
                lines.push(kv("verification", verification.to_string()));
                lines.push(kv("notes", notes));
            }
        }
        lines
    } else {
        vec![Line::styled(
            "No items in this view.",
            Style::default().fg(MUTED),
        )]
    };
    frame.render_widget(
        Paragraph::new(content)
            .block(Block::default().borders(Borders::ALL).title(" Details "))
            .wrap(Wrap { trim: false }),
        rows[0],
    );

    let associations = app.associations();
    let items = associations
        .iter()
        .map(|association| ListItem::new(association.label.clone()))
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.association_selected));
    let border = if app.focus == Focus::Associations {
        ACCENT
    } else {
        MUTED
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border))
                    .title(" Associated · Tab then Enter to follow "),
            )
            .highlight_symbol("→ ")
            .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        rows[1],
        &mut state,
    );
}

fn draw_errors(frame: &mut Frame, app: &App, area: Rect) {
    let items = if app.errors.is_empty() {
        vec![ListItem::new("No errors detected.")]
    } else {
        app.errors
            .iter()
            .map(|error| ListItem::new(error.clone()).style(Style::default().fg(ERROR)))
            .collect()
    };
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if app.errors.is_empty() {
                        Color::Green
                    } else {
                        ERROR
                    }))
                    .title(" Evaluation errors "),
            )
            .highlight_symbol("▸ "),
        area,
        &mut state,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let footer = Line::from(vec![
        Span::styled(" ←/→ ", key()),
        Span::raw("views  "),
        Span::styled("j/k ", key()),
        Span::raw("move  "),
        Span::styled("Tab/Enter ", key()),
        Span::raw("follow  "),
        Span::styled("a ", key()),
        Span::raw("actions  "),
        Span::styled("r ", key()),
        Span::raw("refresh  "),
        Span::styled("? ", key()),
        Span::raw("help  "),
        Span::styled("q ", key()),
        Span::raw("quit  "),
        Span::styled(&app.status, Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(footer).alignment(Alignment::Left), area);
}

fn draw_overlay(frame: &mut Frame, overlay: &Overlay) {
    match overlay {
        Overlay::Help => {
            let area = centered(72, 84, frame.area());
            frame.render_widget(Clear, area);
            let help = Text::from(vec![
                Line::styled("Navigation", heading()),
                Line::raw("←/→ or h/l     change collection"),
                Line::raw("↑/↓ or j/k     move selection"),
                Line::raw("Tab            switch between items and associations"),
                Line::raw("Enter          focus/follow an association"),
                Line::raw("b/Backspace    go back after following"),
                Line::raw("r              reload graph and derived state"),
                Line::raw(""),
                Line::styled("Node kinds", heading()),
                Line::from(vec![
                    Span::styled(NodeKind::Work.glyph(), kind_style(NodeKind::Work)),
                    Span::raw("              work node: produces its own output"),
                ]),
                Line::from(vec![
                    Span::styled(
                        NodeKind::Verification.glyph(),
                        kind_style(NodeKind::Verification),
                    ),
                    Span::raw("              verification node: reviews one candidate"),
                ]),
                Line::raw(""),
                Line::styled("Attachments", heading()),
                Line::raw("A              browse the selected node's attachments"),
                Line::raw("j/k            select an attachment"),
                Line::raw("J/K or PgUp/Dn scroll the payload"),
                Line::raw("Esc            close the browser"),
                Line::raw("Text payloads are shown as text, others as a hex dump."),
                Line::raw(""),
                Line::styled("Actions", heading()),
                Line::raw("a or :         open every Linka action"),
                Line::raw("Tab/↑/↓        move between form fields"),
                Line::raw("Enter          next field / submit final field"),
                Line::raw("Ctrl+Enter     submit from any field"),
                Line::raw("Esc            close a dialog"),
                Line::raw(""),
                Line::styled("Errors", heading()),
                Line::raw("Action errors remain in their form in red."),
                Line::raw("Refresh errors are retained in the Errors collection."),
            ]);
            frame.render_widget(
                Paragraph::new(help)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(ACCENT))
                            .title(" Help "),
                    )
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Overlay::Actions { selected } => {
            let area = centered(52, 82, frame.area());
            frame.render_widget(Clear, area);
            let items = ACTIONS
                .iter()
                .map(|action| ListItem::new(action.label()))
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(ACCENT))
                            .title(" Linka actions · Enter to configure "),
                    )
                    .highlight_symbol("▸ ")
                    .highlight_style(Style::default().bg(Color::Rgb(35, 45, 55))),
                area,
                &mut state,
            );
        }
        Overlay::Form(form) => {
            let height = (form.fields.len() as u16 * 3 + 7).max(10);
            let area = centered_height(72, height, frame.area());
            frame.render_widget(Clear, area);
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if form.error.is_some() {
                        ERROR
                    } else {
                        ACCENT
                    }))
                    .title(format!(" {} ", form.action.label())),
                area,
            );
            let inner = area.inner(Margin {
                horizontal: 2,
                vertical: 1,
            });
            if form.fields.is_empty() {
                let text = vec![
                    Line::raw("This action has no parameters."),
                    Line::raw(""),
                    Line::styled("Enter to run · Esc to cancel", Style::default().fg(MUTED)),
                ];
                frame.render_widget(Paragraph::new(text), inner);
            } else {
                let constraints = form
                    .fields
                    .iter()
                    .map(|_| Constraint::Length(3))
                    .chain(std::iter::once(Constraint::Min(3)))
                    .collect::<Vec<_>>();
                let rows = Layout::vertical(constraints).split(inner);
                for (index, field) in form.fields.iter().enumerate() {
                    let selected = form.selected == index;
                    let title = format!(
                        " {} · {} ",
                        field.label,
                        if field.hint.is_empty() {
                            "text"
                        } else {
                            field.hint
                        }
                    );
                    frame.render_widget(
                        Paragraph::new(field.value.as_str()).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(if selected {
                                    ACCENT
                                } else {
                                    MUTED
                                }))
                                .title(title),
                        ),
                        rows[index],
                    );
                    if selected {
                        let cursor_x = rows[index]
                            .x
                            .saturating_add(1)
                            .saturating_add(field.value.chars().count() as u16)
                            .min(rows[index].right().saturating_sub(2));
                        frame.set_cursor_position((cursor_x, rows[index].y + 1));
                    }
                }
                let footer = if let Some(error) = &form.error {
                    Text::from(vec![
                        Line::styled(format!("Error: {error}"), Style::default().fg(ERROR)),
                        Line::styled(
                            "Fix the values and press Ctrl+Enter · Esc to cancel",
                            Style::default().fg(MUTED),
                        ),
                    ])
                } else {
                    Text::from(Line::styled(
                        "Enter: next/submit · Ctrl+Enter: submit · Esc: cancel",
                        Style::default().fg(MUTED),
                    ))
                };
                frame.render_widget(
                    Paragraph::new(footer).wrap(Wrap { trim: false }),
                    rows[form.fields.len()],
                );
            }
        }
        Overlay::Attachments(browser) => {
            let area = centered(86, 84, frame.area());
            frame.render_widget(Clear, area);
            let columns =
                Layout::horizontal([Constraint::Length(34), Constraint::Min(20)]).split(area);
            let items = browser
                .items
                .iter()
                .map(|item| {
                    ListItem::new(vec![
                        Line::styled(
                            format!("{}/{}", item.namespace, item.key),
                            Style::default().fg(ACCENT),
                        ),
                        Line::styled(
                            format!(
                                "  {} B · {}",
                                item.size,
                                item.media_type.as_deref().unwrap_or("—")
                            ),
                            Style::default().fg(MUTED),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(browser.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(ACCENT))
                            .title(format!(" Attachments · {} ", browser.node)),
                    )
                    .highlight_symbol("▸ ")
                    .highlight_style(
                        Style::default()
                            .bg(Color::Rgb(35, 45, 55))
                            .add_modifier(Modifier::BOLD),
                    ),
                columns[0],
                &mut state,
            );
            draw_scrollable_text(
                frame,
                columns[1],
                " j/k select · J/K scroll · Esc to close ",
                &browser.body,
                browser.scroll,
            );
        }
        Overlay::Text {
            title,
            body,
            scroll,
        } => {
            let area = centered(80, 82, frame.area());
            frame.render_widget(Clear, area);
            draw_scrollable_text(
                frame,
                area,
                &format!(" {title} · Esc to close "),
                body,
                *scroll,
            );
        }
    }
}

/// A bordered, wrapped, scrollable body with a scrollbar once it overflows.
/// `scroll` is clamped to the wrapped height so the end is always reachable and
/// never scrolls past it.
fn draw_scrollable_text(frame: &mut Frame, area: Rect, title: &str, body: &str, scroll: u16) {
    let width = area.width.saturating_sub(2).max(1) as usize;
    let line_count = body
        .lines()
        .map(|line| line.chars().count().div_ceil(width).max(1) as u16)
        .sum::<u16>();
    let viewport = area.height.saturating_sub(2);
    let scroll = scroll.min(line_count.saturating_sub(viewport));
    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT))
                    .title(title.to_string()),
            )
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
    if line_count > viewport {
        let mut state = ScrollbarState::new(line_count as usize).position(scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut state,
        );
    }
}

fn kv(key_text: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key_text:<12}"), Style::default().fg(MUTED)),
        Span::raw(value.into()),
    ])
}

fn centered(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - height_percent) / 2),
        Constraint::Percentage(height_percent),
        Constraint::Percentage((100 - height_percent) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - width_percent) / 2),
        Constraint::Percentage(width_percent),
        Constraint::Percentage((100 - width_percent) / 2),
    ])
    .split(vertical[1])[1]
}

fn centered_height(width_percent: u16, height: u16, area: Rect) -> Rect {
    let height = height.min(area.height.saturating_sub(2)).max(3);
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - width_percent) / 2),
        Constraint::Percentage(width_percent),
        Constraint::Percentage((100 - width_percent) / 2),
    ])
    .split(vertical[1])[1]
}

fn heading() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

fn key() -> Style {
    Style::default().fg(Color::Black).bg(Color::Gray)
}

fn kind_style(kind: NodeKind) -> Style {
    let color = match kind {
        NodeKind::Work => Color::Blue,
        NodeKind::Verification => Color::Magenta,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn status_style(status: &str) -> Style {
    let color = match status {
        "complete" | "accepted" | "published" => Color::Green,
        "ready" | "pending" => Color::Cyan,
        "rejected" | "blocked" => Color::Red,
        value if value.contains("stale") || value.contains("awaiting") => Color::Yellow,
        _ => Color::White,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

trait AuthorLabel {
    fn label(self) -> &'static str;
}

impl AuthorLabel for linka::Author {
    fn label(self) -> &'static str {
        self.as_str()
    }
}
