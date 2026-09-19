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

use crate::chrome::{panel_block, PanelChrome};
use crate::palette;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;
use styra_protocol::{
    AttributedMount, DrivaOptions, FloorKind, LaunchMount, LaunchPolicy, Mount, MountAccess,
    MountOrigin, VariableOrigin, WritableMountMode,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchScope {
    Workspace,
    #[default]
    Interaction,
}

impl LaunchScope {
    fn title(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Interaction => "this interaction",
        }
    }
    fn phrase(self) -> &'static str {
        match self {
            Self::Workspace => "the Workspace",
            Self::Interaction => "this interaction",
        }
    }
    fn other(self) -> Self {
        match self {
            Self::Workspace => Self::Interaction,
            Self::Interaction => Self::Workspace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrivaStatus {
    Pending,
    Running,
    Idle,
    Background,
    Stopped,
    Ended,
}

impl DrivaStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Pending => "not started",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Background => "idle · background work running",
            Self::Stopped => "stopped",
            Self::Ended => "ended",
        }
    }
}

pub struct DrivaWorkspace {
    pub id: Option<String>,
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub git_repository: Option<PathBuf>,
    pub host_path: Option<PathBuf>,
    pub server_path: Option<PathBuf>,
    pub session_count: Option<usize>,
    pub age: Option<String>,
    pub created_at_ms: Option<u64>,
    pub last_accessed_at_ms: Option<u64>,
    pub root: Option<PathBuf>,
    pub working_directory: Option<PathBuf>,
}

impl DrivaWorkspace {
    fn root(&self) -> Option<&std::path::Path> {
        self.root.as_deref()
    }
    fn working_directory_or_current(&self) -> Option<PathBuf> {
        self.working_directory.clone()
    }
}

pub struct DrivaActivity {
    pub status: DrivaStatus,
}

pub struct DrivaLaunch<'a> {
    pub workspace: &'a LaunchPolicy,
    pub interaction: &'a LaunchPolicy,
    pub scope: LaunchScope,
    pub workspace_cursor: usize,
    pub interaction_cursor: usize,
    pub prompt: Option<&'a str>,
    pub driva: Option<&'a DrivaOptions>,
    pub planned: bool,
    pub requested_scroll: u16,
}

impl DrivaLaunch<'_> {
    fn policy(&self, scope: LaunchScope) -> &LaunchPolicy {
        match scope {
            LaunchScope::Workspace => self.workspace,
            LaunchScope::Interaction => self.interaction,
        }
    }
    fn effective(&self) -> LaunchPolicy {
        LaunchPolicy::merge(self.workspace, self.interaction)
    }
    fn cursor(&self, scope: LaunchScope) -> usize {
        match scope {
            LaunchScope::Workspace => self.workspace_cursor,
            LaunchScope::Interaction => self.interaction_cursor,
        }
    }
}

pub struct DrivaView<'a> {
    pub chrome: PanelChrome,
    pub editable: bool,
    pub launch: DrivaLaunch<'a>,
    pub workspace: DrivaWorkspace,
    pub activity: DrivaActivity,
    pub selection_name: String,
    pub session_id: &'a str,
    pub session_name: Option<&'a str>,
    pub queued_count: usize,
    pub last_message: Option<String>,
    pub workspace_launch_pending: usize,
    pub git_repository_prompt: Option<&'a str>,
}

