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
mod tests;
