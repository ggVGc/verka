//! Terminal rendering of [`App`] with ratatui.
//!
//! The event list (each entry a summary line that grows inline when expanded),
//! messages, and a one-line status/help footer, with the message box floating
//! over the center while active.
//! Rendering is a pure function of `App`; all state lives in [`crate::app`].

mod driva;
mod footer;
mod help;
mod input;
mod launcher;
mod list;
mod log;
mod markdown;
mod messages;
mod picker;
mod preview;
mod raw;
mod transcript;

use driva::render_driva;
pub(crate) use footer::{message_text_color, tag_color};
use footer::render_footer;
use help::render_keybinds;
use input::{input_area_height, render_input};
use launcher::render_launcher;
pub(crate) use list::summary_line;
use list::render_list;
use log::render_log;
pub(crate) use log::log_line;
use messages::{message_area_height, render_messages};
pub use picker::{
    render_interactions_picker, render_name_prompt, render_picker, render_workspace_picker,
};
use preview::{render_fullscreen_preview, render_preview};
pub(crate) use preview::preview_scroll_limit;
use raw::render_raw;
use transcript::render_transcript_view;

use crate::app::{App, Focus, LaunchLabel, Status, View};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Cap on detail lines shown for one expanded entry, so a single noisy command
/// cannot bury the rest of the session.
const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";
/// Backdrop painted behind a selected list row (including its expanded detail
/// lines, if any), via the list's `highlight_style`. Deliberately not white —
/// with `Modifier::REVERSED`, a `White` foreground would reverse onto the
/// background and flip the whole selected row to a glaring full-white fill.
/// A custom RGB rather than `Color::DarkGray`: the latter reads as too light
/// a highlight against the app's otherwise dark palette.
const SELECTION_BG: Color = Color::Rgb(38, 38, 38);

/// Color coding for the status dot, so running vs. idle for input reads at
/// a glance instead of requiring the operator to read the label text.
fn status_color(status: &Status) -> Color {
    match status {
        Status::Pending => Color::Blue,
        Status::Running => Color::Yellow,
        Status::Idle => Color::Green,
        Status::Stopped => Color::DarkGray,
        Status::Ended { error: Some(_), .. } => Color::Red,
        Status::Ended { .. } => Color::DarkGray,
    }
}

/// Build a block title of the form
/// " styra · agent · model · effort · ● status[ · suffix] ".
///
/// The model and effort are named in every view's title because they are what
/// the session is actually spending: the agent alone does not say it (a bare
/// provider pins no model), and the answer can differ from what was asked for.
/// A value the agent itself reported is shown plainly; one that only reflects
/// the launch request, not yet confirmed by the agent, is dimmed to mark it as
/// such. See [`App::launch_label`].
///
/// The plain-text spans are explicitly colored rather than left unstyled:
/// an unstyled span only patches over whatever the block's border already
/// painted underneath it, so when the border dims to `DarkGray` for an
/// unfocused panel, unstyled title text would dim right along with it and
/// become hard to read.
fn title_line(label: &LaunchLabel, status: &Status, suffix: Option<&str>) -> Line<'static> {
    let color = status_color(status);
    let text_style = Style::default().fg(Color::Gray);
    let value_style = |reported: bool| {
        if reported {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };
    let mut spans = vec![Span::styled(
        format!(" styra · {} · ", label.agent),
        text_style,
    )];
    // Launch profiles always name a model; retain a plain fallback for a
    // malformed in-memory label rather than leaving the title empty.
    let model = label
        .model
        .clone()
        .unwrap_or_else(|| "default model".into());
    spans.push(Span::styled(model, value_style(label.model_reported)));
    if let Some(effort) = &label.effort {
        spans.push(Span::styled(" · ", text_style));
        spans.push(Span::styled(
            effort.clone(),
            value_style(label.effort_reported),
        ));
    }
    spans.push(Span::styled(" · ", text_style));
    spans.push(Span::styled("● ", Style::default().fg(color)));
    spans.push(Span::styled(
        status.label(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    spans.push(match suffix {
        Some(suffix) => Span::styled(format!(" · {suffix} "), text_style),
        None => Span::styled(" ", text_style),
    });
    Line::from(spans)
}

/// The chrome every full-region view wears: a border that brightens when the
/// list has focus, the session's status title, and the Workspace name at the
/// top right. `suffix` names the view in the title; `None` is the event list,
/// which is the default view and so needs no name.
fn view_block(app: &App, suffix: Option<&str>) -> Block<'static> {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.launch_label(), &app.status, suffix));
    if let Some(title) = workspace_title(app) {
        block = block.title(title);
    }
    block
}

/// A view's empty state: one muted line saying why there is nothing to show,
/// inside that view's own block so the chrome stays put as content arrives.
fn render_placeholder(frame: &mut Frame, block: Block<'static>, area: Rect, text: &str) {
    let paragraph = Paragraph::new(Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(Color::Gray),
    )))
    .block(block);
    frame.render_widget(paragraph, area);
}

