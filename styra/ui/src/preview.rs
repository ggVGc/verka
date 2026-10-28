//! Side-panel and full-screen previews for one timeline entry.

use std::borrow::Cow;

use crate::code::code_block_lines;
use crate::diff::diff_block_lines;
use crate::event_list::{
    summary_line, suspicious_shell_success, wrap_rendered, EventEntry, SummaryContext,
};
use crate::footer::message_text_color;
use crate::markdown::{markdown_block_render, EntryIndex, LinkDisplay};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use styra_protocol::event::{AgentEvent, DetailBlock, Protocol};

const DETAIL_INDENT: &str = "    ";

/// Stands for the Workspace root at the start of a changed file's path.
pub const WORKSPACE_SHORTHAND: &str = "ws:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
    Selection,
    Command,
}

pub struct PreviewView<'a> {
    pub entry: Option<EventEntry<'a>>,
    /// The entry itself, when it is a file change: drawn as one, from a diff
    /// the host may have placed in its file (see [`ChangeView::diff`]).
    pub entry_change: Option<ChangeView<'a>>,
    /// The absolute paths the Workspace is known by — inside the agent's
    /// sandbox and on this host. A changed file's path under one of them is
    /// drawn from the Workspace on, after [`WORKSPACE_SHORTHAND`].
    pub workspace_roots: Vec<String>,
    pub protocol: Protocol,
    pub target: PreviewTarget,
    pub links: LinkDisplay,
    pub link_highlight: Option<EntryIndex>,
    /// The log of the Session a `branch` marker names, shown under the marker
    /// in place of its own detail: the marker only says where the other side
    /// is, and what happened there is what the operator wants to read.
    pub branch_log: Option<BranchLog<'a>>,
    /// The local destination being inspected while Markdown-link navigation is
    /// active. This supersedes the conversation entry in the preview pane.
    pub file_target: Option<FileTarget>,
    /// The selected Styra answer, parsed under its original request contract.
    pub answer_target: Option<styra_protocol::Answer>,
    pub requested_scroll: u16,
    pub fullscreen: bool,
}

/// One file change as the preview draws it.
pub struct ChangeView<'a> {
    /// The files it touched. Empty for a diff that names its own files in its
    /// headers.
    pub paths: &'a [String],
    /// The diff to draw, or `None` when the provider reported none. It may
    /// differ from what the provider sent: a snippet that gave no position
    /// can have had one found for it, so its lines can be numbered.
    pub diff: Option<Cow<'a, str>>,
}

/// Whether the linked Session's log has been read yet. Loading is a round
/// trip to the server, so an unread log and an empty one must not read the
/// same.
#[derive(Clone, Copy, Debug)]
pub enum BranchLog<'a> {
    Loading,
    /// Its events, presented as its own provider presents them.
    Ready {
        events: &'a [AgentEvent],
        protocol: Protocol,
    },
    Failed(&'a str),
}

pub struct FileTarget {
    /// The destination exactly as the link wrote it, including any position.
    pub location: String,
    pub content: FileTargetContent,
    /// A one-based source line from the destination, if one was supplied.
    pub line: Option<u32>,
}

pub enum FileTargetContent {
    Empty,
    Ready(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewFeedback {
    pub limit: u16,
    pub effective_scroll: u16,
    /// How many lines the preview shows at once.
    pub viewport: u16,
}

pub fn render(frame: &mut Frame, view: &PreviewView<'_>, area: Rect) -> PreviewFeedback {
    if let Some(target) = &view.file_target {
        let content = match &target.content {
            FileTargetContent::Empty => crate::files::FileContent::Empty,
            FileTargetContent::Ready(content) => crate::files::FileContent::Ready(content),
            FileTargetContent::Failed(error) => crate::files::FileContent::Failed(error),
        };
        return crate::files::render_content(
            frame,
            Some(&target.location),
            content,
            target.line,
            view.requested_scroll,
            area,
        );
    }
    let (content_area, block) = if view.fullscreen {
        (area, None)
    } else {
        let (shown, other_target) = match view.target {
            PreviewTarget::Selection => ("preview", "command"),
            PreviewTarget::Command => ("command", "selection"),
        };
        let answer_title = view
            .answer_target
            .as_ref()
            .map(|answer| crate::answer::title(Some(answer)));
        let shown = if let Some(title) = answer_title.as_deref() {
            title
        } else if view.branch_log.is_some() {
            "branch log"
        } else {
            shown
        };
        let title = format!(" {shown} · C: {other_target} ");
        (
            Rect::new(
                area.x + 1,
                area.y + 1,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            ),
            Some(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::INACTIVE))
                    .title(Span::styled(title, Style::default().fg(theme::MUTED_TEXT))),
            ),
        )
    };
    let mut lines = wrap_preview_lines(
        preview_lines(view),
        usize::from(content_area.width),
        if view.answer_target.is_some() {
            0
        } else {
            summary_indent(view.entry.as_ref())
        },
        if view.entry_change.is_some() || view.answer_target.is_some() {
            0
        } else {
            DETAIL_INDENT.len()
        },
    );
    crate::diff::fill_changed_rows(&mut lines, usize::from(content_area.width));
    let limit = preview_scroll_limit(&lines, content_area.width, content_area.height);
    let effective = view.requested_scroll.min(limit);
    let mut paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((effective, 0));
    if let Some(block) = block {
        paragraph = paragraph.block(block);
    }
    frame.render_widget(paragraph, area);
    PreviewFeedback {
        limit,
        effective_scroll: effective,
        viewport: content_area.height,
    }
}

