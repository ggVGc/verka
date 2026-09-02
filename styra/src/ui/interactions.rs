//! The live-interaction navigator embedded above the main event timeline.

use super::{palette, short_id, status_color};
use crate::activity::Status;
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use styra_server::InteractionSummary;

pub(crate) fn height(app: &App, available: u16) -> u16 {
    let rows = rows(app);
    let message_rows = rows
        .iter()
        .filter(|row| {
            matches!(row, Row::Interaction(index) if app.interactions.items[*index].last_message.is_some())
        })
        .count() as u16;
    (rows.len() as u16 + message_rows + 2)
        .max(3)
        .min(available.saturating_div(2).max(3))
        .min(available)
}

pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let rows = rows(app);
    let item_width = area.width.saturating_sub(2);
    let scope = if app.interactions.only_current_workspace {
        app.workspace_name
            .as_deref()
            .or(app.workspace_id.as_deref())
            .unwrap_or("Current Workspace")
    } else {
        "All"
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(format!(
            " {scope} · live interactions · j/k preview · Enter load · S stop · D delete stopped · w scope · a close "
        ));
    let items = rows
        .iter()
        .map(|row| match row {
            Row::Workspace(name) => workspace_heading(name),
            Row::Interaction(index) => {
                let interaction = &app.interactions.items[*index];
                item(
                    interaction,
                    *index == app.interactions.selected,
                    item_width,
                    app.activity.progress().events,
                )
            }
        })
        .collect::<Vec<_>>();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(rows.iter().position(
        |row| matches!(row, Row::Interaction(index) if *index == app.interactions.selected),
    ));
    frame.render_stateful_widget(list, area, &mut state);
}

enum Row {
    Workspace(String),
    Interaction(usize),
}

fn rows(app: &App) -> Vec<Row> {
    let visible = app
        .interactions
        .visible_indices(app.workspace_id.as_deref());
    if app.interactions.only_current_workspace {
        return visible.into_iter().map(Row::Interaction).collect();
    }

    let mut rows = Vec::new();
    let mut placed = vec![false; app.interactions.items.len()];
    for leader in &visible {
        if placed[*leader] {
            continue;
        }
        let workspace_id = &app.interactions.items[*leader].workspace_id;
        rows.push(Row::Workspace(workspace_name(app, workspace_id)));
        for index in &visible {
            if !placed[*index] && app.interactions.items[*index].workspace_id == *workspace_id {
                placed[*index] = true;
                rows.push(Row::Interaction(*index));
            }
        }
    }
    rows
}

fn workspace_name(app: &App, workspace_id: &str) -> String {
    app.interactions
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .map(crate::session::workspace_display_name)
        .unwrap_or_else(|| workspace_id.to_owned())
}

fn workspace_heading(name: &str) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        format!(" {name}"),
        Style::default()
            .fg(palette::WARNING)
            .add_modifier(Modifier::BOLD),
    )))
}

