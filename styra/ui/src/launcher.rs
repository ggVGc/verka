//! The launch picker: agent, model, and reasoning effort side by side, with
//! the resulting selection spelled out along the bottom border so the
//! operator sees exactly what it is selecting.

use crate::palette;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

pub struct LauncherView {
    pub selection: String,
    pub provider_locked: bool,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub provider_selected: usize,
    pub model_selected: usize,
    pub effort_selected: usize,
    pub focused: LauncherColumn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherColumn {
    Provider,
    Model,
    Effort,
}

pub fn render_launcher(frame: &mut Frame, launcher: &LauncherView, area: Rect) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(palette::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        area,
    );
    let desired_height = (launcher
        .providers
        .len()
        .max(launcher.models.len())
        .max(launcher.efforts.len()) as u16)
        .saturating_add(2)
        .max(4);
    let height = desired_height.min(area.height);
    let width = area.width.saturating_sub(4).min(100);
    let area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let hint = " ? keys ";
    let frame_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Line::from(vec![
            Span::styled(
                " styra · launch · ",
                Style::default().fg(palette::MUTED_TEXT),
            ),
            Span::styled(
                format!("{} ", launcher.selection),
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
    render_launcher_column(
        frame,
        columns[0],
        if launcher.provider_locked {
            " agent · fixed "
        } else {
            " agent "
        },
        &launcher.providers,
        launcher.provider_selected,
        launcher.focused == LauncherColumn::Provider,
    );
    render_launcher_column(
        frame,
        columns[1],
        " model ",
        &launcher.models,
        launcher.model_selected,
        launcher.focused == LauncherColumn::Model,
    );
    render_launcher_column(
        frame,
        columns[2],
        " effort ",
        &launcher.efforts,
        launcher.effort_selected,
        launcher.focused == LauncherColumn::Effort,
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
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn view() -> LauncherView {
        LauncherView {
            selection: "codex:gpt-5.6-sol/minimal".into(),
            provider_locked: false,
            providers: vec!["codex".into(), "claude".into()],
            models: vec!["gpt-5.6-sol".into(), "gpt-5.6-terra".into()],
            efforts: vec!["minimal".into(), "medium".into()],
            provider_selected: 0,
            model_selected: 0,
            effort_selected: 0,
            focused: LauncherColumn::Model,
        }
    }

    fn rendered(view: &LauncherView) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_launcher(frame, view, frame.area()))
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
    fn shows_each_profile_axis_and_the_composed_selection() {
        let screen = rendered(&view());
        for text in [
            "styra · launch",
            "agent",
            "model",
            "effort",
            "gpt-5.6-sol",
            "minimal",
        ] {
            assert!(screen.contains(text), "missing {text}: {screen}");
        }
    }

    #[test]
    fn marks_a_locked_provider_as_fixed() {
        let mut view = view();
        view.provider_locked = true;
        assert!(rendered(&view).contains("agent · fixed"));
    }

    #[test]
    fn renders_as_a_centered_modal() {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_launcher(frame, &view(), frame.area()))
            .unwrap();

        // Two rows of choices need a four-row modal; centering it in 20 rows
        // puts its top border at row 8 rather than at the top of the screen.
        assert_eq!(terminal.backend().buffer()[(2, 8)].symbol(), "┌");
    }
}