pub fn preview_lines(view: &PreviewView<'_>) -> Vec<Line<'static>> {
    if let Some(answer) = &view.answer_target {
        return crate::answer::preview_lines(answer);
    }
    let Some(entry) = &view.entry else {
        return vec![Line::from(Span::styled(
            "  no entry selected",
            Style::default().fg(theme::MUTED_TEXT),
        ))];
    };
    let mut lines = vec![summary_line(entry, SummaryContext::Preview, view.protocol)];
    if let Some(change) = &view.entry_change {
        lines.extend(change_lines(change, &view.workspace_roots));
        return lines;
    }
    if let Some(log) = view.branch_log {
        lines.push(Line::from(""));
        lines.extend(branch_log_lines(log));
        return lines;
    }
    let suspicious = suspicious_shell_success(entry.event);
    lines.extend(detail_lines(
        entry.event,
        view.protocol,
        view.links,
        suspicious,
        view.link_highlight,
        &entry.branch,
    ));
    lines
}

/// An event's text as the preview shows it in full — a message's Markdown,
/// say — for a host to show elsewhere, as the diff panel shows the message
/// that started the turn it is the diff of.
pub fn message_lines(
    event: &AgentEvent,
    protocol: Protocol,
    links: LinkDisplay,
) -> Vec<Line<'static>> {
    detail_lines(
        event,
        protocol,
        links,
        false,
        None,
        &crate::event_list::BranchLink::default(),
    )
}

/// The linked Session's conversation, one line per message, oldest first.
fn branch_log_lines(log: BranchLog<'_>) -> Vec<Line<'static>> {
    let muted = |text: String| {
        vec![Line::from(Span::styled(
            text,
            Style::default().fg(theme::MUTED_TEXT),
        ))]
    };
    match log {
        BranchLog::Loading => muted(format!("{DETAIL_INDENT}loading…")),
        BranchLog::Failed(error) => vec![Line::from(Span::styled(
            format!("{DETAIL_INDENT}could not load the branch's log: {error}"),
            Style::default().fg(theme::ERROR),
        ))],
        BranchLog::Ready { events, protocol } => {
            let lines: Vec<_> = events
                .iter()
                .filter_map(|event| crate::picker::conversation_line(event, Some(protocol)))
                .collect();
            if lines.is_empty() {
                muted(format!("{DETAIL_INDENT}no messages yet"))
            } else {
                lines
            }
        }
    }
}

/// A file change's paths and its diff, drawn by [`diff_block_lines`] so each
/// file's code is highlighted in its own language and numbered. Unchanged
/// context and file metadata are left out: the preview is for seeing what
/// changed.
fn change_lines(change: &ChangeView<'_>, workspace_roots: &[String]) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = change
        .paths
        .iter()
        .map(|path| changed_path_line(path, workspace_roots))
        .collect();
    match change.diff.as_deref().filter(|diff| !diff.is_empty()) {
        Some(diff) => {
            // A diff for several files names each in its headers; a single
            // file's may not, so the path says what it is.
            let path = match change.paths {
                [path] => Some(path.as_str()),
                _ => None,
            };
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(diff_block_lines(diff, path, true));
        }
        None => lines.push(Line::from(Span::styled(
            "no diff reported",
            Style::default().fg(theme::MUTED_TEXT),
        ))),
    }
    lines
}

/// A changed file's path, its Workspace root, if it has one, shortened to a
/// muted [`WORKSPACE_SHORTHAND`] so the part that differs between files
/// stands out.
fn changed_path_line(path: &str, workspace_roots: &[String]) -> Line<'static> {
    let style = Style::default().fg(message_text_color("files"));
    let relative = workspace_roots.iter().find_map(|root| {
        let rest = path.strip_prefix(root.trim_end_matches('/'))?;
        match rest.strip_prefix('/') {
            Some(rest) => Some(rest),
            None => rest.is_empty().then_some(rest),
        }
    });
    match relative {
        Some(relative) => Line::from(vec![
            Span::styled(WORKSPACE_SHORTHAND, Style::default().fg(theme::MUTED_TEXT)),
            Span::styled(relative.to_owned(), style),
        ]),
        None => Line::from(Span::styled(path.to_owned(), style)),
    }
}

