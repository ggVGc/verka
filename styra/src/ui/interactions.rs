//! The live-interaction navigator embedded above the main event timeline.

use super::{palette, short_id, status_color};
use crate::app::{App, Status};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use styra_server::InteractionSummary;

pub(crate) fn height(app: &App, available: u16) -> u16 {
    (app.interactions
        .visible_indices(app.workspace_id.as_deref())
        .len() as u16
        + 2)
    .max(3)
    .min(available.saturating_div(2).max(3))
    .min(available)
}

pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let visible = app
        .interactions
        .visible_indices(app.workspace_id.as_deref());
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
            " {scope} · live interactions · j/k make current · w scope · Enter/a close "
        ));
    let items = visible
        .iter()
        .map(|index| {
            let interaction = &app.interactions.items[*index];
            item(
                interaction,
                *index == app.interactions.selected,
                interaction.id == app.session_id,
            )
        })
        .collect::<Vec<_>>();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(
        visible
            .iter()
            .position(|index| *index == app.interactions.selected),
    );
    frame.render_stateful_widget(list, area, &mut state);
}

fn item(interaction: &InteractionSummary, selected: bool, current: bool) -> ListItem<'static> {
    let status = status(interaction);
    let color = status_color(&status);
    let name = interaction
        .name
        .clone()
        .unwrap_or_else(|| short_id(&interaction.id).to_owned());
    ListItem::new(Line::from(vec![
        Span::styled(
            if selected { "• " } else { "  " },
            Style::default().fg(if selected {
                palette::SELECTION_MARKER
            } else {
                palette::INACTIVE
            }),
        ),
        Span::styled(
            format!("{} ", status.glyph()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(name, Style::default().fg(palette::TEXT)),
        Span::styled(
            format!(" · {}", interaction.workspace_id),
            Style::default().fg(palette::WARNING),
        ),
        Span::styled(
            format!(" · {}", interaction.selection.provider.as_str()),
            Style::default().fg(palette::ACCENT),
        ),
        Span::styled(
            if current { " · current" } else { "" },
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
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
            "s-2",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "current timeline".into(),
        });

        let screen = testing::rendered(&app);
        let navigator = screen.find("live interactions").unwrap();
        let timeline = screen.find("current timeline").unwrap();
        assert!(navigator < timeline, "{screen}");
        assert!(
            screen.contains("second · payments · codex · current"),
            "{screen}"
        );
    }

    #[test]
    fn navigator_names_and_filters_the_current_workspace_scope() {
        let mut app = testing::app("s-1");
        app.workspace_id = Some("payments".into());
        app.workspace_name = Some("Payments".into());
        let mut other = interaction("s-2", "other");
        other.workspace_id = "ledger".into();
        app.interactions
            .open(vec![interaction("s-1", "current"), other], "s-1");
        app.interactions
            .toggle_workspace_scope("s-1", Some("payments"));

        let screen = testing::rendered(&app);
        assert!(screen.contains("Payments · live interactions"), "{screen}");
        assert!(screen.contains("current · payments"), "{screen}");
        assert!(!screen.contains("other · ledger"), "{screen}");
    }
}
