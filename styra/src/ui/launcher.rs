//! The launch picker: agent, model, and reasoning effort side by side, with
//! the resulting selection spelled out along the bottom border so the
//! operator sees exactly what it is selecting.

use super::palette;
use crate::launcher::{LaunchColumn, Launcher};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use styra_server::agent::PROVIDERS;

pub(crate) fn render_launcher(frame: &mut Frame, launcher: &Launcher, area: Rect) {
    let provider = launcher.provider();
    let selection = launcher.selection();
    let hint = " j/k choose · Tab/h/l column · Enter select · D save default · q cancel ";
    // The composed selection and the key hints go on the outer frame rather
    // than on a column, where a narrow terminal would clip them.
    let frame_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Line::from(vec![
            Span::styled(
                " styra · launch · ",
                Style::default().fg(palette::MUTED_TEXT),
            ),
            Span::styled(
                format!("{} ", selection.name()),
                Style::default()
                    .fg(palette::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_bottom(Line::from(Span::styled(
            hint,
            Style::default().fg(palette::MUTED_TEXT),
        )));
    let inner = frame_block.inner(area);
    frame.render_widget(frame_block, area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(inner);

    let providers: Vec<String> = PROVIDERS
        .iter()
        .map(|provider| provider.as_str().to_owned())
        .collect();
    // The catalog ordered by recency, and a model the catalog doesn't list
    // carried from the profile the picker was opened on — see `Launcher`.
    let models = launcher.models();
    let efforts: Vec<String> = provider
        .efforts()
        .iter()
        .map(|effort| effort.as_str().to_owned())
        .collect();

    render_launcher_column(
        frame,
        columns[0],
        " agent ",
        &providers,
        launcher.provider,
        launcher.column == LaunchColumn::Provider,
    );
    render_launcher_column(
        frame,
        columns[1],
        " model ",
        &models,
        launcher.model,
        launcher.column == LaunchColumn::Model,
    );
    render_launcher_column(
        frame,
        columns[2],
        " effort ",
        &efforts,
        launcher.effort,
        launcher.column == LaunchColumn::Effort,
    );
}

fn render_launcher_column(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    rows: &[String],
    selected: usize,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(palette::ACCENT)
    } else {
        Style::default().fg(palette::INACTIVE)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            title.to_owned(),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    if index == selected { "• " } else { "  " },
                    Style::default().fg(if index == selected {
                        palette::SELECTION_MARKER
                    } else {
                        palette::TEXT
                    }),
                ),
                Span::styled(row.clone(), Style::default().fg(palette::TEXT)),
            ]))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    if !rows.is_empty() {
        state.select(Some(selected.min(rows.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;
    use crate::app::App;

    use styra_server::agent::Provider;

    /// An open launcher over a pending session. The log entry is here because
    /// the launcher draws over a session view that has one.
    fn launching() -> App {
        let mut app = testing::pending_app();
        app.push_log(styra_server::LogEntry::info("journal: /tmp/styra/s-1"));
        app.open_launcher();
        app
    }

    #[test]
    fn the_launcher_shows_a_column_per_axis_of_the_profile() {
        let app = launching();
        let screen = rendered(&app);
        assert!(screen.contains("styra · launch"), "{screen}");
        for column in ["agent", "model", "effort"] {
            assert!(
                screen.contains(column),
                "missing the {column} column: {screen}"
            );
        }
        assert!(screen.contains("gpt-5.6-sol"), "{screen}");
        assert!(screen.contains("minimal"), "{screen}");
        assert!(screen.contains("Enter select"), "{screen}");
        assert!(screen.contains("D save default"), "{screen}");
    }

    /// Every row is a concrete choice out of the agent's own catalogs: no
    /// free-text row to type an id into, and no row standing for "whatever the
    /// agent is configured for".
    #[test]
    fn the_launcher_offers_only_concrete_catalog_rows() {
        let body = testing::screen(&launching()).body();
        assert!(!body.contains("custom"), "{body}");
        assert!(!body.contains("│ default"), "{body}");
    }

    /// The picker spells out the profile its current rows add up to. Both the
    /// starting profile and the expected one are named, so the arithmetic is
    /// between two fixed points rather than "one row along from the default".
    #[test]
    fn the_launcher_title_states_the_profile_the_rows_add_up_to() {
        let mut app = launching();
        assert!(
            rendered(&app).contains(testing::PROFILE),
            "the launcher should open on the profile the session carries"
        );

        let launcher = app.launcher.as_mut().unwrap();
        launcher.next_column();
        launcher.next(); // gpt-5.6-sol -> gpt-5.6-terra
        launcher.next_column();
        launcher.prev(); // high -> medium

        let screen = rendered(&app);
        assert!(screen.contains("codex:gpt-5.6-terra/medium"), "{screen}");
    }

    /// Nothing of the session view shows through a modal picker. Asserted over
    /// the body rather than the whole screen: the footer renders the host's
    /// working directory, and a needle this short can match a path component.
    #[test]
    fn the_launcher_is_modal_and_hides_the_session_view() {
        let body = testing::screen(&launching()).body();
        assert!(!body.contains("message"), "{body}");
    }

    /// A model the catalog does not list — one the operator named with
    /// stored state is still shown, as a final row the picker carries.
    #[test]
    fn the_launcher_shows_a_carried_model_alongside_the_catalog() {
        let mut app = App::pending(
            styra_server::agent::Selection::parse("claude:claude-opus-4-1-20250805").unwrap(),
        );
        app.open_launcher();

        let screen = rendered(&app);
        assert!(screen.contains("claude-opus-4-1-20250805"), "{screen}");
        // Alongside the catalog it is not part of.
        assert!(screen.contains("claude-opus-5"), "{screen}");
        assert!(
            screen.contains("claude:claude-opus-4-1-20250805"),
            "{screen}"
        );
    }

    /// Every model the picker offers for Claude Code is a full id, so the
    /// composed selection names the exact model rather than a moving alias.
    #[test]
    fn the_claude_column_offers_full_model_ids() {
        let mut app = App::pending(styra_server::agent::Selection::parse("claude").unwrap());
        app.open_launcher();
        let screen = rendered(&app);
        for model in Provider::Claude.models() {
            assert!(model.starts_with("claude-"), "{model} is not a full id");
        }
        // The top of the catalog is visible on an 80x20 screen.
        assert!(screen.contains("claude-fable-5"), "{screen}");
        assert!(screen.contains("claude-opus-5"), "{screen}");
    }
}
