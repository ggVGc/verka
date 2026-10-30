use super::*;
use crate::fuzzy_list::{render_fuzzy_list, FuzzyListView};
use crate::theme;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Clear};
use ratatui::Frame;

/// How tall the list is allowed to grow. The catalog crossed with the ladders
/// is longer than most terminals, and a modal that eats the whole screen hides
/// the session it was opened over — the list scrolls to its cursor instead.
const MAX_ROWS: u16 = 16;

pub fn render_launcher(frame: &mut Frame, launcher: &LauncherView, area: Rect) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(theme::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        area,
    );
    let desired_height = (launcher.rows.len() as u16)
        .min(MAX_ROWS)
        .saturating_add(if launcher.effort.is_some() { 4 } else { 2 })
        .max(4);
    let height = desired_height.min(area.height);
    let width = area
        .width
        .saturating_sub(4)
        .min(if launcher.effort.is_some() { 96 } else { 60 });
    let area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let title = if launcher.provider_locked {
        format!(" styra · {} · agent fixed · ", launcher.title)
    } else {
        format!(" styra · {} · ", launcher.title)
    };
    if let Some(effort) = &launcher.effort {
        let block = Block::bordered()
            .title(format!("{title}{} ", launcher.selection))
            .title_bottom(" ctrl-o model · ctrl-e effort · Enter select · Esc cancel ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(inner);
        render_fuzzy_list(
            frame,
            &FuzzyListView {
                title: " Models · ctrl-o ",
                rows: &launcher.rows,
                list: &launcher.list,
                focused: !effort.focused,
                empty_note: NO_ROWS,
                hint: if effort.focused {
                    " ctrl-o to focus "
                } else {
                    " type to narrow "
                },
            },
            panes[0],
        );
        render_fuzzy_list(
            frame,
            &FuzzyListView {
                title: " Effort · ctrl-e ",
                rows: &effort.rows,
                list: &effort.list,
                focused: effort.focused,
                empty_note: "not supported",
                hint: "",
            },
            panes[1],
        );
        return;
    }
    render_fuzzy_list(
        frame,
        &FuzzyListView {
            title: &format!("{title}{} ", launcher.selection),
            rows: &launcher.rows,
            list: &launcher.list,
            focused: true,
            empty_note: NO_ROWS,
            // The list is the whole picker, so its own border carries what the
            // picker has to say: the keys, and the query being typed.
            hint: &launcher.hint,
        },
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rows() -> Vec<String> {
        [
            "codex:gpt-5.6-sol/minimal",
            "claude:claude-opus-5/max",
            "claude:claude-haiku-4-5-20251001",
        ]
        .iter()
        .map(|row| (*row).to_owned())
        .collect()
    }

    fn view() -> LauncherView {
        let rows = rows();
        LauncherView {
            title: "launch".into(),
            hint: " type to narrow · ? keys ".into(),
            selection: "codex:gpt-5.6-sol/minimal".into(),
            provider_locked: false,
            list: FuzzyList::at(&rows, 0),
            effort: None,
            rows,
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

    /// Every row is a whole triple, so what is on screen is what would launch
    /// — no assembling a selection out of three columns.
    #[test]
    fn shows_whole_triples_and_the_one_it_would_confirm() {
        let screen = rendered(&view());
        for text in [
            "styra · launch",
            "type to narrow",
            "codex:gpt-5.6-sol/minimal",
            "claude:claude-opus-5/max",
            "claude:claude-haiku-4-5-20251001",
        ] {
            assert!(screen.contains(text), "missing {text}: {screen}");
        }
    }

    #[test]
    fn typing_narrows_the_list_and_shows_the_query() {
        let mut view = view();
        for character in "haiku".chars() {
            view.list.push(character);
        }
        let screen = rendered(&view);

        assert!(screen.contains("/haiku"), "the query is drawn: {screen}");
        assert!(screen.contains("claude-haiku"), "{screen}");
        assert!(!screen.contains("opus"), "the rows it excluded are gone");
    }

    #[test]
    fn a_query_matching_nothing_says_so() {
        let mut view = view();
        for character in "zzz".chars() {
            view.list.push(character);
        }
        assert!(rendered(&view).contains("no match for zzz"));
    }

    #[test]
    fn marks_a_locked_provider_as_fixed() {
        let mut view = view();
        view.provider_locked = true;
        assert!(rendered(&view).contains("agent fixed"));
    }

    #[test]
    fn shows_separate_model_and_effort_lists_with_independent_filters() {
        let mut view = view();
        view.title = "model / effort".into();
        view.rows = vec!["model-sol".into(), "model-astra".into()];
        view.list = FuzzyList::at(&view.rows, 0);
        view.list.push('s');
        view.list.push('o');
        view.list.push('l');
        let rows = vec!["low".into(), "high".into()];
        let mut list = FuzzyList::at(&rows, 1);
        for ch in "high".chars() {
            list.push(ch);
        }
        view.effort = Some(EffortView {
            rows,
            list,
            focused: true,
        });
        let screen = rendered(&view);
        for expected in ["Models", "Effort", "model-sol", "high", "/sol", "/high"] {
            assert!(screen.contains(expected), "missing {expected}: {screen}");
        }
        assert!(!screen.contains("model-astra"));
        assert!(!screen.contains("low"));
    }

    #[test]
    fn renders_as_a_centered_modal() {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_launcher(frame, &view(), frame.area()))
            .unwrap();

        // Three rows need a five-row modal, 60 columns wide; centering that in
        // an 80x20 screen puts its top-left corner at (10, 7) rather than at
        // the top of the screen.
        assert_eq!(terminal.backend().buffer()[(10, 7)].symbol(), "┌");
    }

    /// A catalog longer than the screen is what the list is for, so the modal
    /// stops growing and scrolls instead.
    #[test]
    fn a_long_catalog_does_not_grow_the_modal_past_its_limit() {
        let rows: Vec<String> = (0..60)
            .map(|index| format!("claude:model-{index}"))
            .collect();
        let mut view = view();
        view.list = FuzzyList::at(&rows, 59);
        view.rows = rows;

        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        terminal
            .draw(|frame| render_launcher(frame, &view, frame.area()))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            screen.contains("claude:model-59"),
            "the list scrolls to the cursor: {screen}"
        );
        assert!(
            !screen.contains("claude:model-0 "),
            "and leaves the top off"
        );
    }
}
