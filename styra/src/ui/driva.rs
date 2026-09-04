//! The server's general account of the current Workspace and Interaction:
//! durable Workspace metadata, live Interaction state, and the Driva sandbox
//! it runs in (or would run in before the first message).
//!
//! Before anything has launched the same fields describe the policy the next
//! interaction would start under, marked as planned so the two are not read as
//! the same claim. In that state the view is also where the policy is chosen.
//!
//! What is chosen is two settings, not one, and they are shown as two: the
//! Workspace's standing policy applies to every launch here and outlives every
//! interaction in it, while this interaction's own settings are layered over it
//! and go when it does. Each gets its own pane, with the same three rows in the
//! same order, so the difference between them is which pane a grant sits in and
//! nothing else. `Tab` moves the editing keys between the panes and the focused
//! one says so; every other key acts on whichever that is, so there is one set
//! of keys rather than one per layer.
//!
//! Above both is the policy those two resolve to, which is what the agent
//! actually gets — including the parts neither pane can change: the workspace
//! mount, the profile's credential mounts, the broker's control mount.

use super::{palette, view_block};
use crate::app::App;
use crate::launch::LaunchScope;
use crate::mount;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use styra_server::{AttributedMount, DrivaOptions, Mount, MountAccess, MountOrigin};

/// Rows the effective-policy summary keeps for itself before either settings
/// pane is given any height. Enough for the banner and the fields that say what
/// is about to run; the mounts below them are what a short terminal loses.
const SUMMARY_MIN_HEIGHT: u16 = 5;

/// Width of the label column inside a settings pane. Wide enough for
/// `templates`, and identical in both panes so the two read as one form.
const SETTING_LABEL: usize = 10;

pub(crate) fn render_driva(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some("details"));
    let options = app.launch.driva.as_ref();

    // A live interaction's policy is a record: there is nothing to choose, so
    // the panes and their keys are not drawn over it at all.
    if !app.can_edit_launch() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_summary(frame, app, options, inner);
        render_prompt(frame, app, area);
        return;
    }

    let mut rest = block.inner(area);
    frame.render_widget(block, area);

    // Carved from the bottom: the panes and the keys for them are the point of
    // this screen while it is editable, and each is given only what is left
    // above the summary's own floor, so a short terminal drops the ends of the
    // mount list rather than the settings being edited.
    let hints = hint_lines(app);
    let workspace = pane_rows(app, LaunchScope::Workspace);
    let interaction = pane_rows(app, LaunchScope::Interaction);
    let hint_area = take_bottom(&mut rest, hints.len() as u16, 1);
    let interaction_area = take_bottom(&mut rest, interaction.len() as u16 + 2, SUMMARY_MIN_HEIGHT);
    let workspace_area = take_bottom(&mut rest, workspace.len() as u16 + 2, SUMMARY_MIN_HEIGHT);

    render_summary(frame, app, options, rest);
    render_pane(
        frame,
        app,
        LaunchScope::Workspace,
        workspace,
        workspace_area,
    );
    render_pane(
        frame,
        app,
        LaunchScope::Interaction,
        interaction,
        interaction_area,
    );
    frame.render_widget(Paragraph::new(hints), hint_area);
    render_prompt(frame, app, area);
}

/// Take `wanted` rows off the bottom of `area`, leaving at least `floor` there,
/// and shrink `area` by what was taken. A zero-height result is a pane there was
/// no room for; rendering it draws nothing.
fn take_bottom(area: &mut Rect, wanted: u16, floor: u16) -> Rect {
    let height = wanted.min(area.height.saturating_sub(floor));
    area.height -= height;
    Rect {
        x: area.x,
        y: area.y + area.height,
        width: area.width,
        height,
    }
}

