//! The one-line footer with the keyboard shortcut reference and workspace.

use super::palette;
use crate::app::App;
use crate::keymap::HELP;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

/// Now, in milliseconds since the epoch — the clock a quota window's reset is
/// read against, so a window that has already turned over stops warning.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let working_directory = app
        .workspace
        .working_directory_or_current()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let worktrees = format!(
        " W worktrees: {} ",
        if app.workspace.worktrees_enabled {
            "ON"
        } else {
            "OFF"
        }
    );
    let worktrees_width = worktrees.width().min(area.width as usize) as u16;
    let idle_count = app.interactions.idle_notification_count();
    let idle_notice = (idle_count > 0).then(|| {
        format!(
            " a {idle_count} interaction{} idle ",
            if idle_count == 1 { "" } else { "s" }
        )
    });
    let idle_notice_width = idle_notice
        .as_deref()
        .map(UnicodeWidthStr::width)
        .unwrap_or_default()
        .min(area.width.saturating_sub(worktrees_width) as usize)
        as u16;
    // The quota warning rides the footer because what it says is true of the
    // account rather than of the view: a plan window filling up is worth
    // knowing in the event list, the raw view, or anywhere else, and the footer
    // is the one line every view keeps. `Q` opens the readings behind it.
    let quota_alert = super::quota::alert(app, now_ms());
    let quota_width = quota_alert
        .as_ref()
        .map(Line::width)
        .unwrap_or_default()
        .min(area.width.saturating_sub(worktrees_width) as usize) as u16;
    // Waiting out a rate limit is the session's standing answer, kept by the
    // server and acted on long after the quota view was closed. It belongs
    // next to the quota warning for the same reason that warning is here: the
    // operator who needs to know it is armed is watching the interaction, not
    // the readings. Only the non-default "on" takes footer space — every
    // session starts off, and the quota view's own title spells out both.
    let retry_notice = app.auto_retry.then_some(" R rate-limit retry: on ");
    let retry_width = retry_notice
        .map(UnicodeWidthStr::width)
        .unwrap_or_default()
        .min(
            area.width
                .saturating_sub(worktrees_width)
                .saturating_sub(quota_width) as usize,
        ) as u16;
    let directory_width = working_directory.width().min(
        area.width
            .saturating_sub(worktrees_width)
            .saturating_sub(idle_notice_width)
            .saturating_sub(retry_width)
            .saturating_sub(quota_width) as usize,
    ) as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(quota_width),
            Constraint::Length(retry_width),
            Constraint::Length(idle_notice_width),
            Constraint::Length(worktrees_width),
            Constraint::Length(directory_width),
        ])
        .split(area);

    let keybinds = Paragraph::new(Line::from(Span::styled(
        format!(" {HELP} keybinds"),
        Style::default().fg(palette::MUTED_TEXT),
    )));
    let directory = Paragraph::new(Line::from(Span::styled(
        working_directory,
        Style::default().fg(palette::ADDITIONAL_INFO),
    )))
    .right_aligned();
    let worktrees = Paragraph::new(Line::from(Span::styled(
        worktrees,
        Style::default().fg(if app.workspace.worktrees_enabled {
            palette::SUCCESS
        } else {
            palette::INACTIVE
        }),
    )))
    .right_aligned();
    frame.render_widget(keybinds, chunks[0]);
    if let Some(quota_alert) = quota_alert {
        frame.render_widget(Paragraph::new(quota_alert).right_aligned(), chunks[1]);
    }
    if let Some(retry_notice) = retry_notice {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                retry_notice,
                Style::default().fg(palette::SUCCESS),
            )))
            .right_aligned(),
            chunks[2],
        );
    }
    if let Some(idle_notice) = idle_notice {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                idle_notice,
                Style::default().fg(palette::SUCCESS),
            )))
            .right_aligned(),
            chunks[3],
        );
    }
    frame.render_widget(worktrees, chunks[4]);
    frame.render_widget(directory, chunks[5]);
}

pub(crate) fn tag_color(tag: &str) -> Color {
    match tag {
        "agent" => palette::AGENT_TAG,
        "user" => palette::USER_TAG,
        "shell" => palette::SHELL_TAG,
        "tool" => palette::SPECIAL,
        "plan" | "files" => palette::INFO,
        // A branch marker is a link the operator can act on (`b`), so it is
        // colored as an accent rather than as passing information.
        "branch" => palette::ACCENT,
        "error" | "malformed" => palette::ERROR,
        _ => palette::ADDITIONAL_INFO,
    }
}

/// A very light tint for conversational prose. These stay close to the
/// default foreground so messages are distinguishable without competing with
/// the stronger colors reserved for status and errors.
pub(crate) fn message_text_color(tag: &str) -> Color {
    match tag {
        "agent" => palette::AGENT_TEXT,
        "user" => palette::USER_TEXT,
        _ => palette::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use styra_server::{DrivaOptions, InteractionActivity, InteractionSummary};

    fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
            id: id.into(),
            name: None,
            workspace_id: "workspace".into(),
            selection: styra_server::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                base: vec![],
                mounts: vec![],
            },
            accepting: true,
            activity,
            idle_unseen: false,
            last_message: None,
            events: 0,
        }
    }

    #[test]
    fn footer_shows_keybinds_and_working_directory() {
        let mut app = testing::app("s1");
        app.workspace.enter("/tmp/styra/workspace".into());
        let screen = rendered(&app);
        assert!(screen.contains("? keybinds"));
        assert!(screen.contains("/tmp/styra/workspace"));
        assert!(screen.contains("W worktrees: OFF"));
        assert!(!screen.contains("j/k next/prev"));
    }

    #[test]
    fn footer_makes_enabled_worktree_creation_visible() {
        let mut app = testing::app("s1");
        app.workspace.worktrees_enabled = true;

        assert!(rendered(&app).contains("W worktrees: ON"));
    }

    /// An armed rate-limit retry has to be visible from the interaction the
    /// operator is watching: it was set once, in a view they have since left,
    /// and it changes what happens hours later.
    #[test]
    fn footer_reports_an_armed_rate_limit_retry() {
        let mut app = testing::app("s1");
        assert!(
            !rendered(&app).contains("rate-limit retry"),
            "off is the default and takes no footer space"
        );

        app.auto_retry = true;

        assert!(rendered(&app).contains("R rate-limit retry: on"));
    }

    #[test]
    fn footer_reports_interactions_that_became_idle_while_unseen() {
        let mut app = testing::app("current");
        app.interactions.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("other", InteractionActivity::Running),
            ],
            vec![],
        );
        app.interactions.close();
        let mut other = interaction("other", InteractionActivity::Pending);
        other.idle_unseen = true;
        app.interactions.refresh(vec![
            interaction("current", InteractionActivity::Running),
            other,
        ]);

        assert!(rendered(&app).contains("a 1 interaction idle"));
    }

    #[test]
    fn working_directory_is_aligned_to_the_bottom_right() {
        let mut app = testing::app("s1");
        app.workspace.enter("/workspace".into());
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer();

        let bottom_row: String = (0..40)
            .map(|x| buffer.cell((x, 9)).unwrap().symbol())
            .collect();
        assert!(bottom_row.ends_with("/workspace"));
    }
}