impl DrivaView<'_> {
    fn can_edit_launch(&self) -> bool {
        self.editable
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrivaFeedback {
    pub scroll_limit: u16,
    pub effective_scroll: u16,
}

/// Rows the effective-policy summary keeps for itself before either settings
/// pane is given any height. Enough for the banner and the fields that say what
/// is about to run; the mounts below them are what a short terminal loses.
const SUMMARY_MIN_HEIGHT: u16 = 5;

/// Width of the label column inside a settings pane. Wide enough for
/// `templates`, and identical in both panes so the two read as one form.
const SETTING_LABEL: usize = 10;

pub fn render(frame: &mut Frame, app: &DrivaView, area: Rect) -> DrivaFeedback {
    let block = panel_block(&app.chrome);
    let options = app.launch.driva;

    // A live interaction's policy is a record: there is nothing to choose, so
    // the panes and their keys are not drawn over it at all.
    if !app.can_edit_launch() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let feedback = render_summary(frame, app, options, inner);
        render_prompt(frame, app, area);
        render_git_repository_prompt(frame, app, area);
        return feedback;
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

    let feedback = render_summary(frame, app, options, rest);
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
    render_git_repository_prompt(frame, app, area);
    feedback
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
fn render_summary(
    frame: &mut Frame,
    app: &DrivaView,
    options: Option<&DrivaOptions>,
    area: Rect,
) -> DrivaFeedback {
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
    render_sandbox(frame, app, options, sandbox_area)
}

/// The sandbox account, in whichever of three shapes fits what there is room
/// for.
///
/// Everything here is worth reading and none of it can be edited, so the
/// question is only how much of it is on screen at once. It is laid out in one
/// column when that fits; the private root flows into a second column when
/// that is what makes it fit; and when neither does, the whole account becomes
/// one scrolling column, because an account of what an agent can reach that
/// quietly stops halfway is worse than no account at all.
fn render_sandbox(
    frame: &mut Frame,
    app: &DrivaView,
    options: Option<&DrivaOptions>,
    area: Rect,
) -> DrivaFeedback {
    if area.height == 0 {
        return DrivaFeedback::default();
    }
    let width = area.width.max(1);
    let lines = sandbox_lines(app, options);
    let height = |lines: &Vec<Line<'static>>| {
        Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(width)
    };
    let total = height(&lines);
    if total <= usize::from(area.height) {
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return DrivaFeedback::default();
    }

    // Two columns for the private root, which is the longest part and the one
    // part that is a list of independent groups.
    let private_root = options.filter(|options| !options.base.is_empty());
    if let Some(root) = private_root.filter(|_| area.width >= 72) {
        let prefix = sandbox_prefix_lines(app, options);
        let prefix_height = height(&prefix);
        let columns = private_root_column_height(root, width / 2);
        if prefix_height + columns <= usize::from(area.height) {
            let prefix_height = prefix_height as u16;
            frame.render_widget(
                Paragraph::new(prefix).wrap(Wrap { trim: false }),
                Rect {
                    height: prefix_height,
                    ..area
                },
            );
            render_private_root(
                frame,
                root,
                Rect {
                    y: area.y + prefix_height,
                    height: area.height - prefix_height,
                    ..area
                },
            );
            return DrivaFeedback::default();
        }
    }

    // One scrolling column. What is off screen is said in the section's own
    // heading rather than on a line of its own: a view that is cut off without
    // admitting it reads as the whole policy, and spending a row on saying so
    // would cut it off one line sooner.
    let mut lines = lines;
    let limit = total.saturating_sub(usize::from(area.height));
    let limit = limit.min(usize::from(u16::MAX)) as u16;
    let offset = app.launch.requested_scroll.min(limit);
    if let Some(heading) = lines.iter_mut().find(|line| line.to_string() == "sandbox") {
        heading.push_span(Span::styled(
            format!(
                "  ▾ {} more line(s) · PgDn/PgUp",
                usize::from(limit.saturating_sub(offset))
            ),
            Style::default().fg(palette::ADDITIONAL_INFO),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((offset, 0)),
        area,
    );
    DrivaFeedback {
        scroll_limit: limit,
        effective_scroll: offset,
    }
}

/// How tall the private-root listing is once its capability groups are dealt
/// into two columns of `width`, which is what [`render_private_root`] does.
fn private_root_column_height(options: &DrivaOptions, width: u16) -> usize {
    let width = width.max(1);
    let groups = private_root_groups(options);
    let heights: Vec<usize> = groups
        .iter()
        .map(|group| Paragraph::new(group.clone()).line_count(width))
        .collect();
    let total: usize = heights.iter().sum();
    let target = total.div_ceil(2);
    let mut first = 0;
    for height in &heights {
        if first > 0 && first + height > target {
            break;
        }
        first += height;
    }
    // Two rows for the heading the listing keeps above its columns.
    2 + first.max(total - first)
}

fn sandbox_lines(app: &DrivaView, options: Option<&DrivaOptions>) -> Vec<Line<'static>> {
    let mut lines = sandbox_prefix_lines(app, options);
    if let Some(options) = options {
        lines.extend(private_root_lines(options));
    }
    lines
}

/// What the sandbox holds that no mount accounts for, and that no layer of the
/// policy can take away.
///
/// The mount list answers "what of the host can this agent reach"; it does not
/// answer "what can this agent write", because the backend lays down a
/// filesystem of its own underneath every mount — a root, a `/tmp`, the
/// working directory — and most of it is writable. An agent whose `HOME` is a
/// directory under `/tmp` therefore has a complete, writable home that appears
/// in no mount row anywhere, which is exactly the kind of grant this view
/// exists to make impossible to miss.
fn floor_lines(options: &DrivaOptions) -> Vec<Line<'static>> {
    if options.floor.is_empty() {
        return Vec::new();
    }
    // Only the innermost entry the home sits under is the one that holds it:
    // `/` is an ancestor of everything, and saying so of `/` would be noise.
    let home = home_directory(options);
    let home_entry = home.as_deref().and_then(|home| {
        options
            .floor
            .iter()
            .enumerate()
            .filter(|(_, entry)| home.starts_with(&entry.path))
            .max_by_key(|(_, entry)| entry.path.components().count())
            .map(|(index, _)| index)
    });
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "sandbox floor — the backend's own, under every mount",
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    lines.extend(options.floor.iter().enumerate().map(|(index, entry)| {
        let (label, colour) = match entry.kind {
            FloorKind::Tmpfs => ("tmp ", palette::INFO),
            FloorKind::Directory => ("dir ", palette::INFO),
            FloorKind::RootFs => ("ro  ", palette::MUTED_TEXT),
            FloorKind::Proc | FloorKind::Devices => ("sys ", palette::MUTED_TEXT),
        };
        let mut detail = match &entry.source {
            Some(source) => format!("{} — {}", source.display(), entry.kind.description()),
            None => entry.kind.description().to_owned(),
        };
        // The one part of the floor an operator is most likely to have
        // assumed is a mount: the agent's home, when the profile pins `HOME`
        // to a path that lands here.
        if home_entry == Some(index) {
            detail.push_str(match home.as_deref() {
                Some(home) if home == entry.path => " · is the agent's HOME",
                _ => " · holds the agent's HOME",
            });
        }
        Line::from(vec![
            Span::styled(
                format!("    {label}"),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<20} {detail}", entry.path.display().to_string()),
                Style::default().fg(palette::TEXT),
            ),
        ])
    }));
    lines
}