/// The policy the two settings panes resolve to: what the sandbox will actually
/// be, including everything neither pane can change.
fn render_summary(frame: &mut Frame, app: &App, options: Option<&DrivaOptions>, area: Rect) {
    let workspace = workspace_lines(app);
    let interaction = interaction_lines(app);
    let mut sandbox_area = area;

    // The two objects are peers in this overview. Columns keep their complete
    // metadata from pushing the sandbox below the policy editors; narrow
    // terminals fall back to a readable vertical sequence.
    if area.width >= 72 {
        let left_width = area.width / 2;
        let right_width = area.width.saturating_sub(left_width);
        let workspace = Paragraph::new(workspace).wrap(Wrap { trim: false });
        let interaction = Paragraph::new(interaction).wrap(Wrap { trim: false });
        let height = workspace
            .line_count(left_width.max(1))
            .max(interaction.line_count(right_width.max(1)))
            .min(usize::from(area.height)) as u16;
        let overview = Rect { height, ..area };
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(overview);
        frame.render_widget(workspace, columns[0]);
        frame.render_widget(interaction, columns[1]);
        sandbox_area.y += height;
        sandbox_area.height = sandbox_area.height.saturating_sub(height);
    } else {
        let mut overview = workspace;
        overview.push(Line::from(""));
        overview.extend(interaction);
        let overview = Paragraph::new(overview).wrap(Wrap { trim: false });
        let height = overview
            .line_count(area.width.max(1))
            .min(usize::from(area.height)) as u16;
        frame.render_widget(overview, Rect { height, ..area });
        sandbox_area.y += height;
        sandbox_area.height = sandbox_area.height.saturating_sub(height);
    }
    frame.render_widget(
        Paragraph::new(sandbox_lines(app, options)).wrap(Wrap { trim: false }),
        sandbox_area,
    );
}

fn sandbox_lines(app: &App, options: Option<&DrivaOptions>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(""), section_line("sandbox")];

    let Some(options) = options else {
        lines.push(detail_field_line(
            "state",
            "unavailable — the server has not resolved a launch policy",
        ));
        return lines;
    };
    // Before launch this is a plan, not a record: say so, so an operator does
    // not read it as the sandbox some agent is already running in.
    if app.launch.planned {
        lines.push(Line::from(Span::styled(
            "  planned — applied when the next interaction starts",
            Style::default().fg(palette::WARNING),
        )));
        lines.push(Line::from(""));
    }
    lines.extend([
        driva_field_line("backend", &options.isolation_backend),
        driva_field_line("command", &options.command.join(" ")),
        driva_field_line("workdir", &options.working_directory.display().to_string()),
        driva_field_line("network", &network_label(app, options.network)),
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
            if app.can_edit_launch() {
                "effective mounts"
            } else {
                "mounts"
            },
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
    ]);
    lines.extend(grouped_mount_lines(&options.mounts));
    lines
}

/// The complete durable Workspace snapshot retained by the client. These are
/// all fields the server's `WorkspaceSummary` reports, rather than only the
/// name and worktree toggle that happen to be used elsewhere in the UI.
fn workspace_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![section_line("Workspace")];
    lines.push(detail_field_line(
        "identity",
        &format!(
            "{} · {}",
            match (&app.workspace.given_name, &app.workspace.name) {
                (Some(name), _) => name.clone(),
                (None, Some(display)) => format!("unnamed (shown as {display})"),
                (None, None) => "unknown".into(),
            },
            app.workspace.id.as_deref().unwrap_or("unknown")
        ),
    ));
    lines.push(detail_field_line(
        "host path",
        &app.workspace
            .host_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unknown".into()),
    ));
    lines.push(detail_field_line(
        "git repo",
        &app.workspace
            .git_repository
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".into()),
    ));
    lines.push(detail_field_line(
        "server path",
        &app.workspace
            .server_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unknown".into()),
    ));
    let sessions = app
        .workspace
        .session_count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "unknown".into());
    lines.push(detail_field_line(
        "created",
        &match (&app.workspace.age, app.workspace.created_at_ms) {
            (Some(age), Some(at)) => format!("{age} · {at} ms"),
            (Some(age), None) => age.clone(),
            (None, Some(at)) => format!("{at} ms"),
            (None, None) => "unknown".into(),
        },
    ));
    lines.push(detail_field_line(
        "tracked",
        &format!(
            "{sessions} session(s) · accessed {} ms",
            app.workspace
                .last_accessed_at_ms
                .map(|at| at.to_string())
                .unwrap_or_else(|| "unknown".into())
        ),
    ));
    let standing = &app.launch.workspace;
    lines.push(detail_field_line(
        "capabilities",
        &format!(
            "worktrees {} · network {} · {} template(s) · {} mount(s)",
            if app.workspace.worktrees_enabled {
                "on"
            } else {
                "off"
            },
            if standing.grants_network() {
                "on"
            } else {
                "off"
            },
            standing.templates.len(),
            standing.mounts.len()
        ),
    ));
    lines
}

