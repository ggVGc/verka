use super::*;

use crate::code::code_block_lines;
use crate::diff::diff_block_lines;
use crate::event_list::{summary_line, wrap_rendered, EventEntry, SummaryContext};
use crate::footer::message_text_color;
use crate::markdown::{markdown_block_render, LinkDisplay};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use styra_protocol::event::{AgentEvent, DetailBlock, Protocol};

const DETAIL_INDENT: &str = "    ";

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
        preview_lines(view, usize::from(content_area.width)),
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
    let effective = match latest_branch_entry_rows(view, usize::from(content_area.width)) {
        // Asked for the end of a linked Session's log, it shows as an
        // overview tile does: the latest entry, with as much before it as
        // fits, or from its start when it is taller than the pane.
        Some(latest) if view.requested_scroll == u16::MAX => {
            let total = preview_scroll_limit(&lines, content_area.width, 0);
            limit.min(total.saturating_sub(latest))
        }
        _ => view.requested_scroll.min(limit),
    };
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

/// Everything the preview shows, `width` columns wide. Only the linked
/// Session's log is laid out to it here; the rest is wrapped afterward.
pub fn preview_lines(view: &PreviewView<'_>, width: usize) -> Vec<Line<'static>> {
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
    if let Some(log) = &view.branch_log {
        lines.push(Line::from(""));
        lines.extend(branch_log_lines(log, width, view.links));
        return lines;
    }
    // The list row already shows the entry's content; what the preview adds
    // for it is still to be decided.
    lines.push(Line::from(Span::styled(
        format!("{DETAIL_INDENT}{ENTRY_PLACEHOLDER}"),
        Style::default().fg(theme::MUTED_TEXT),
    )));
    lines
}

/// What the preview shows for an entry that is neither a file change, a
/// branch marker nor an answer.
pub const ENTRY_PLACEHOLDER: &str = "nothing more to preview for this entry yet";

/// An event's text in full — a message's Markdown, say — for a host to show
/// elsewhere, as the diff panel shows the message that started the turn it is
/// the diff of.
pub fn message_lines(
    event: &AgentEvent,
    protocol: Protocol,
    links: LinkDisplay,
) -> Vec<Line<'static>> {
    let color = message_text_color(event.tag());
    let mut lines = Vec::new();
    let blocks = crate::event_list::without_branch_snapshot(
        event,
        crate::answer::presented_detail(event, protocol),
    );
    for (index, block) in blocks.into_iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(match block {
            DetailBlock::Text(text) => {
                markdown_block_render(
                    &text,
                    Style::default().fg(color),
                    DETAIL_INDENT,
                    links,
                    None,
                )
                .lines
            }
            DetailBlock::Code { language, text } => {
                code_block_lines(&text, language.as_deref(), color, false, DETAIL_INDENT)
            }
        });
    }
    lines
}

/// The linked Session's conversation, oldest first, each entry drawn as its
/// own event list draws it — already `width` columns wide.
fn branch_log_lines(log: &BranchLog<'_>, width: usize, links: LinkDisplay) -> Vec<Line<'static>> {
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
        BranchLog::Ready { entries, protocol } => {
            let lines: Vec<_> = entries
                .iter()
                .flat_map(|entry| crate::event_list::entry_rows(entry, width, *protocol, links))
                .collect();
            if lines.is_empty() {
                muted(format!("{DETAIL_INDENT}no messages yet"))
            } else {
                lines
            }
        }
    }
}

/// How many rows the last entry of a loaded linked Session's log takes,
/// when that log is what the preview shows. It ends the preview's content.
fn latest_branch_entry_rows(view: &PreviewView<'_>, width: usize) -> Option<u16> {
    if view.answer_target.is_some() || view.entry_change.is_some() {
        return None;
    }
    let Some(BranchLog::Ready { entries, protocol }) = &view.branch_log else {
        return None;
    };
    let latest = entries.last()?;
    let rows = crate::event_list::entry_rows(latest, width, *protocol, view.links).len();
    Some(rows.min(usize::from(u16::MAX)) as u16)
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
mod tests;