/// Where the agent's home lands inside the sandbox, as its own environment
/// states it. Read back from the captured policy rather than assumed, because
/// a profile is free to pin it anywhere.
fn home_directory(options: &DrivaOptions) -> Option<PathBuf> {
    options
        .environment
        .iter()
        .find(|variable| variable.name == "HOME")
        .map(|variable| PathBuf::from(&variable.value))
}

/// Every environment variable the agent will run with, under the layer that
/// set it.
///
/// A sandbox's environment is cleared before anything is set in it, so this is
/// the whole of what the agent sees and not a difference against the operator's
/// own shell. It belongs next to the mounts for the same reason the mounts are
/// grouped: a value crosses into the sandbox as surely as a path does, and
/// "why is that set" has the same three or four answers.
fn environment_lines(options: &DrivaOptions) -> Vec<Line<'static>> {
    if options.environment.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "environment — all of it; the sandbox starts with none",
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    // A fixed order rather than the order Driva sets them in: the request's
    // own variables are one alphabetical run of mixed layers, and grouping
    // them by that run would repeat every heading.
    for origin in [
        VariableOrigin::Profile,
        VariableOrigin::Template,
        VariableOrigin::Broker,
        VariableOrigin::Base,
        VariableOrigin::Sandbox,
    ] {
        let mut variables = options
            .environment
            .iter()
            .filter(|variable| variable.origin == origin)
            .peekable();
        if variables.peek().is_none() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!("  {}", origin.label()),
            Style::default().fg(palette::ADDITIONAL_INFO),
        )));
        lines.extend(variables.map(|variable| {
            Line::from(vec![
                Span::styled(
                    format!("    {} ", variable.name),
                    Style::default().fg(palette::MUTED_TEXT),
                ),
                Span::styled(variable.value.clone(), Style::default().fg(palette::TEXT)),
            ])
        }));
    }
    lines
}