/// The current server Interaction projected through the state the client keeps
/// synchronized from its summary and update stream.
fn interaction_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![section_line("current interaction")];
    if app.session_id.is_empty() {
        lines.push(detail_field_line("state", "none — not started"));
        lines.push(detail_field_line("profile", &app.selection.name()));
        return lines;
    }
    lines.extend([
        detail_field_line(
            "identity",
            &format!(
                "{} · {}",
                app.session_name.as_deref().unwrap_or("unnamed"),
                app.session_id
            ),
        ),
        detail_field_line(
            "Workspace id",
            app.workspace.id.as_deref().unwrap_or("unknown"),
        ),
        detail_field_line("profile", &app.selection.name()),
        detail_field_line("status", &app.activity.status.label()),
        detail_field_line(
            "accepting",
            if matches!(
                app.activity.status,
                crate::activity::Status::Running
                    | crate::activity::Status::Idle
                    | crate::activity::Status::Background
            ) {
                "yes"
            } else {
                "no"
            },
        ),
        detail_field_line(
            "workspace",
            &app.workspace
                .root()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".into()),
        ),
        detail_field_line(
            "working dir",
            &app.workspace
                .working_directory_or_current()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".into()),
        ),
        detail_field_line("queued", &app.outbox.queued_count().to_string()),
    ]);
    if let Some(message) = app.timeline.entries.iter().rev().find_map(|entry| {
        matches!(
            entry.event,
            styra_server::event::AgentEvent::AgentMessage { .. }
        )
        .then(|| entry.event.summary())
    }) {
        lines.push(detail_field_line("last message", &message));
    }
    lines
}

fn section_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_owned(),
        Style::default()
            .fg(palette::ACCENT)
            .add_modifier(Modifier::BOLD),
    ))
}

fn detail_field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<13} "),
            Style::default().fg(palette::ADDITIONAL_INFO),
        ),
        Span::styled(value.to_owned(), Style::default().fg(palette::TEXT)),
    ])
}

/// The effective mounts under a heading per layer that contributed them.
///
/// Flat, the list answers "what can the agent touch" but not "why", and the two
/// questions are asked together: a grant an operator does not recognize is
/// either the profile's doing, a template's, or their own, and only the last of
/// those is theirs to take back with `x`. Groups appear in the order the mounts
/// themselves do, so this only ever inserts headings — the sequence Driva was
/// handed is still readable down the column.
fn grouped_mount_lines(mounts: &[AttributedMount]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current: Option<MountOrigin> = None;
    for attributed in mounts {
        if current != Some(attributed.origin) {
            current = Some(attributed.origin);
            lines.push(Line::from(Span::styled(
                format!("  {}", attributed.origin.label()),
                Style::default().fg(palette::ADDITIONAL_INFO),
            )));
        }
        lines.push(mount_line(&attributed.mount));
    }
    lines
}

/// How the effective network policy reads, together with where it comes from
/// when that is not the operator's own doing.
///
/// `w` cannot force networking off — every agent profile Styra can launch
/// already permits it, and a template may too, so the resolved policy can read
/// "on" whatever the operator's input says. Showing only the resolved value made
/// `w` look like a key that did nothing: the message said "network off for the
/// next interaction" while the field kept reading `on`. Name the source instead,
/// so the key is visible and its limit is stated — and, when the answer is
/// inherited, say which layer it is inherited from.
fn network_label(app: &App, effective: bool) -> String {
    let on = if effective { "on" } else { "off" };
    if !app.can_edit_launch() {
        return on.to_owned();
    }
    let asked = app.launch.effective().grants_network();
    if effective != asked {
        return format!("{on} — from the agent profile; your setting: off (w cannot revoke it)");
    }
    match (app.launch.interaction.network, app.launch.workspace.network) {
        // Stated by this launch, against what it would otherwise inherit.
        (Some(_), Some(_)) => format!("{on} — this interaction, over the Workspace policy"),
        (Some(_), None) => on.to_owned(),
        (None, Some(_)) => format!("{on} — from the Workspace policy"),
        (None, None) => on.to_owned(),
    }
}