/// The marker the event list and transcript show while the conversation-only
/// filter is on. Only those two views follow that filter, so this is kept out
/// of [`view_block`] rather than shown over the raw, log, and driva views too.
fn conversation_only_title(block: Block<'static>) -> Block<'static> {
    block.title_bottom(Line::from(Span::styled(
        " conversation only ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )))
}

/// Right-aligned title for the primary panel's top border.
fn workspace_title(app: &App) -> Option<Line<'static>> {
    app.session_name
        .as_ref()
        .or(app.workspace_name.as_ref())
        .map(|name| {
        Line::from(Span::styled(
            format!(" {name} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .right_aligned()
    })
}

pub fn render(frame: &mut Frame, app: &App) {
    if app.show_keybinds {
        render_keybinds(frame, frame.area());
        return;
    }

    // The launch picker is modal: it is the only thing that can be acted on
    // while open, so it takes the whole frame like the session and interaction
    // pickers do, rather than overlaying a screen whose keys are inert.
    if let Some(launcher) = &app.launcher {
        render_launcher(frame, launcher, frame.area());
        return;
    }

    // The full-screen preview replaces everything — no input box, no footer,
    // no border — so the whole terminal is nothing but the selected entry's
    // text, cleanly selectable and copyable.
    if app.view == View::Preview {
        render_fullscreen_preview(frame, app, frame.area());
        return;
    }

    let input_active = app.focus == Focus::Input;
    let message_height = message_area_height(app).min(
        frame
            .area()
            .height
            .saturating_sub(2),
    );
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(message_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    match app.view {
        View::Events => render_list(frame, app, chunks[0]),
        View::Raw => render_raw(frame, app, chunks[0]),
        View::Log => render_log(frame, app, chunks[0]),
        View::Transcript => render_transcript_view(frame, app, chunks[0]),
        View::Driva => render_driva(frame, app, chunks[0]),
        View::Preview => unreachable!("handled above"),
    }
    if message_height > 0 {
        render_messages(frame, app, chunks[1]);
    }
    render_footer(frame, app, chunks[2]);

    if input_active {
        // The message box is modal while input has focus. Wash the completed
        // screen beneath it down to dark gray (and request terminal dimming)
        // so all text, borders, and status colors visibly recede. Then clear
        // and redraw the box itself at normal brightness.
        frame.render_widget(
            Block::default().style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            frame.area(),
        );
        let width = frame.area().width.saturating_sub(4).min(80);
        let height = input_area_height(app, width.saturating_sub(2));
        let area = Rect {
            x: frame.area().x + frame.area().width.saturating_sub(width) / 2,
            y: frame.area().y + frame.area().height.saturating_sub(height) / 2,
            width,
            height: height.min(frame.area().height),
        };
        frame.render_widget(Clear, area);
        render_input(frame, app, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use styra_server::event::{AgentEvent, TokenUsage};

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    /// The (x, y) of `needle`'s first character in the buffer. Column-based
    /// rather than a byte offset into a joined `String`: title rows carry
    /// multi-byte box-drawing and separator glyphs (`┌`, `·`, `●`) ahead of
    /// plain-ASCII text, so a byte offset from `str::find` would overshoot the
    /// actual column whenever the needle sits after one of those.
    fn find_column(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let needle_chars: Vec<char> = needle.chars().collect();
        for y in 0..buffer.area.height {
            let symbols: Vec<&str> = (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect();
            let found = (0..symbols.len()).find(|&start| {
                needle_chars.iter().enumerate().all(|(i, &ch)| {
                    symbols.get(start + i).and_then(|s| s.chars().next()) == Some(ch)
                })
            });
            if let Some(x) = found {
                return (x as u16, y);
            }
        }
        panic!("no cell contains {needle:?}");
    }

    #[test]
    fn header_shows_selection_and_status() {
        let app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        let screen = rendered(&app);
        assert!(screen.contains("styra"));
        assert!(screen.contains("codex"));
        assert!(screen.contains("running"));
    }

    #[test]
    fn header_shows_workspace_name_at_the_top_right() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.workspace_name = Some("payments".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();

        let (x, y) = find_column(buffer, "payments");
        assert_eq!(y, 0);
        assert_eq!(x, 80 - "payments".len() as u16 - 2);
    }

    #[test]
    fn header_prefers_the_session_name_to_the_workspace_name() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.workspace_name = Some("payments".into());
        app.session_name = Some("Fix retries".into());
        let screen = rendered(&app);
        assert!(screen.contains("Fix retries"));
    }

    #[test]
    fn event_list_header_indicates_conversation_only_filter() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.toggle_conversation_only();
        assert!(rendered(&app).contains("conversation only"));
    }

    #[test]
    fn header_shows_a_dot_indicating_running_vs_idle() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        assert!(rendered(&app).contains('●'));
        assert_eq!(status_color(&app.status), Color::Yellow);

        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert!(rendered(&app).contains("idle"));
        assert_eq!(status_color(&app.status), Color::Green);
    }

    #[test]
    fn header_text_stays_legible_when_the_panel_is_unfocused() {
        // An unstyled span only patches over whatever the block's border
        // already painted underneath it, so title text left unstyled would
        // inherit the border's `DarkGray` the moment the panel loses focus.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_input();
        assert_eq!(app.focus, Focus::Input);

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let (x, y) = find_column(&buffer, "styra");
        let cell = buffer.cell((x, y)).unwrap();
        assert_ne!(
            cell.style().fg,
            Some(Color::DarkGray),
            "title text must not inherit the dimmed unfocused border color"
        );
    }

    #[test]
    fn message_box_floats_in_the_center_of_the_primary_view() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_input();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();

        let (_, input_y) = find_column(buffer, "type a message, Enter to send");
        let (_, view_y) = find_column(buffer, "styra");
        assert_eq!(input_y, 9);
        assert!(
            input_y > view_y,
            "the message box should float over the primary view"
        );
    }

    #[test]
    fn message_box_dims_the_view_behind_it_but_stays_bright() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_input();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();

        let (view_x, view_y) = find_column(buffer, "styra");
        let view_style = buffer.cell((view_x, view_y)).unwrap().style();
        assert_eq!(view_style.fg, Some(Color::DarkGray));
        assert!(view_style.add_modifier.contains(Modifier::DIM));

        let (input_x, input_y) = find_column(buffer, "type a message, Enter to send");
        let input_style = buffer.cell((input_x, input_y)).unwrap().style();
        assert_eq!(input_style.fg, Some(Color::Gray));
        assert!(!input_style.add_modifier.contains(Modifier::DIM));
    }

    /// Every view's status line must name the model and effort in use, since
    /// the agent name alone does not say it and the session may be spending a
    /// model nobody typed.
    #[test]
    fn the_status_line_names_the_model_and_effort_in_use() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s-1",
        );
        assert!(rendered(&app).contains("styra · codex · gpt-5.6-sol · high"));

        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        });
        let screen = rendered(&app);
        assert!(
            screen.contains("styra · codex · gpt-5.6-sol · high · ● running"),
            "{screen}"
        );

        // Every other view carries the same status line, so switching away from
        // the event list does not lose it.
        let toggles: [fn(&mut App); 4] = [
            App::toggle_raw,
            |app| app.toggle_view(View::Log),
            |app| app.toggle_view(View::Transcript),
            |app| app.toggle_view(View::Driva),
        ];
        for toggle in toggles {
            toggle(&mut app);
            assert!(
                rendered(&app).contains("gpt-5.6-sol · high"),
                "{}",
                rendered(&app)
            );
            toggle(&mut app);
        }
    }

    /// A launch that pinned a model shows it before the agent confirms anything,
    /// but dimmed: it is what was asked for, not yet what is known to run.
    #[test]
    fn a_requested_model_is_shown_dimmed_until_the_agent_reports_one() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let (x, y) = find_column(&buffer, "opus");
        assert_eq!(buffer.cell((x, y)).unwrap().fg, Color::DarkGray);

        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "s-1".into(),
            model: Some("claude-opus-4-8".into()),
            effort: None,
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let (x, y) = find_column(&buffer, "claude-opus-4-8");
        assert_eq!(buffer.cell((x, y)).unwrap().fg, Color::White);
        // The launch's own effort stays alongside the reported model, still
        // dimmed: Claude Code never reports one, so it remains only what was
        // asked for.
        let (x, y) = find_column(&buffer, "max");
        assert_eq!(buffer.cell((x, y)).unwrap().fg, Color::DarkGray);
    }
}
