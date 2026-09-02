//! The notes editor: a box floating over the current view holding the
//! operator's own notes on this Session and its Workspace.
//!
//! Notes are read and written in the same place, so opening the editor is also
//! how notes are read. It floats over the view rather than replacing it (as the
//! pickers do) because notes are written *about* what is on screen.

use crate::app::App;
use crate::notes::{Editor, Scope};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::input::wrapped_input_lines;
use super::palette;

/// Where the editor's box sits: centered, wide enough for prose and tall
/// enough to hold a screenful of it, without ever covering the whole terminal.
fn notes_area(frame_area: Rect) -> Rect {
    let width = frame_area.width.saturating_sub(6).clamp(1, 90);
    let height = frame_area
        .height
        .saturating_sub(4)
        .clamp(3, 24)
        .min(frame_area.height);
    Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn title(editor: &Editor) -> String {
    if editor.session_available() {
        format!(
            " {} · Tab {} · Ctrl+S save · Esc cancel ",
            editor.scope().label(),
            editor.scope().other_label()
        )
    } else {
        // Before the first message there is no Session to attach notes to, so
        // say which scope this is writing to and leave Tab unadvertised.
        format!(" {} · Ctrl+S save · Esc cancel ", editor.scope().label())
    }
}

pub(crate) fn render_notes(frame: &mut Frame, app: &App, editor: &Editor) {
    // Wash the finished view underneath down, the way the message box does, so
    // the editor reads as the only thing that can be typed into right now.
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(palette::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        frame.area(),
    );

    let area = notes_area(frame.area());
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::WARNING))
        .title(Span::styled(
            title(editor),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    if let Some(name) = scope_subject(app, editor) {
        block = block.title(
            Line::from(Span::styled(
                format!(" {name} "),
                Style::default().fg(palette::ACCENT),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);

    let text = editor.buffer();
    let mut lines = if text.is_empty() {
        vec![Line::from(Span::styled(
            "No notes yet — type to add some.",
            Style::default().fg(palette::MUTED_TEXT),
        ))]
    } else {
        wrapped_input_lines(
            text,
            usize::from(inner.width.max(1)),
            Style::default().fg(palette::TEXT),
        )
    };

    // The cursor trails the text, so a note being written stays in view even
    // once it is longer than the box.
    let mut cursor_col = if text.is_empty() {
        0
    } else {
        lines.last().map(|line| line.width()).unwrap_or_default()
    };
    if cursor_col == usize::from(inner.width) {
        lines.push(Line::default());
        cursor_col = 0;
    }
    let cursor_row = lines.len().saturating_sub(1) as u16;
    let scroll = (lines.len() as u16).saturating_sub(inner.height);

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
    frame.set_cursor_position(Position {
        x: inner.x + cursor_col as u16,
        y: inner.y + cursor_row.saturating_sub(scroll),
    });
}

/// What the notes on screen are about: the Session's name, or the Workspace's.
fn scope_subject(app: &App, editor: &Editor) -> Option<String> {
    match editor.scope() {
        Scope::Session => app
            .session_name
            .clone()
            .or_else(|| (!app.session_id.is_empty()).then(|| app.session_id.clone())),
        Scope::Workspace => app.workspace_name.clone(),
    }
}

/// The pickers' notes pane: the highlighted row's notes, read-only, beside the
/// list. Same yellow as the editor, so the two read as one feature.
pub fn render_notes_pane(frame: &mut Frame, scope: Scope, notes: Option<&str>, area: Rect) {
    let text = notes
        .filter(|text| !text.is_empty())
        .unwrap_or("No notes yet — press e to add some.");
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette::WARNING))
                .title(format!(" {} ", scope.label())),
        ),
        area,
    );
}

/// The pickers' notes editor, floating over the picker underneath on the same
/// `Ctrl+S`/`Esc` terms as the main view's editor.
pub fn render_notes_prompt(frame: &mut Frame, scope: Scope, value: &str) {
    let area = frame.area();
    let popup = Rect::new(
        area.x + 3,
        area.y + 2,
        area.width.saturating_sub(6),
        area.height.saturating_sub(4),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(value.to_owned())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(palette::WARNING))
                    .title(format!(
                        " {} · Ctrl+S save · Enter newline · Esc cancel ",
                        scope.label()
                    )),
            ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;
    use crate::app::App;
    use crate::notes;

    fn app() -> App {
        let mut app = testing::app("s1");
        app.session_name = Some("Fix retries".into());
        app.workspace_name = Some("payments".into());
        app
    }

    #[test]
    fn the_editor_opens_on_the_session_notes_and_shows_how_to_save() {
        let mut app = app();
        app.notes.set_known("check the retry budget", "");
        notes::open(&mut app);

        let screen = rendered(&app);
        assert!(screen.contains("Session notes"), "{screen}");
        assert!(screen.contains("check the retry budget"), "{screen}");
        assert!(screen.contains("Ctrl+S save"), "{screen}");
        assert!(screen.contains("Fix retries"), "{screen}");
    }

    #[test]
    fn tab_shows_the_workspace_notes_instead() {
        let mut app = app();
        app.notes.set_known("session text", "workspace text");
        notes::open(&mut app);
        notes::toggle_scope(&mut app);

        let screen = rendered(&app);
        assert!(screen.contains("workspace text"), "{screen}");
        assert!(!screen.contains("session text"), "{screen}");
    }

    #[test]
    fn an_empty_note_says_so_rather_than_showing_a_blank_box() {
        let mut app = app();
        notes::open(&mut app);
        assert!(rendered(&app).contains("No notes yet"));
    }

    /// Notes nobody can see are notes nobody will read again, so the view says
    /// when there are some — and says nothing when there are none.
    #[test]
    fn the_view_marks_a_session_that_has_notes() {
        let mut app = app();
        assert!(!rendered(&app).contains("notes"));

        app.notes.set_known("", "deploys are manual here");
        assert!(rendered(&app).contains("notes · E"));
    }

    /// With no Session launched there is nothing for Session notes to belong
    /// to, so the editor opens on the Workspace instead.
    #[test]
    fn a_pending_session_edits_workspace_notes_only() {
        let mut app = testing::pending_app();
        app.workspace_name = Some("payments".into());
        notes::open(&mut app);
        assert!(rendered(&app).contains("Workspace notes"));
    }
}
