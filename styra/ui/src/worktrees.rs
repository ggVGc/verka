//! The worktree picker: every linked checkout Styra knows of, one row per
//! Session working in it, narrowed as it is typed at. What the cursor is on is
//! spelled out beneath the list, because a row has room for the checkout's
//! name and the Session's, and not for the path or the branch.

use crate::fuzzy_list::{render_fuzzy_list, FuzzyList, FuzzyListView};
use crate::theme;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub struct WorktreePickerView<'a> {
    /// Which Workspaces the list covers, drawn in the title.
    pub scope: &'a str,
    /// One row per Session in a worktree, and one for a worktree no Session
    /// records, in the order the caller grouped them.
    pub rows: &'a [String],
    /// The query typed at the list and the cursor among what it left standing.
    pub list: &'a FuzzyList,
    /// What the row under the cursor is, one `(label, value)` line each.
    pub detail: &'a [(&'a str, String)],
    /// A one-off answer to the last key, such as why Enter did nothing.
    pub notice: Option<&'a str>,
}

/// What the list says when no Session in scope has a worktree.
pub const NO_ROWS: &str = "no session has a worktree";

pub fn render_worktree_picker(frame: &mut Frame, view: &WorktreePickerView<'_>) {
    let detail_height = view.detail.len() as u16 + u16::from(view.notice.is_some()) + 2;
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(detail_height)])
        .split(frame.area());
    render_fuzzy_list(
        frame,
        &FuzzyListView {
            title: &format!(" styra · worktrees · {} ", view.scope),
            rows: view.rows,
            list: view.list,
            focused: true,
            empty_note: NO_ROWS,
            hint: " type to narrow · Enter open · ? keys ",
        },
        panes[0],
    );

    let mut lines: Vec<Line> = view
        .detail
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(
                    format!(" {label:<10}"),
                    Style::default().fg(theme::MUTED_TEXT),
                ),
                Span::styled(value.clone(), Style::default().fg(theme::TEXT)),
            ])
        })
        .collect();
    if let Some(notice) = view.notice {
        lines.push(Line::from(Span::styled(
            format!(" {notice}"),
            Style::default()
                .fg(theme::WARNING)
                .add_modifier(Modifier::BOLD),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::INACTIVE)),
        ),
        panes[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(view: &WorktreePickerView<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| render_worktree_picker(frame, view))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The list says what each row is, and the pane beneath it says where the
    /// row under the cursor is on disk — the part a row has no room for.
    #[test]
    fn the_selected_rows_checkout_is_spelled_out_beneath_the_list() {
        let rows = vec![
            "fix-flaky-test-s-1  fix the flaky test · live".to_owned(),
            "tidy-up-s-2  s-2 · completed".to_owned(),
        ];
        let list = FuzzyList::at(&rows, 0);
        let detail = [
            ("worktree", "/state/worktrees/fix-flaky-test-s-1".to_owned()),
            ("branch", "styra/fix-flaky-test-s-1".to_owned()),
        ];
        let screen = rendered(&WorktreePickerView {
            scope: "this Workspace",
            rows: &rows,
            list: &list,
            detail: &detail,
            notice: Some("no session works in this worktree"),
        });

        assert!(screen.contains("worktrees · this Workspace"), "{screen}");
        assert!(screen.contains("fix the flaky test · live"), "{screen}");
        assert!(screen.contains("tidy-up-s-2"), "{screen}");
        assert!(
            screen.contains("/state/worktrees/fix-flaky-test-s-1"),
            "{screen}"
        );
        assert!(screen.contains("styra/fix-flaky-test-s-1"), "{screen}");
        assert!(
            screen.contains("no session works in this worktree"),
            "{screen}"
        );
    }
}