/// The sandbox facts that precede the private-root capability listing.
fn sandbox_prefix_lines(app: &DrivaView, options: Option<&DrivaOptions>) -> Vec<Line<'static>> {
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
    // The rest of what the request states, in one line because none of it is
    // editable and none of it is as pressing as the grants above — but each
    // part is something an operator would otherwise have to know Driva's
    // defaults to answer.
    lines.push(Line::from(""));
    lines.push(driva_field_line(
        "run as",
        &[
            if options.interactive {
                "a terminal of its own"
            } else {
                "no terminal — the protocol runs over pipes"
            },
            if options.new_session {
                "its own session, so it cannot type into yours"
            } else {
                "this terminal's session"
            },
            match options.writable_mounts {
                WritableMountMode::Direct => "writable mounts write through to the host",
                WritableMountMode::Overlay => "writes go to an overlay and are discarded",
            },
        ]
        .join(" · "),
    ));
    lines.extend(floor_lines(options));
    lines.extend(environment_lines(options));
    lines
}

/// Paint the private-root listing.  Its heading remains full width, while an
/// editable view may put capability groups in two columns if one column would
/// run below the settings panes.
fn render_private_root(frame: &mut Frame, options: &DrivaOptions, area: Rect) {
    if area.height == 0 {
        return;
    }
    let lines = private_root_lines(options);
    let heading = Paragraph::new(lines[..2].to_vec()).wrap(Wrap { trim: false });
    let heading_height = heading
        .line_count(area.width.max(1))
        .min(usize::from(area.height)) as u16;
    frame.render_widget(
        heading,
        Rect {
            height: heading_height,
            ..area
        },
    );
    let content_area = Rect {
        y: area.y + heading_height,
        height: area.height.saturating_sub(heading_height),
        ..area
    };
    if content_area.height == 0 {
        return;
    }

    let groups = private_root_groups(options);
    let one_column = Paragraph::new(groups.concat()).wrap(Wrap { trim: false });
    if one_column.line_count(content_area.width.max(1)) <= usize::from(content_area.height) {
        frame.render_widget(one_column, content_area);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(content_area);
    let left_width = columns[0].width.max(1);
    let total_height: usize = groups
        .iter()
        .map(|group| Paragraph::new(group.clone()).line_count(left_width))
        .sum();
    let target = total_height.div_ceil(2);
    let mut split = 0;
    let mut used = 0;
    for group in &groups {
        let height = Paragraph::new(group.clone()).line_count(left_width);
        if split > 0 && used + height > target {
            break;
        }
        used += height;
        split += 1;
    }
    frame.render_widget(
        Paragraph::new(groups[..split].concat()).wrap(Wrap { trim: false }),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(groups[split..].concat()).wrap(Wrap { trim: false }),
        columns[1],
    );
}

/// What the sandbox holds before any mount: the base its private root is
/// built from, under the capability that asked for each part.
///
/// The mounts above are the whole of what this launch *asked* for, but not the
/// whole of what the agent can reach — a sandbox still needs the host's `sh`,
/// its libraries, and its certificates. Naming them keeps the answer to "what
/// can this agent touch" complete, and makes the absence of the operator's
/// home from that list something an operator can see rather than assume.
///
/// Grouping by capability answers the next question with it: a path here is
/// not an arbitrary grant but the local meaning of something the sandbox
/// needs, and a host that keeps that thing elsewhere says so in one place.
fn private_root_lines(options: &DrivaOptions) -> Vec<Line<'static>> {
    if options.base.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "private root — read-only, no host home or data paths",
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    lines.extend(private_root_groups(options).into_iter().flatten());
    lines
}