/// One event's presented detail blocks, a blank line between each.
fn detail_lines(
    event: &AgentEvent,
    protocol: Protocol,
    links: LinkDisplay,
    suspicious: bool,
    highlight: Option<EntryIndex>,
    branch: &crate::event_list::BranchLink<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut entries_before = 0;
    let blocks = crate::event_list::without_branch_snapshot(
        event,
        crate::answer::presented_detail(event, protocol),
    );
    for (index, block) in blocks.into_iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(presented_block_lines(
            block,
            message_text_color(event.tag()),
            suspicious,
            links,
            highlight,
            &mut entries_before,
        ));
    }
    lines.extend(crate::event_list::branch_message_line(branch));
    lines
}

fn presented_block_lines(
    block: DetailBlock,
    color: Color,
    suspicious: bool,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
    entries_before: &mut EntryIndex,
) -> Vec<Line<'static>> {
    if let DetailBlock::Text(text) = &block {
        let rendered = markdown_block_render(
            text,
            Style::default().fg(color),
            DETAIL_INDENT,
            links,
            highlight.and_then(|index| index.checked_sub(*entries_before)),
        );
        *entries_before += rendered.entries;
        return rendered.lines;
    }
    let (text, language) = match block {
        DetailBlock::Text(text) => (text, None),
        DetailBlock::Code { language, text } => (text, language),
    };
    code_block_lines(&text, language.as_deref(), color, suspicious, DETAIL_INDENT)
}

fn summary_indent(entry: Option<&EventEntry<'_>>) -> usize {
    usize::from(matches!(
        entry.map(|entry| entry.event),
        Some(AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. })
    )) * 2
}

fn wrap_preview_lines(
    lines: Vec<Line<'static>>,
    width: usize,
    summary_indent: usize,
    detail_indent: usize,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .flat_map(|(index, line)| {
            wrap_rendered(
                line,
                width,
                if index == 0 {
                    summary_indent
                } else {
                    detail_indent
                },
            )
        })
        .collect()
}

pub fn preview_scroll_limit(lines: &[Line<'_>], width: u16, height: u16) -> u16 {
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .saturating_sub(usize::from(height))
        .min(usize::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<String> {
        vec!["/tmp/styra/workspace".into(), "/home/op/project/".into()]
    }

    #[test]
    fn unicode_preview_scroll_counts_each_wrapped_row_once() {
        let lines = wrap_preview_lines(
            vec![Line::from("界界界界界界界"), Line::from("e\u{301}👩‍💻界")],
            6,
            2,
            0,
        );
        assert!(lines.iter().all(|line| line.width() <= 6));
        assert_eq!(lines.len(), 4);
        assert_eq!(preview_scroll_limit(&lines, 6, 2), 2);
    }

    #[test]
    fn a_path_under_the_workspace_starts_with_a_muted_shorthand() {
        for path in [
            "/tmp/styra/workspace/src/retry.rs",
            "/home/op/project/src/retry.rs",
        ] {
            let line = changed_path_line(path, &roots());
            let texts: Vec<&str> = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(texts, [WORKSPACE_SHORTHAND, "src/retry.rs"]);
            assert_eq!(line.spans[0].style.fg, Some(theme::MUTED_TEXT));
            assert_eq!(line.spans[1].style.fg, Some(message_text_color("files")));
        }
    }

    /// A sibling that only shares the root's leading characters is not in it,
    /// and a path elsewhere is shown whole.
    #[test]
    fn a_path_outside_the_workspace_is_shown_whole() {
        for path in ["/tmp/styra/workspace2/a.rs", "/etc/hosts", "src/a.rs"] {
            let line = changed_path_line(path, &roots());
            assert_eq!(line.spans.len(), 1, "{path}");
            assert_eq!(line.spans[0].content, path.to_owned());
        }
    }
}