/// The templates the next launch would layer, in the order they apply, saying
/// which of them the Workspace contributes: those live in the other pane.
fn templates_label(app: &App) -> String {
    let effective = app.launch.effective().templates;
    if effective.is_empty() {
        return "none".to_owned();
    }
    let from_workspace = if app.launch.interaction.standalone {
        &[][..]
    } else {
        &app.launch.workspace.templates
    };
    effective
        .iter()
        .map(|name| {
            if from_workspace.contains(name) {
                format!("{name} (Workspace)")
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How one settings pane's rows are painted. The pane the keys act on is bright
/// and carries the mount cursor; the other is muted, so which layer an edit
/// would land in is never in question. A Workspace policy this interaction has
/// opted out of is struck through as well as muted: it is still worth reading
/// and still editable here, but it is not part of this launch.
#[derive(Clone, Copy)]
struct PaneStyle {
    label: Style,
    value: Style,
    marker: Style,
    cursor: bool,
}

fn pane_style(app: &App, scope: LaunchScope) -> PaneStyle {
    let focused = app.launch.scope == scope;
    let ignored = scope == LaunchScope::Workspace && app.launch.interaction.standalone;
    let mut value = if focused {
        Style::default().fg(palette::TEXT)
    } else {
        Style::default().fg(palette::MUTED_TEXT)
    };
    if ignored {
        value = Style::default()
            .fg(palette::INACTIVE)
            .add_modifier(Modifier::CROSSED_OUT);
    }
    PaneStyle {
        label: if focused {
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette::INACTIVE)
        },
        value,
        marker: Style::default().fg(palette::WARNING),
        cursor: focused,
    }
}

/// One row of a settings pane: a marker column, a fixed label column, a value.
fn setting_line(style: PaneStyle, marked: bool, label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            if marked && style.cursor {
                " • "
            } else {
                "   "
            },
            style.marker,
        ),
        Span::styled(
            format!("{label:<width$}", width = SETTING_LABEL),
            style.label,
        ),
        Span::styled(value.to_owned(), style.value),
    ])
}

/// The settings one layer holds, in the same rows for both layers.
fn pane_rows(app: &App, scope: LaunchScope) -> Vec<Line<'static>> {
    let style = pane_style(app, scope);
    let policy = app.launch.policy(scope);
    let mut rows = Vec::new();

    // Whether this interaction starts from the Workspace's policy at all is the
    // interaction's own answer, so it is a row of its pane rather than a key
    // hint — `I` was invisible as anything but a hint before.
    if scope == LaunchScope::Interaction {
        rows.push(setting_line(
            style,
            false,
            "inherits",
            if policy.standalone {
                "nothing — standalone, the Workspace policy does not apply"
            } else {
                "the Workspace policy above"
            },
        ));
    }

    rows.push(setting_line(
        style,
        false,
        "network",
        &scope_network_label(app, scope),
    ));
    rows.push(setting_line(
        style,
        false,
        "templates",
        &if policy.templates.is_empty() {
            "none".to_owned()
        } else {
            policy.templates.join(", ")
        },
    ));

    if policy.mounts.is_empty() {
        rows.push(setting_line(style, false, "mounts", "none — m adds one"));
        return rows;
    }
    let selected = app.launch.cursor(scope);
    for (index, mount) in policy.mounts.iter().enumerate() {
        rows.push(setting_line(
            style,
            index == selected,
            if index == 0 { "mounts" } else { "" },
            &mount::label(mount),
        ));
    }
    rows
}

/// What one layer says about networking, as that layer alone.
///
/// The Workspace's is a plain on/off: nothing sits under it to inherit from.
/// This interaction's has a third answer — saying nothing — and what that
/// resolves to is worth printing next to it, since it is the reason `w` can look
/// like it changed nothing.
fn scope_network_label(app: &App, scope: LaunchScope) -> String {
    match scope {
        LaunchScope::Workspace => match app.launch.workspace.network {
            Some(true) => "on".to_owned(),
            Some(false) => "off".to_owned(),
            None => "off — not stated".to_owned(),
        },
        LaunchScope::Interaction => match app.launch.interaction.network {
            Some(true) => "on".to_owned(),
            Some(false) => "off — withdrawn here".to_owned(),
            None => {
                let inherited =
                    !app.launch.interaction.standalone && app.launch.workspace.grants_network();
                format!(
                    "not stated — inherits {}",
                    if inherited { "on" } else { "off" }
                )
            }
        },
    }
}

