//! What the session was launched with — the isolation backend, the command it
//! runs, and the mount/network policy enforced around it — an answer to "what
//! can this agent touch" without having to go dig through `main.rs`.
//!
//! Before anything has launched the same fields describe the policy the next
//! interaction would start under, marked as planned so the two are not read as
//! the same claim. In that state the view is also where the policy is chosen:
//! the operator's own inputs are listed below the effective policy, separately,
//! because only those can be edited — the rest come from the profile, the
//! templates, and the sandbox broker.

use super::{render_placeholder, view_block};
use crate::app::{App, LaunchInputs};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use styra_server::{Mount, MountAccess};

pub(crate) fn render_driva(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some("driva"));

    let Some(options) = &app.driva_options else {
        render_placeholder(frame, block, area, "  no launch policy to describe");
        render_prompt(frame, app, area);
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
    ]);
    // Only meaningful for a launch that has not happened: on a live
    // interaction these are the *client's* inputs for the next one, while
    // everything else on screen is a record of the running sandbox. The
    // templates a live interaction did launch with are already visible in its
    // mounts below.
    if app.can_edit_launch() {
        lines.push(driva_field_line("templates", &templates_label(app)));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            "mounts",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    ]);
    lines.extend(options.mounts.iter().map(mount_line));

    if app.can_edit_launch() {
        lines.extend(editable_section(app));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
    render_prompt(frame, app, area);
}

/// The templates the next launch would layer, in the order they apply.
fn templates_label(app: &App) -> String {
    if app.launch.templates.is_empty() {
        "none".to_owned()
    } else {
        app.launch.templates.join(", ")
    }
}

/// The part of the policy this operator owns: the mounts they added, with a
/// cursor over them, and the keys that change any of it.
///
/// Kept separate from the effective mount list above rather than folded into
/// it, because those are the only rows `x` can remove — the workspace, the
/// profile's credential mounts, and the broker's control mount are not the
/// operator's to drop.
fn editable_section(app: &App) -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("added by you", heading)),
    ];
    if app.launch.mounts.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none — press m to add one",
            Style::default().fg(Color::Gray),
        )));
    } else {
        let selected = app
            .driva_selected_mount
            .min(app.launch.mounts.len().saturating_sub(1));
        lines.extend(app.launch.mounts.iter().enumerate().map(|(index, mount)| {
            let current = index == selected;
            Line::from(vec![
                Span::styled(
                    if current { "  • " } else { "    " },
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    LaunchInputs::mount_label(mount),
                    if current {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
            ])
        }));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  m add mount · x remove · T templates · w network · D save as default",
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

/// The "add a mount" prompt, floating over the view it edits.
fn render_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let Some(text) = &app.driva_prompt else {
        return;
    };
    let width = area.width.saturating_sub(4).min(72);
    let height = 3u16.min(area.height);
    let prompt = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" mount · source[:destination][:ro|rw] · Enter add · Esc cancel ");
    frame.render_widget(Clear, prompt);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(text.clone(), Style::default().fg(Color::White)),
            Span::styled("▏", Style::default().fg(Color::Yellow)),
        ]))
        .block(block),
        prompt,
    );
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
            app.launch.clone(),
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

    fn editable_app() -> App {
        use styra_server::DrivaOptions;

        let selection = styra_server::agent::Selection::parse("codex").unwrap();
        let mut app = App::pending(selection.clone());
        app.toggle_view(View::Driva);
        app.set_planned_driva_options(
            selection,
            app.launch.clone(),
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: false,
                mounts: Vec::new(),
            }),
        );
        app
    }

    fn tall(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 40)).unwrap();
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

    /// Before launch the view has to say what the operator can change, and
    /// keep their own mounts apart from the ones they cannot remove.
    #[test]
    fn a_not_yet_started_launch_lists_the_operators_own_inputs_and_the_keys_for_them() {
        let mut app = editable_app();
        let screen = tall(&app);
        assert!(screen.contains("templates none"), "{screen}");
        assert!(screen.contains("added by you"), "{screen}");
        assert!(screen.contains("none — press m to add one"), "{screen}");
        assert!(screen.contains("m add mount · x remove"), "{screen}");

        app.set_launch_templates(vec!["rust".into(), "browser".into()]);
        app.driva_prompt = Some("/srv/data:/mnt/data:rw".into());
        app.confirm_driva_prompt();
        let screen = tall(&app);
        assert!(screen.contains("rust, browser"), "{screen}");
        assert!(screen.contains("/srv/data → /mnt/data (rw)"), "{screen}");
    }

    /// A live session's policy is a record, so none of the editing chrome is
    /// drawn over it.
    #[test]
    fn a_live_launch_policy_offers_no_editing() {
        use styra_server::DrivaOptions;

        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.toggle_view(View::Driva);
        app.set_driva_options(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: Vec::new(),
        });
        let screen = tall(&app);
        assert!(!screen.contains("added by you"), "{screen}");
        assert!(!screen.contains("add mount"), "{screen}");
    }

    #[test]
    fn the_mount_prompt_floats_over_the_policy_it_edits() {
        let mut app = editable_app();
        app.open_driva_prompt();
        app.driva_prompt = Some("/srv/data".into());
        let screen = tall(&app);
        assert!(screen.contains("source[:destination][:ro|rw]"), "{screen}");
        assert!(screen.contains("/srv/data"), "{screen}");
    }
}