fn item(
    interaction: &InteractionSummary,
    selected: bool,
    width: u16,
    events: usize,
) -> ListItem<'static> {
    let status = status(interaction);
    let color = status_color(&status);
    let name = interaction
        .name
        .clone()
        .unwrap_or_else(|| short_id(&interaction.id).to_owned());
    let marker = if status == Status::Running {
        super::running_indicator(events).to_owned()
    } else {
        status.glyph().to_string()
    };
    let main = Line::from(vec![
        Span::styled(
            if selected { "• " } else { "  " },
            Style::default().fg(if selected {
                palette::SELECTION_MARKER
            } else {
                palette::INACTIVE
            }),
        ),
        Span::styled(
            format!("{marker} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(name, Style::default().fg(palette::TEXT)),
        Span::styled(
            format!(" · {}", interaction.selection.provider.as_str()),
            Style::default().fg(palette::ACCENT),
        ),
    ]);
    let mut lines = vec![main];
    if let Some(text) = &interaction.last_message {
        let body = format!("    « {text}");
        let padding = (width as usize).saturating_sub(body.chars().count());
        lines.push(Line::from(Span::styled(
            format!("{body}{}", " ".repeat(padding)),
            Style::default()
                .fg(palette::SUBORDINATE_TEXT)
                .bg(palette::SUBORDINATE_BACKGROUND),
        )));
    }
    ListItem::new(lines)
}

fn status(interaction: &InteractionSummary) -> Status {
    if interaction.accepting {
        Status::from(interaction.activity)
    } else {
        Status::Ended {
            exit_code: None,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing;
    use std::path::PathBuf;
    use styra_server::event::AgentEvent;
    use styra_server::{DrivaOptions, InteractionActivity};

    fn interaction(id: &str, name: &str) -> InteractionSummary {
        InteractionSummary {
            id: id.into(),
            name: Some(name.into()),
            workspace_id: "payments".into(),
            selection: styra_server::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                mounts: vec![],
            },
            accepting: true,
            activity: InteractionActivity::Pending,
            last_message: None,
        }
    }

    #[test]
    fn navigator_is_above_the_current_interactions_event_log() {
        let mut app = testing::app("s-2");
        app.interactions.open(
            vec![interaction("s-1", "first"), interaction("s-2", "second")],
            vec![],
            "s-2",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "current timeline".into(),
        });

        let screen = testing::rendered(&app);
        let navigator = screen.find("live interactions").unwrap();
        let timeline = screen.find("current timeline").unwrap();
        assert!(navigator < timeline, "{screen}");
        assert!(screen.contains(" payments"), "{screen}");
        assert!(screen.contains("second · codex"), "{screen}");
        assert!(!screen.contains("· current"), "{screen}");
    }

    #[test]
    fn navigator_puts_the_latest_response_below_the_interaction() {
        let mut app = testing::app("s-1");
        let mut interaction = interaction("s-1", "first");
        interaction.last_message = Some("The checks are green.".into());
        app.interactions.open(vec![interaction], vec![], "s-1");

        let screen = testing::rendered(&app);
        let interaction_row = screen.find("first · codex").unwrap() / 80;
        let response_row = screen.find("« The checks are green.").unwrap() / 80;

        assert_eq!(response_row, interaction_row + 1, "{screen}");
    }

    #[test]
    fn navigator_uses_the_running_spinner_instead_of_the_static_glyph() {
        let mut app = testing::app("s-1");
        let mut interaction = interaction("s-1", "working");
        interaction.activity = InteractionActivity::Running;
        app.interactions.open(vec![interaction], vec![], "s-1");

        let screen = testing::rendered(&app);

        assert!(
            screen.contains(super::super::running_indicator(0)),
            "{screen}"
        );
        assert!(!screen.contains("> working"), "{screen}");
    }

    #[test]
    fn navigator_names_and_filters_the_current_workspace_scope() {
        let mut app = testing::app("s-1");
        app.workspace_id = Some("payments".into());
        app.workspace_name = Some("Payments".into());
        let mut other = interaction("s-2", "other");
        other.workspace_id = "ledger".into();
        app.interactions
            .open(vec![interaction("s-1", "current"), other], vec![], "s-1");
        app.interactions
            .toggle_workspace_scope("s-1", Some("payments"));

        let screen = testing::rendered(&app);
        assert!(screen.contains("Payments · live interactions"), "{screen}");
        assert!(screen.contains("current · codex"), "{screen}");
        assert!(!screen.contains("other"), "{screen}");
        assert!(!screen.contains("ledger"), "{screen}");
    }

    #[test]
    fn all_scope_groups_interactions_under_workspace_headings() {
        let mut app = testing::app("s-1");
        let mut ledger = interaction("s-2", "ledger session");
        ledger.workspace_id = "ledger".into();
        app.interactions.open(
            vec![interaction("s-1", "payments session"), ledger],
            vec![],
            "s-1",
        );

        let screen = testing::rendered(&app);
        let payments_heading = screen.find(" payments").unwrap();
        let payments_session = screen.find("payments session").unwrap();
        let ledger_heading = screen.find(" ledger").unwrap();
        let ledger_session = screen.find("ledger session").unwrap();
        assert!(payments_heading < payments_session, "{screen}");
        assert!(ledger_heading < ledger_session, "{screen}");
    }
}