/// One settings pane: a titled box saying which layer it is, what that layer
/// reaches, and — for the Workspace's — whether the server has it.
fn render_pane(
    frame: &mut Frame,
    app: &App,
    scope: LaunchScope,
    rows: Vec<Line<'static>>,
    area: Rect,
) {
    if area.height == 0 {
        return;
    }
    let focused = app.launch.scope == scope;
    let title = Style::default().fg(if focused {
        palette::ACCENT
    } else {
        palette::INACTIVE
    });
    let spans = vec![
        Span::styled(if focused { " ▸ " } else { "   " }, title),
        Span::styled(
            scope.title(),
            if focused {
                title.add_modifier(Modifier::BOLD)
            } else {
                title
            },
        ),
        Span::styled(
            match scope {
                LaunchScope::Workspace => " · every launch here ",
                LaunchScope::Interaction => " · over the Workspace policy ",
            },
            Style::default().fg(palette::ADDITIONAL_INFO),
        ),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            palette::ACCENT
        } else {
            palette::INACTIVE
        }))
        .title(Line::from(spans));
    frame.render_widget(Paragraph::new(rows).block(block), area);
}

/// The keys, named against the pane they would act on. Two lines: what every
/// pane answers to, then what is particular to the focused one.
fn hint_lines(app: &App) -> Vec<Line<'static>> {
    let muted = Style::default().fg(palette::ADDITIONAL_INFO);
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "  Tab {} · m mount · x remove · T templates · w network",
            app.launch.scope.other().phrase()
        ),
        muted,
    ))];
    lines.push(Line::from(Span::styled(
        match app.launch.scope {
            LaunchScope::Workspace => {
                "  changes are stored by the server and shared by every client".to_owned()
            }
            LaunchScope::Interaction => format!(
                "  I {} · U move up into it · D save as default",
                if app.launch.interaction.standalone {
                    "inherit the Workspace"
                } else {
                    "ignore the Workspace"
                }
            ),
        },
        muted,
    )));
    lines
}

