//! Terminal rendering of [`App`] with ratatui.
//!
//! The event list (each entry a summary line that grows inline when expanded),
//! messages, and a one-line status/help footer, with the message box floating
//! over the center while active.
//! Rendering is a pure function of `App`; all state lives in [`crate::app`].

mod answer;
mod driva;
mod files;
mod footer;
mod help;
mod input;
mod insert;
mod interactions;
mod launcher;
mod list;
mod log;
mod markdown;
mod messages;
mod modal_input;
mod notes;
mod palette;
mod picker;
mod preview;
pub(crate) mod quota;
mod raw;
#[cfg(test)]
mod testing;
mod transcript;

use answer::render_answer;
use driva::render_driva;
use files::render_files;
use footer::render_footer;
pub(crate) use footer::{message_text_color, tag_color};
use help::render_keybinds;
use input::render_input;
use insert::render_insert;
use launcher::render_launcher;
use list::render_list;
pub(crate) use list::{summary_line, wrap_line};
use log::render_log;
use messages::{message_area_height, render_messages};
use notes::render_notes;
pub use notes::render_notes_prompt;
pub(crate) use picker::short_id;
pub use picker::{
    render_message_popup, render_name_prompt, render_picker, render_template_picker,
    render_workspace_picker, Preview, SessionsPreview,
};
pub(crate) use preview::preview_scroll_limit;
use preview::{render_fullscreen_preview, render_preview};
use quota::render_quota;
use raw::render_raw;
use transcript::render_transcript_view;

use crate::app::{App, Focus, LaunchLabel, Status, View};
use crate::insert::Prompt;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::time::Duration;

/// Braille spinner frames shared by every view that represents active agent
/// work. The phase advances only when an agent event arrives.
const RUNNING_INDICATOR: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn running_indicator(events: usize) -> &'static str {
    RUNNING_INDICATOR[events % RUNNING_INDICATOR.len()]
}

/// Cap on detail lines shown for one expanded entry, so a single noisy command
/// cannot bury the rest of the session.
const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";
/// A duration in the compact form the status line and tail use: `12s`,
/// `2m14s`, `1h04m`. Seconds are dropped past an hour, where they no longer
/// tell the operator anything they are waiting on.
pub(crate) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// Color coding for the status dot, so running vs. idle for input reads at
/// a glance instead of requiring the operator to read the label text.
fn status_color(status: &Status) -> Color {
    match status {
        Status::Pending => palette::INFO,
        Status::Running => palette::WARNING,
        Status::Idle => palette::SUCCESS,
        // Idle, but with work still running behind it: closer to idle than to
        // a turn in flight, and distinct from both.
        Status::Background => palette::MUTED_WARNING,
        Status::Stopped => palette::INACTIVE,
        Status::Ended { error: Some(_), .. } => palette::ERROR,
        Status::Ended { .. } => palette::INACTIVE,
    }
}

/// Build a block title of the form
/// " workspace · agent · model · effort · ● status[ · suffix] ".
///
/// The leading slot names the Workspace rather than the program: which
/// Workspace this session belongs to is what an operator running several at
/// once needs from a glance at the top-left, and the program's own name told
/// them nothing they did not already know.
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
fn title_line(
    label: &LaunchLabel,
    workspace: Option<&str>,
    status: &Status,
    elapsed: Option<String>,
    suffix: Option<&str>,
) -> Line<'static> {
    let color = status_color(status);
    let text_style = Style::default().fg(palette::MUTED_TEXT);
    let value_style = |reported: bool| {
        if reported {
            Style::default().fg(palette::TEXT)
        } else {
            Style::default().fg(palette::ADDITIONAL_INFO)
        }
    };
    // Bold near-white rather than the cyan the Workspace name used to wear at
    // the top right: on the border line, cyan on black is the least legible
    // thing in the title, and this is now the first word of it.
    let mut spans = vec![Span::raw(" ")];
    if let Some(workspace) = workspace {
        spans.push(Span::styled(
            workspace.to_owned(),
            Style::default()
                .fg(palette::TEXT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", text_style));
    }
    spans.push(Span::styled(format!("{} · ", label.agent), text_style));
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
    // How long that status has held. The event list's status tail says this at
    // more length, but only the list has one: in the raw, log, or files view
    // the title is the only place a long-running turn can show it is alive.
    if let Some(elapsed) = elapsed {
        spans.push(Span::styled(
            format!(" {elapsed}"),
            Style::default().fg(color),
        ));
    }
    spans.push(match suffix {
        Some(suffix) => Span::styled(format!(" · {suffix} "), text_style),
        None => Span::styled(" ", text_style),
    });
    Line::from(spans)
}

/// How long the current status has held, for the views' shared title — or
/// `None` where the figure would say nothing: before a launch, once the
/// process has ended, and while idle — none of those is a state the operator
/// is waiting out, so how long it has held is not worth counting.
fn status_elapsed(app: &App) -> Option<String> {
    match app.status {
        Status::Running | Status::Background | Status::Stopped => {
            Some(format_duration(app.progress().in_status))
        }
        Status::Pending | Status::Idle | Status::Ended { .. } => None,
    }
}

/// The chrome every full-region view wears: a border that brightens when the
/// list has focus, the session's status title (opening with the Workspace
/// name), and the Session name at the top right. `suffix` names the view in
/// the title; `None` is the event list, which is the default view and so
/// needs no name.
fn view_block(app: &App, suffix: Option<&str>) -> Block<'static> {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(palette::ACCENT)
    } else {
        Style::default().fg(palette::INACTIVE)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(
            &app.launch_label(),
            app.workspace_name.as_deref(),
            &app.status,
            status_elapsed(app),
            suffix,
        ));
    if let Some(title) = session_title(app) {
        block = block.title(title);
    }
    if let Some(title) = notes_title(app) {
        block = block.title_bottom(title);
    }
    block
}

