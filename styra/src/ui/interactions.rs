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
        app.workspace
            .name
            .as_deref()
            .or(app.workspace.id.as_deref())
            .unwrap_or("Current Workspace")
    } else {
        "All"
    };
    // The Workspace jump only exists where there are Workspace groups to jump
    // between, so it is only advertised in All scope.
    let jump = if app.interactions.only_current_workspace {
        ""
    } else {
        "ctrl-j/k workspace · "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(format!(
            " {scope} · live interactions · j/k move · {jump}Enter close · S stop · D delete stopped · w scope · a close "
        ));
    // The cursor and the interaction on screen part company while a move is
    // settling or its load is running: the cursor is where the operator is,
    // the marked row is what the panes below still show.
    let cursor = app.interactions.cursor(&app.session_id);
    let loading = app
        .interactions
        .pending(&app.session_id)
        .map(|interaction| interaction.id.as_str());
    let items = rows
        .iter()
        .map(|row| match row {
            Row::Workspace(name) => workspace_heading(name),
            Row::Interaction(index) => {
                let interaction = &app.interactions.items[*index];
                item(
                    interaction,
                    interaction.id == app.session_id,
                    loading == Some(interaction.id.as_str()),
                    item_width,
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
        |row| matches!(row, Row::Interaction(index) if app.interactions.items[*index].id == cursor),
    ));
    frame.render_stateful_widget(list, area, &mut state);
}

enum Row {
    Workspace(String),
    Interaction(usize),
}

fn rows(app: &App) -> Vec<Row> {
    let ordered = app
        .interactions
        .display_indices(app.workspace.id.as_deref());
    if app.interactions.only_current_workspace {
        return ordered.into_iter().map(Row::Interaction).collect();
    }

    // `display_indices` already groups the entries by Workspace, so a heading
    // is due wherever the Workspace changes.
    let mut rows = Vec::new();
    let mut heading = None;
    for index in ordered {
        let workspace_id = &app.interactions.items[index].workspace_id;
        if heading.as_ref() != Some(workspace_id) {
            rows.push(Row::Workspace(workspace_name(app, workspace_id)));
            heading = Some(workspace_id.clone());
        }
        rows.push(Row::Interaction(index));
    }
    rows
}

fn workspace_name(app: &App, workspace_id: &str) -> String {
    app.interactions
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .map(crate::workspace::display_name)
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

/// One interaction's row. `current` is the interaction the panes below show,
/// which carries the marker; `loading` is set on the row the cursor has come
/// to but which has not replaced it yet.
fn item(
    interaction: &InteractionSummary,
    current: bool,
    loading: bool,
    width: u16,
) -> ListItem<'static> {
    let status = status(interaction);
    let color = status_color(&status);
    let name = interaction
        .name
        .clone()
        .unwrap_or_else(|| short_id(&interaction.id).to_owned());
    let marker = if status == Status::Running {
        // Each row's own event count, not the attached session's: the rows
        // that are working animate whether or not this client's session is.
        super::running_indicator(interaction.events).to_owned()
    } else {
        status.glyph().to_string()
    };
    let mut main = vec![
        Span::styled(
            if current { "• " } else { "  " },
            Style::default().fg(if current {
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
    ];
    // Said on the row rather than left to the panes below, which go on showing
    // the marked interaction until this one arrives: a screen that has not
    // caught up with the cursor and one that has look nothing alike.
    if loading {
        main.push(Span::styled(
            " · loading…",
            Style::default().fg(palette::INACTIVE),
        ));
    }
    if interaction.accepting
        && interaction.activity == styra_server::InteractionActivity::Pending
        && interaction.idle_unseen
    {
        main.push(Span::styled(
            " · NEWLY IDLE",
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![Line::from(main)];
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
            auto_retry: false,
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
                base: Vec::new(),
                mounts: vec![],
            },
            accepting: true,
            activity: InteractionActivity::Pending,
            idle_unseen: false,
            last_message: None,
            events: 0,
        }
    }

    #[test]
    fn navigator_is_above_the_current_interactions_event_log() {
        let mut app = testing::app("s-2");
        app.interactions.open(
            vec![interaction("s-1", "first"), interaction("s-2", "second")],
            vec![],
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
        app.interactions.open(vec![interaction], vec![]);

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
        app.interactions.open(vec![interaction], vec![]);

        let screen = testing::rendered(&app);

        assert!(
            screen.contains(super::super::running_indicator(0)),
            "{screen}"
        );
        assert!(!screen.contains("> working"), "{screen}");
    }

    #[test]
    fn navigator_marks_an_idle_interaction_that_has_not_been_focused() {
        let mut app = testing::app("s-1");
        let mut unseen = interaction("s-2", "finished elsewhere");
        unseen.idle_unseen = true;
        app.interactions
            .open(vec![interaction("s-1", "current"), unseen], vec![]);

        assert!(testing::rendered(&app).contains("NEWLY IDLE"));
    }

    #[test]
    fn navigator_steps_each_spinner_with_that_interactions_own_events() {
        // The attached session is idle and has seen nothing, so a shared
        // counter would freeze both rows on frame zero.
        let mut app = testing::app("s-1");
        let mut first = interaction("s-1", "first");
        first.activity = InteractionActivity::Running;
        let mut second = interaction("s-2", "second");
        second.activity = InteractionActivity::Running;
        second.events = 3;
        app.interactions.open(vec![first, second], vec![]);

        let screen = testing::rendered(&app);
        let phases = ["first", "second"].map(|name| {
            let row = screen.find(&format!("{name} · codex")).unwrap() / 80;
            (0..super::super::RUNNING_INDICATOR.len())
                .find(|frame| {
                    screen
                        .find(super::super::running_indicator(*frame))
                        .is_some_and(|at| at / 80 == row)
                })
                .unwrap()
        });

        assert_eq!(phases, [0, 3], "{screen}");
    }

    /// While a move settles, the cursor and the interaction on screen are two
    /// different rows, and the navigator has to say which is which: the panes
    /// below still belong to the marked one.
    #[test]
    fn a_cursor_that_has_moved_off_the_current_interaction_says_it_is_loading() {
        let mut app = testing::app("s-1");
        app.workspace.id = Some("payments".into());
        app.interactions.open(
            vec![interaction("s-1", "first"), interaction("s-2", "second")],
            vec![],
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "first's timeline".into(),
        });

        app.interactions.cursor_next("s-1", Some("payments"));

        let screen = testing::screen(&app);
        let (_, current) = screen.find("first · codex");
        let (cursor_x, cursor) = screen.find("second · codex");
        assert!(screen.row(cursor).contains("loading…"), "{}", screen.all());
        assert!(!screen.row(current).contains("loading"), "{}", screen.all());
        assert!(screen.row(current).contains('•'), "{}", screen.all());
        // The cursor is highlighted where the marker is not, and the screen
        // below is still the interaction the marker names.
        assert_eq!(
            screen.buffer().cell((cursor_x, cursor)).unwrap().style().bg,
            Some(palette::SELECTION_BACKGROUND)
        );
        assert!(
            screen.all().contains("first's timeline"),
            "{}",
            screen.all()
        );
    }

    #[test]
    fn navigator_names_and_filters_the_current_workspace_scope() {
        let mut app = testing::app("s-1");
        app.workspace.id = Some("payments".into());
        app.workspace.name = Some("Payments".into());
        let mut other = interaction("s-2", "other");
        other.workspace_id = "ledger".into();
        app.interactions
            .open(vec![interaction("s-1", "current"), other], vec![]);
        app.interactions.toggle_workspace_scope();

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
        app.interactions
            .open(vec![interaction("s-1", "payments session"), ledger], vec![]);

        let screen = testing::rendered(&app);
        let payments_heading = screen.find(" payments").unwrap();
        let payments_session = screen.find("payments session").unwrap();
        let ledger_heading = screen.find(" ledger").unwrap();
        let ledger_session = screen.find("ledger session").unwrap();
        assert!(payments_heading < payments_session, "{screen}");
        assert!(ledger_heading < ledger_session, "{screen}");
    }
}
