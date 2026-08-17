//! What the session was launched with — the isolation backend, the command it
//! runs, and the mount/network policy enforced around it — an answer to "what
//! can this agent touch" without having to go dig through `main.rs`.
//!
//! Before anything has launched the same fields describe the policy the next
//! interaction would start under, marked as planned so the two are not read as
//! the same claim.

use super::{render_placeholder, view_block};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use styra_server::{Mount, MountAccess};

pub(crate) fn render_driva(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some("driva"));

    let Some(options) = &app.driva_options else {
        render_placeholder(frame, block, area, "  no launch policy to describe");
        return;
    };

    let mut lines = Vec::new();
    // Before launch this is a plan, not a record: say so, so an operator does
    // not read it as the sandbox some agent is already running in.
    if app.driva_planned {
        lines.push(Line::from(Span::styled(
            "  planned — applied when the next interaction starts",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
    }
    lines.extend([
        driva_field_line("backend", &options.isolation_backend),
        driva_field_line("command", &options.command.join(" ")),
        driva_field_line("workdir", &options.working_directory.display().to_string()),
        driva_field_line("network", if options.network { "on" } else { "off" }),
        Line::from(""),
        Line::from(Span::styled(
            "mounts",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    ]);
    lines.extend(options.mounts.iter().map(mount_line));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn driva_field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<8} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_owned(), Style::default().fg(Color::White)),
    ])
}

fn mount_line(mount: &Mount) -> Line<'static> {
    match mount {
        Mount::Bind {
            source,
            destination,
            access,
        } => {
            let (label, color) = match access {
                MountAccess::ReadWrite => ("rw", Color::Yellow),
                MountAccess::ReadOnly => ("ro", Color::Gray),
            };
            Line::from(vec![
                Span::styled(
                    format!("  {label} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} → {}", source.display(), destination.display()),
                    Style::default().fg(Color::White),
                ),
            ])
        }
        Mount::Temporary { destination } => Line::from(vec![
            Span::styled(
                "  tmp ",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                destination.display().to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
        Mount::Overlay {
            source,
            destination,
        } => Line::from(vec![
            Span::styled(
                "  ovl ",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} → {}", source.display(), destination.display()),
                Style::default().fg(Color::White),
            ),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::View;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn driva_view_shows_the_launch_policy_or_a_placeholder_before_launch() {
        use styra_server::DrivaOptions;
        use styra_server::{Mount, MountAccess};

        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.toggle_view(View::Driva);
        let placeholder = rendered(&app);
        assert!(placeholder.contains("no launch policy"));

        app.set_driva_options(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into(), "app-server".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: vec![Mount::Bind {
                source: PathBuf::from("/home/op/project"),
                destination: PathBuf::from("/tmp/styra/workspace"),
                access: MountAccess::ReadWrite,
            }],
        });
        let screen = rendered(&app);
        assert!(screen.contains("driva"));
        assert!(screen.contains("bwrap"));
        assert!(screen.contains("codex app-server"));
        assert!(screen.contains("off"));
        assert!(screen.contains("/home/op/project"));
        assert!(screen.contains("/tmp/styra/workspace"));
        assert!(!screen.contains("planned"));
    }

    #[test]
    fn driva_view_marks_the_policy_a_not_yet_started_interaction_would_launch_under() {
        use styra_server::DrivaOptions;

        let selection = styra_server::agent::Selection::parse("codex").unwrap();
        let mut app = App::new(selection.clone(), "s1");
        app.toggle_view(View::Driva);
        app.set_planned_driva_options(
            selection,
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into(), "app-server".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: true,
                mounts: Vec::new(),
            }),
        );
        let screen = rendered(&app);
        assert!(screen.contains("planned — applied when the next interaction starts"));
        assert!(screen.contains("codex app-server"));
        assert!(screen.contains("network  on"));
    }
}