/// Marker shown while this Session or its Workspace has notes. Without it the
/// notes would be invisible until someone thought to press `E`, which is no
/// use for a note written to be found again later.
fn notes_title(app: &App) -> Option<Line<'static>> {
    app.notes.any().then(|| {
        Line::from(Span::styled(
            " ✎ notes · E ",
            Style::default().fg(palette::WARNING),
        ))
        .right_aligned()
    })
}

/// A view's empty state: one muted line saying why there is nothing to show,
/// inside that view's own block so the chrome stays put as content arrives.
fn render_placeholder(frame: &mut Frame, block: Block<'static>, area: Rect, text: &str) {
    let paragraph = Paragraph::new(Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(palette::MUTED_TEXT),
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
            .fg(palette::ACCENT)
            .add_modifier(Modifier::BOLD),
    )))
}

/// Right-aligned title for the primary panel's top border: the Session name,
/// once the Session has earned one. The Workspace name used to lead this title
/// but now opens the left-hand one, so repeating it here would only spend
/// border width saying the same thing twice.
fn session_title(app: &App) -> Option<Line<'static>> {
    let session = app.session_name.as_deref()?;
    Some(
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                session.to_owned(),
                Style::default()
                    .fg(palette::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ])
        .right_aligned(),
    )
}

pub fn render(frame: &mut Frame, app: &App) {
    if app.show_keybinds {
        render_keybinds(frame, frame.area(), app.keybinds_scroll);
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
        // Notes are reachable from every view, this one included, so the editor
        // still has to be drawn over it.
        if let Some(editor) = app.notes.editor() {
            render_notes(frame, app, editor);
        }
        return;
    }

    // The notes editor is modal too, but it floats over the view it was opened
    // from rather than replacing it, so the view is drawn first and the editor
    // over it at the end of this function.
    let input_active = app.focus == Focus::Input && !app.notes.is_open();
    let message_height = message_area_height(app).min(frame.area().height.saturating_sub(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(message_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    match app.view {
        View::Events if app.interactions.open => {
            let panes = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(interactions::height(app, chunks[0].height)),
                    Constraint::Min(1),
                ])
                .split(chunks[0]);
            interactions::render(frame, app, panes[0]);
            render_list(frame, app, panes[1]);
        }
        View::Events => render_list(frame, app, chunks[0]),
        View::Raw => render_raw(frame, app, chunks[0]),
        View::Log => render_log(frame, app, chunks[0]),
        View::Quota => render_quota(frame, app, chunks[0]),
        View::Transcript => render_transcript_view(frame, app, chunks[0]),
        View::Driva => render_driva(frame, app, chunks[0]),
        View::Files => render_files(frame, app, chunks[0]),
        View::Answer => render_answer(frame, app, chunks[0]),
        View::Preview => unreachable!("handled above"),
    }
    if message_height > 0 {
        render_messages(frame, app, chunks[1]);
    }
    render_footer(frame, app, chunks[2]);

    // The message box is modal while input has focus: it dims the finished
    // screen and floats over the middle of it.
    if input_active {
        render_input(frame, app);
    }

    if let Some(editor) = app.notes.editor() {
        render_notes(frame, app, editor);
    }

    // Last, because it is the innermost modal: it is opened from the message
    // box and floats over it, and it holds the terminal cursor while it does.
    render_insert(frame, app.insert.as_ref().map(Prompt::state), frame.area());
}

#[cfg(test)]
mod tests {
    use super::testing::{self, rendered};
    use super::*;
    use styra_server::event::{AgentEvent, TokenUsage};

    #[test]
    fn header_shows_selection_and_status() {
        let title = testing::screen(&testing::app("s1")).title();
        // Scoped to the title row, and stated positively. The old form was
        // `!rendered(..).contains("styra")` over the flattened buffer, meant
        // to say the title no longer opens with the program's name — but the
        // footer renders the host's working directory, so it also failed in
        // any checkout whose path happened to contain the word.
        assert!(title.starts_with("┌ codex · "), "{title}");
        assert!(title.contains("running"), "{title}");
    }

    #[test]
    fn header_opens_with_the_workspace_name_at_the_top_left() {
        let mut app = testing::app("s1");
        app.workspace_name = Some("payments".into());
        let screen = testing::screen(&app);

        // Spelled as the row it produces rather than as a column number: what
        // the test means is that the workspace name comes first, ahead of the
        // agent, and a bare `(x, y)` says that only by arithmetic over the
        // border and the title's leading pad.
        assert!(
            screen.title().starts_with("┌ payments · codex · "),
            "{}",
            screen.title()
        );

        let (x, y) = screen.find("payments");
        // Readable against the border line, unlike the cyan it used to wear.
        let style = screen.buffer().cell((x, y)).unwrap().style();
        assert_eq!(style.fg, Some(palette::TEXT));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn header_shows_the_workspace_name_alongside_the_session_name() {
        let mut app = testing::app("s1");
        app.workspace_name = Some("payments".into());
        app.session_name = Some("Fix retries".into());
        let screen = rendered(&app);
        assert!(screen.contains("Fix retries"));
        assert!(screen.contains("payments"));
    }

    #[test]
    fn event_list_header_indicates_conversation_only_filter() {
        let mut app = testing::app("s1");
        app.timeline.conversation_only = true;
        assert!(rendered(&app).contains("conversation only"));
    }

    #[test]
    fn header_shows_a_dot_indicating_running_vs_idle() {
        let mut app = testing::app("s1");
        assert!(rendered(&app).contains('●'));
        assert_eq!(status_color(&app.status), palette::WARNING);

        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert!(rendered(&app).contains("idle"));
        assert_eq!(status_color(&app.status), palette::SUCCESS);
    }

    #[test]
    fn header_text_has_a_style_independent_of_the_panel_border() {
        // Explicit span styles keep the title independent of the block border.
        // The rendered view is intentionally dimmed later when the modal input
        // box opens, so inspect the title before that overlay is applied.
        let app = testing::app("s1");
        let title = title_line(
            &app.launch_label(),
            Some("payments"),
            &app.status,
            None,
            None,
        );
        assert_eq!(title.spans[1].style.fg, Some(palette::TEXT));
        assert_eq!(title.spans[2].style.fg, Some(palette::MUTED_TEXT));
    }

    #[test]
    fn message_box_floats_in_the_center_of_the_primary_view() {
        let mut app = testing::app("s1");
        app.enter_input();

        let screen = testing::screen(&app);
        let (_, input_y) = screen.find("type a message, Enter to send");
        let (_, view_y) = screen.find("codex");
        assert_eq!(input_y, 9);
        assert!(
            input_y > view_y,
            "the message box should float over the primary view"
        );
    }

    #[test]
    fn message_box_dims_the_view_behind_it_but_stays_bright() {
        let mut app = testing::app("s1");
        app.enter_input();

        let screen = testing::screen(&app);
        let (view_x, view_y) = screen.find("codex");
        let view_style = screen.buffer().cell((view_x, view_y)).unwrap().style();
        assert_eq!(view_style.fg, Some(palette::MODAL_BACKDROP));
        assert!(view_style.add_modifier.contains(Modifier::DIM));

        let (input_x, input_y) = screen.find("type a message, Enter to send");
        let input_style = screen.buffer().cell((input_x, input_y)).unwrap().style();
        assert_eq!(input_style.fg, Some(palette::MUTED_TEXT));
        assert!(!input_style.add_modifier.contains(Modifier::DIM));
    }

    /// Every view's status line must name the model and effort in use, since
    /// the agent name alone does not say it and the session may be spending a
    /// model nobody typed.
    #[test]
    fn the_status_line_names_the_model_and_effort_in_use() {
        let mut app = testing::app("s-1");
        let expected = format!(" codex · {} · {}", testing::MODEL, testing::EFFORT);
        assert!(rendered(&app).contains(&expected), "{}", rendered(&app));

        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some(testing::MODEL.into()),
            effort: Some(testing::EFFORT.into()),
        });
        let screen = rendered(&app);
        assert!(
            screen.contains(&format!("{expected} · ● running")),
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
            let named = format!("{} · {}", testing::MODEL, testing::EFFORT);
            assert!(rendered(&app).contains(&named), "{}", rendered(&app));
            toggle(&mut app);
        }
    }

    /// A launch that pinned a model shows it before the agent confirms anything,
    /// but dimmed: it is what was asked for, not yet what is known to run.
    #[test]
    fn a_requested_model_is_shown_dimmed_until_the_agent_reports_one() {
        let mut app = testing::app_with("claude:opus/max", "s-1");
        let screen = testing::screen(&app);
        let (x, y) = screen.find("opus");
        assert_eq!(
            screen.buffer().cell((x, y)).unwrap().fg,
            palette::ADDITIONAL_INFO
        );

        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "s-1".into(),
            model: Some("claude-opus-4-8".into()),
            effort: None,
        });
        let screen = testing::screen(&app);
        let (x, y) = screen.find("claude-opus-4-8");
        assert_eq!(screen.buffer().cell((x, y)).unwrap().fg, palette::TEXT);
        // The launch's own effort stays alongside the reported model, still
        // dimmed: Claude Code never reports one, so it remains only what was
        // asked for.
        let (x, y) = screen.find("max");
        assert_eq!(
            screen.buffer().cell((x, y)).unwrap().fg,
            palette::ADDITIONAL_INFO
        );
    }
}
