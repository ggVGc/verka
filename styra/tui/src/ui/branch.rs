//! The branching choice, floating over the interaction it will branch.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

use super::palette;
use crate::branch::BranchPrompt;

pub(crate) fn render(frame: &mut Frame, prompt: &BranchPrompt, frame_area: Rect) {
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
    let items = [
        ListItem::new(Line::from("entire interaction through this entry")),
        ListItem::new(Line::from("only this entry")),
    ];
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(prompt.selected_index()));
    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;

    #[test]
    fn chooser_offers_both_branch_histories_and_marks_the_choice() {
        let mut app = testing::app("s1");
        app.branch_prompt = Some(BranchPrompt::new(42));
        app.branch_prompt.as_mut().unwrap().select_next();

        let screen = testing::screen(&app);
        assert!(screen
            .all()
            .contains("entire interaction through this entry"));
        let (x, y) = screen.find("only this entry");
        assert_eq!(
            screen.buffer().cell((x, y)).unwrap().style().bg,
            Some(palette::SELECTION_BACKGROUND)
        );
    }
}