/// The "add a mount" prompt, floating over the view it edits.
fn render_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let Some(text) = &app.launch.prompt else {
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
    // Which layer the mount lands in is the prompt's own business too: it is
    // opened from either pane and the path being typed says nothing about that.
    // On the bottom border rather than beside the syntax, which is already as
    // wide as the box.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" mount · source[:destination][:ro|rw] · Enter add · Esc cancel ")
        .title_bottom(Line::from(Span::styled(
            format!(" for {} ", app.launch.scope.phrase()),
            Style::default().fg(palette::ACCENT),
        )));
    frame.render_widget(Clear, prompt);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(text.clone(), Style::default().fg(palette::TEXT)),
            Span::styled("▏", Style::default().fg(palette::WARNING)),
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
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_owned(), Style::default().fg(palette::TEXT)),
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
                MountAccess::ReadWrite => ("rw", palette::WARNING),
                MountAccess::ReadOnly => ("ro", palette::MUTED_TEXT),
            };
            Line::from(vec![
                Span::styled(
                    format!("    {label} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} → {}", source.display(), destination.display()),
                    Style::default().fg(palette::TEXT),
                ),
            ])
        }
        Mount::Temporary { destination } => Line::from(vec![
            Span::styled(
                "    tmp ",
                Style::default()
                    .fg(palette::INFO)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                destination.display().to_string(),
                Style::default().fg(palette::TEXT),
            ),
        ]),
        Mount::Overlay {
            source,
            destination,
        } => Line::from(vec![
            Span::styled(
                "    ovl ",
                Style::default()
                    .fg(palette::SPECIAL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} → {}", source.display(), destination.display()),
                Style::default().fg(palette::TEXT),
            ),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;
    use super::*;
    use crate::app::View;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    #[test]
    fn details_view_shows_workspace_even_without_a_launch_policy() {
        use styra_server::DrivaOptions;
        use styra_server::{Mount, MountAccess};

        let mut app = testing::app("s1");
        app.toggle_view(View::Driva);
        let placeholder = rendered(&app);
        assert!(placeholder.contains("Workspace"));
        assert!(placeholder.contains("current interaction"));
        assert!(placeholder.contains("server has not resolved a launch policy"));

        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into(), "app-server".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: vec![AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: PathBuf::from("/home/op/project"),
                    destination: PathBuf::from("/tmp/styra/workspace"),
                    access: MountAccess::ReadWrite,
                },
            }],
        });
        let screen = tall(&app);
        assert!(screen.contains("details"));
        assert!(screen.contains("bwrap"));
        assert!(screen.contains("codex app-server"));
        assert!(screen.contains("off"));
        assert!(screen.contains("/home/op/project"));
        assert!(screen.contains("/tmp/styra/workspace"));
        assert!(screen.contains("workspace"));
        assert!(!screen.contains("planned"));
    }

    #[test]
    fn details_include_the_complete_server_workspace_snapshot() {
        let mut app = testing::pending_app();
        let workspace = styra_server::WorkspaceSummary {
            id: "w-42".into(),
            name: Some("payments".into()),
            host_path: "/work/payments".into(),
            git_repository: Some("/git/payments".into()),
            worktrees_enabled: true,
            path: "/state/workspaces/w-42".into(),
            session_count: 7,
            age: "2h ago".into(),
            created_at_ms: 100,
            last_accessed_at_ms: 200,
            launch: styra_server::LaunchPolicy {
                network: Some(true),
                templates: vec!["rust".into()],
                mounts: vec![styra_server::LaunchMount::default()],
                standalone: false,
            },
        };
        app.workspace.enter(workspace.host_path.clone());
        app.show_workspace(&workspace);

        let lines = workspace_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        for value in [
            "payments",
            "w-42",
            "/work/payments",
            "/git/payments",
            "/state/workspaces/w-42",
            "7 session(s)",
            "2h ago · 100 ms",
            "accessed 200 ms",
            "worktrees on",
            "network on",
            "1 template(s)",
            "1 mount(s)",
        ] {
            assert!(lines.contains(value), "missing {value:?} from {lines}");
        }
    }

    #[test]
    fn details_include_current_interaction_state_and_latest_message() {
        let mut app = testing::app("s-7");
        app.session_name = Some("refactor".into());
        app.workspace.id = Some("w-42".into());
        app.workspace.enter("/work/payments/api".into());
        app.outbox.replace_queued(vec![
            styra_server::QueuedMessage::new("one"),
            styra_server::QueuedMessage::new("two"),
        ]);
        app.push_event(styra_server::event::AgentEvent::AgentMessage {
            text: "Implemented the change".into(),
        });

        let lines = interaction_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        for value in [
            "refactor · s-7",
            "w-42",
            testing::PROFILE,
            "running",
            "accepting     yes",
            "/work/payments/api",
            "queued        2",
            "Implemented the change",
        ] {
            assert!(lines.contains(value), "missing {value:?} from {lines}");
        }
    }

    #[test]
    fn driva_groups_effective_mounts_by_their_origin() {
        let mounts = vec![
            AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: PathBuf::from("/host/project"),
                    destination: PathBuf::from("/workspace"),
                    access: MountAccess::ReadWrite,
                },
            },
            AttributedMount {
                origin: MountOrigin::Profile,
                mount: Mount::Bind {
                    source: PathBuf::from("/host/credentials"),
                    destination: PathBuf::from("/root/.config"),
                    access: MountAccess::ReadOnly,
                },
            },
            AttributedMount {
                origin: MountOrigin::Operator,
                mount: Mount::Bind {
                    source: PathBuf::from("/host/data"),
                    destination: PathBuf::from("/data"),
                    access: MountAccess::ReadOnly,
                },
            },
        ];
        let lines = grouped_mount_lines(&mounts)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "  workspace",
                "    rw /host/project → /workspace",
                "  agent profile",
                "    ro /host/credentials → /root/.config",
                "  your mounts",
                "    ro /host/data → /data",
            ]
        );
    }

    #[test]
    fn driva_view_marks_the_policy_a_not_yet_started_interaction_would_launch_under() {
        use styra_server::DrivaOptions;

        let selection = styra_server::agent::Selection::parse("codex").unwrap();
        let mut app = App::new(selection.clone(), "s1");
        app.toggle_view(View::Driva);
        app.launch.plan(
            selection,
            app.launch.effective(),
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into(), "app-server".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: true,
                mounts: Vec::new(),
            }),
        );
        let screen = tall(&app);
        assert!(screen.contains("planned — applied when the next interaction starts"));
        assert!(screen.contains("codex app-server"));
        assert!(screen.contains("network  on"));
    }

    fn editable_app() -> App {
        use styra_server::DrivaOptions;

        let selection = styra_server::agent::Selection::parse("codex").unwrap();
        let mut app = App::pending(selection.clone());
        app.toggle_view(View::Driva);
        app.launch.plan(
            selection,
            app.launch.effective(),
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

    /// Before launch the view has to say what can be changed, in the layer it
    /// would be changed in, with the keys for it.
    #[test]
    fn a_not_yet_started_launch_shows_both_layers_and_the_keys_for_them() {
        let mut app = editable_app();
        let screen = tall(&app);
        assert!(screen.contains("templates none"), "{screen}");
        // Both panes, named, and the one the keys are on marked.
        assert!(screen.contains("Workspace · every launch here"), "{screen}");
        assert!(
            screen.contains("this interaction · over the Workspace policy"),
            "{screen}"
        );
        assert!(screen.contains("▸ this interaction"), "{screen}");
        assert!(screen.contains("none — m adds one"), "{screen}");
        assert!(
            screen.contains("Tab the Workspace · m mount · x remove"),
            "{screen}"
        );

        crate::launch::set_templates(&mut app, vec!["rust".into(), "browser".into()]);
        app.launch.prompt = Some("/srv/data:/mnt/data:rw".into());
        crate::launch::confirm_prompt(&mut app);
        let screen = tall(&app);
        assert!(screen.contains("rust, browser"), "{screen}");
        assert!(screen.contains("/srv/data → /mnt/data (rw)"), "{screen}");
    }

    /// The two layers are separately visible and separately edited: what the
    /// Workspace grants every launch here sits in its own pane, and `Tab` is
    /// what moves the keys — and the cursor — onto it.
    #[test]
    fn each_layer_is_shown_and_edited_in_its_own_pane() {
        let mut app = editable_app();
        app.launch.set_workspace(styra_server::LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![styra_server::LaunchMount {
                source: PathBuf::from("/srv/corpus"),
                destination: Some(PathBuf::from("/mnt/corpus")),
                writable: false,
            }],
            standalone: false,
        });
        crate::launch::set_templates(&mut app, vec!["rust".into(), "browser".into()]);
        app.launch.prompt = Some("/srv/scratch:rw".into());
        crate::launch::confirm_prompt(&mut app);

        let screen = tall(&app);
        // Each layer's own rows, in its own pane.
        assert!(
            screen.contains("/srv/corpus → /mnt/corpus (ro)"),
            "{screen}"
        );
        assert!(screen.contains("/srv/scratch (rw)"), "{screen}");
        // And the layering they resolve to, with the Workspace's own named.
        assert!(screen.contains("rust (Workspace), browser"), "{screen}");
        // This interaction states nothing about the network, and the pane says
        // what that inherits rather than leaving `w` looking inert.
        assert!(screen.contains("not stated — inherits on"), "{screen}");

        // Tab moves the keys onto the Workspace's layer, and the mount cursor
        // with them: its mounts are removable there.
        crate::launch::toggle_scope(&mut app);
        let screen = tall(&app);
        assert!(screen.contains("▸ Workspace"), "{screen}");
        assert!(
            screen.contains("Tab this interaction · m mount"),
            "{screen}"
        );
        crate::launch::remove_selected_mount(&mut app);
        assert_eq!(app.launch.workspace.mounts.len(), 1);
        // This interaction's own mount is untouched by an edit to the other
        // layer.
        assert_eq!(app.launch.interaction.mounts.len(), 1);
    }

    /// A Workspace edit is only reflected after the server returns its new
    /// authoritative policy; the UI never displays an optimistic copy.
    #[test]
    fn workspace_edits_are_rendered_from_the_server_snapshot() {
        let mut app = editable_app();
        crate::launch::toggle_scope(&mut app);
        crate::launch::cycle_network(&mut app);
        assert_eq!(app.launch.workspace.network, None);

        let screen = tall(&app);
        assert!(screen.contains("off — not stated"), "{screen}");

        app.launch.sync_workspace(styra_server::LaunchPolicy {
            network: Some(true),
            ..Default::default()
        });
        let screen = tall(&app);
        assert!(screen.contains("network   on"), "{screen}");
        assert!(
            screen.contains("changes are stored by the server"),
            "{screen}"
        );
    }

    /// Standalone is this interaction's own row, and it says what it does to the
    /// other layer where that layer is shown.
    #[test]
    fn standalone_is_a_row_of_this_interactions_pane_and_strikes_the_other_out() {
        let mut app = editable_app();
        app.launch.set_workspace(styra_server::LaunchPolicy {
            templates: vec!["rust".into()],
            ..styra_server::LaunchPolicy::default()
        });
        let screen = tall(&app);
        assert!(screen.contains("the Workspace policy above"), "{screen}");
        assert!(screen.contains("I ignore the Workspace"), "{screen}");

        crate::launch::toggle_standalone(&mut app);
        let screen = tall(&app);
        assert!(
            screen.contains("nothing — standalone, the Workspace policy does not apply"),
            "{screen}"
        );
        assert!(screen.contains("I inherit the Workspace"), "{screen}");
    }

    /// A network answer the operator did not give themselves has to name the
    /// layer it came from, or `w` reads as a key with no effect.
    #[test]
    fn network_names_the_workspace_policy_when_that_is_where_the_answer_comes_from() {
        use styra_server::DrivaOptions;

        let mut app = editable_app();
        app.launch.set_workspace(styra_server::LaunchPolicy {
            network: Some(true),
            ..styra_server::LaunchPolicy::default()
        });
        app.launch.plan(
            app.selection.clone(),
            app.launch.effective(),
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: true,
                mounts: Vec::new(),
            }),
        );
        let screen = tall(&app);
        assert!(screen.contains("from the Workspace policy"), "{screen}");

        // Withdrawn by this launch: now it is the operator's own answer, over
        // the Workspace's.
        crate::launch::cycle_network(&mut app);
        app.launch.plan(
            app.selection.clone(),
            app.launch.effective(),
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: false,
                mounts: Vec::new(),
            }),
        );
        let screen = tall(&app);
        assert!(
            screen.contains("off — this interaction, over the Workspace policy"),
            "{screen}"
        );
    }

    /// Every agent profile already permits networking, so the resolved policy
    /// reads `on` whatever the operator's input is. The view has to say where
    /// that "on" comes from, otherwise `w` looks like a key that does nothing.
    #[test]
    fn network_names_the_profile_when_it_permits_what_the_operator_did_not() {
        use styra_server::DrivaOptions;

        let mut app = editable_app();
        // The server's answer for a profile that permits networking on its own.
        app.launch.plan(
            app.selection.clone(),
            app.launch.effective(),
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: true,
                mounts: Vec::new(),
            }),
        );
        let screen = tall(&app);
        assert!(screen.contains("from the agent profile"), "{screen}");
        assert!(screen.contains("your setting: off"), "{screen}");

        // Once the operator asks for it too, the field is just the policy.
        crate::launch::cycle_network(&mut app);
        let screen = tall(&app);
        assert!(!screen.contains("from the agent profile"), "{screen}");
        assert!(screen.contains("network  on"), "{screen}");
    }

    /// A live session's policy is a record, so neither settings pane nor any of
    /// their keys are drawn over it.
    #[test]
    fn a_live_launch_policy_offers_no_editing() {
        use styra_server::DrivaOptions;

        let mut app = testing::app("s1");
        app.toggle_view(View::Driva);
        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: Vec::new(),
        });
        let screen = tall(&app);
        assert!(!screen.contains("every launch here"), "{screen}");
        assert!(!screen.contains("m mount"), "{screen}");
    }

    /// Once stopped, there is no live sandbox left to contradict, so editing
    /// reopens exactly as if the interaction had never started.
    #[test]
    fn a_stopped_interactions_launch_policy_can_be_edited_again() {
        use crate::activity::Status;
        use styra_server::DrivaOptions;

        let mut app = testing::app("s1");
        app.toggle_view(View::Driva);
        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: Vec::new(),
        });
        app.activity.status = Status::Stopped;
        assert!(app.can_edit_launch());
        let screen = tall(&app);
        assert!(screen.contains("m mount"), "{screen}");
    }

    #[test]
    fn the_mount_prompt_floats_over_the_policy_it_edits() {
        let mut app = editable_app();
        crate::launch::open_prompt(&mut app);
        app.launch.prompt = Some("/srv/data".into());
        let screen = tall(&app);
        assert!(screen.contains("source[:destination][:ro|rw]"), "{screen}");
        assert!(screen.contains("/srv/data"), "{screen}");
        // Which layer it will land in is part of the prompt, since it is opened
        // from either pane.
        assert!(screen.contains("for this interaction"), "{screen}");
    }
}