/// A capability is indivisible when the private root flows into a second
/// column: its name, paths, and forwarded environment remain together.
fn private_root_groups(options: &DrivaOptions) -> Vec<Vec<Line<'static>>> {
    options
        .base
        .iter()
        .map(|capability| {
            let mut group = vec![Line::from(Span::styled(
                format!("  {} — {}", capability.name, capability.description),
                Style::default().fg(palette::ADDITIONAL_INFO),
            ))];
            for entry in &capability.entries {
                group.push(Line::from(vec![
                    Span::styled(
                        "    ro  ",
                        Style::default()
                            .fg(palette::MUTED_TEXT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        match &entry.source {
                            Some(source) => {
                                format!("{} → {}", entry.path.display(), source.display())
                            }
                            None => entry.path.display().to_string(),
                        },
                        Style::default().fg(palette::TEXT),
                    ),
                ]));
            }
            if !capability.environment.is_empty() {
                group.push(Line::from(vec![
                    Span::styled(
                        "    env ",
                        Style::default()
                            .fg(palette::MUTED_TEXT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        capability.environment.join(" "),
                        Style::default().fg(palette::TEXT),
                    ),
                ]));
            }
            group
        })
        .collect()
}

/// The complete durable Workspace snapshot retained by the client. These are
/// all fields the server's `WorkspaceSummary` reports, rather than only the
/// name and first-prompt worktree choice that happen to be used elsewhere in
/// the UI.
fn workspace_lines(app: &DrivaView) -> Vec<Line<'static>> {
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
            "network {} · {} template(s) · {} mount(s)",
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
fn interaction_lines(app: &DrivaView) -> Vec<Line<'static>> {
    let mut lines = vec![section_line("current interaction")];
    if app.session_id.is_empty() {
        lines.push(detail_field_line("state", "none — not started"));
        lines.push(detail_field_line("profile", &app.selection_name));
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
        detail_field_line("profile", &app.selection_name),
        detail_field_line("status", &app.activity.status.label()),
        detail_field_line(
            "accepting",
            if matches!(
                app.activity.status,
                DrivaStatus::Running | DrivaStatus::Idle | DrivaStatus::Background
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
        detail_field_line("queued", &app.queued_count.to_string()),
    ]);
    if let Some(message) = &app.last_message {
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
fn network_label(app: &DrivaView, effective: bool) -> String {
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
fn templates_label(app: &DrivaView) -> String {
    let effective = app.launch.effective().templates;
    if effective.is_empty() {
        return "none".to_owned();
    }
    let from_workspace = if app.launch.interaction.ignore_workspace {
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

fn pane_style(app: &DrivaView, scope: LaunchScope) -> PaneStyle {
    let focused = app.launch.scope == scope;
    let ignored = scope == LaunchScope::Workspace && app.launch.interaction.ignore_workspace;
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
fn pane_rows(app: &DrivaView, scope: LaunchScope) -> Vec<Line<'static>> {
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
            if policy.ignore_workspace {
                "nothing — the Workspace policy is ignored"
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
        "workspace",
        &scope_workspace_label(app, scope),
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
            &launch_mount_label(mount),
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
fn scope_network_label(app: &DrivaView, scope: LaunchScope) -> String {
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
                let inherited = !app.launch.interaction.ignore_workspace
                    && app.launch.workspace.grants_network();
                format!(
                    "not stated — inherits {}",
                    if inherited { "on" } else { "off" }
                )
            }
        },
    }
}

/// What one layer says about the workspace mount — the directory the agent is
/// launched in, which `m` and `x` cannot reach.
///
/// The Workspace's is a plain read-write/read-only, with the default named as
/// such: nothing sits under it to inherit from, and an unstated answer there
/// still means writable. This interaction's has the third answer, saying
/// nothing, and what that resolves to is printed next to it for the same reason
/// the network row does it.
fn scope_workspace_label(app: &DrivaView, scope: LaunchScope) -> String {
    match scope {
        LaunchScope::Workspace => match app.launch.workspace.writable_workspace {
            Some(true) => "read-write".to_owned(),
            Some(false) => "read-only".to_owned(),
            None => "read-write — not stated".to_owned(),
        },
        LaunchScope::Interaction => match app.launch.interaction.writable_workspace {
            Some(true) => "read-write".to_owned(),
            Some(false) => "read-only — withdrawn here".to_owned(),
            None => {
                let inherited = app.launch.interaction.ignore_workspace
                    || app.launch.workspace.grants_writable_workspace();
                format!(
                    "not stated — inherits {}",
                    if inherited { "read-write" } else { "read-only" }
                )
            }
        },
    }
}

/// One settings pane: a titled box saying which layer it is, what that layer
/// reaches, and — for the Workspace's — whether the server has it.
fn render_pane(
    frame: &mut Frame,
    app: &DrivaView,
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
fn hint_lines(app: &DrivaView) -> Vec<Line<'static>> {
    let muted = Style::default().fg(palette::ADDITIONAL_INFO);
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "  Tab {} · m mount · x remove · T templates · w network · R workspace ro/rw",
            app.launch.scope.other().phrase()
        ),
        muted,
    ))];
    lines.push(Line::from(Span::styled(
        if app.workspace_launch_pending > 0 {
            "  saving Workspace launch policy…".to_owned()
        } else {
            match app.launch.scope {
                LaunchScope::Workspace => {
                    "  changes are stored by the server and shared by every client".to_owned()
                }
                LaunchScope::Interaction => format!(
                    "  I {} · U move up into it · D save as default",
                    if app.launch.interaction.ignore_workspace {
                        "inherit the Workspace"
                    } else {
                        "ignore the Workspace"
                    }
                ),
            }
        },
        muted,
    )));
    lines
}

/// The "add a mount" prompt, floating over the view it edits.
fn render_prompt(frame: &mut Frame, app: &DrivaView, area: Rect) {
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
            Span::styled((*text).to_owned(), Style::default().fg(palette::TEXT)),
            Span::styled("▏", Style::default().fg(palette::WARNING)),
        ]))
        .block(block),
        prompt,
    );
}

/// The durable Workspace Git checkout, which is intentionally not an extra
/// mount and therefore has its own prompt.
fn render_git_repository_prompt(frame: &mut Frame, app: &DrivaView, area: Rect) {
    let Some(text) = &app.git_repository_prompt else {
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
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" Git checkout · path in repository · Enter save · empty clears · Esc cancel ");
    frame.render_widget(Clear, prompt);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled((*text).to_owned(), Style::default().fg(palette::TEXT)),
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

fn launch_mount_label(mount: &LaunchMount) -> String {
    let access = if mount.writable { "rw" } else { "ro" };
    match &mount.destination {
        Some(destination) => format!(
            "{} → {} ({access})",
            mount.source.display(),
            destination.display()
        ),
        None => format!("{} ({access})", mount.source.display()),
    }
}
