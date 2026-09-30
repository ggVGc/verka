//! Side-panel and full-screen previews for one timeline entry.

use crate::code::code_block_lines;
use crate::event_list::{summary_line, suspicious_shell_success, wrap_rendered, EventEntry};
use crate::footer::message_text_color;
use crate::markdown::{markdown_block_render, EntryIndex, LinkDisplay};
use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};

const DETAIL_INDENT: &str = "    ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
    Selection,
    Command,
}

pub struct PreviewView<'a> {
    pub entry: Option<EventEntry<'a>>,
    /// The file changes made during a conversation entry's turn, shown under
    /// its summary in place of its own content. `None` for an entry that
    /// previews as itself; empty for a turn that changed no files.
    pub changes: Option<Vec<&'a AgentEvent>>,
    pub protocol: Protocol,
    pub mode: PresentationMode,
    pub target: PreviewTarget,
    pub links: LinkDisplay,
    pub link_highlight: Option<EntryIndex>,
    /// The local destination being inspected while Markdown-link navigation is
    /// active. This supersedes the conversation entry in the preview pane.
    pub file_target: Option<FileTarget>,
    pub requested_scroll: u16,
    pub fullscreen: bool,
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
}

pub fn render(frame: &mut Frame, view: &PreviewView<'_>, area: Rect) -> PreviewFeedback {
    if let Some(target) = &view.file_target {
        let content = match &target.content {
            FileTargetContent::Empty => crate::files::FileContent::Empty,
            FileTargetContent::Ready(content) => crate::files::FileContent::Ready(content),
            FileTargetContent::Failed(error) => crate::files::FileContent::Failed(error),
        };
        crate::files::render_content(frame, Some(&target.location), content, target.line, area);
        return PreviewFeedback::default();
    }
    let (content_area, block) = if view.fullscreen {
        (area, None)
    } else {
        let (shown, other_target) = match view.target {
            PreviewTarget::Selection => ("preview", "command"),
            PreviewTarget::Command => ("command", "selection"),
        };
        let shown = if view.changes.is_some() {
            "turn diff"
        } else {
            shown
        };
        let (mode, other_mode) = match view.mode {
            PresentationMode::Pretty => ("pretty", "raw"),
            PresentationMode::Raw => ("raw", "pretty"),
        };
        let title = format!(" {shown} · {mode} · v: {other_mode} · C: {other_target} ");
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
                    .border_style(Style::default().fg(palette::INACTIVE))
                    .title(Span::styled(
                        title,
                        Style::default().fg(palette::MUTED_TEXT),
                    )),
            ),
        )
    };
    let lines = wrap_preview_lines(
        preview_lines(view),
        usize::from(content_area.width),
        summary_indent(view.entry.as_ref()),
    );
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
    }
}

pub fn preview_lines(view: &PreviewView<'_>) -> Vec<Line<'static>> {
    let Some(entry) = &view.entry else {
        return vec![Line::from(Span::styled(
            "  no entry selected",
            Style::default().fg(palette::MUTED_TEXT),
        ))];
    };
    let mut lines = vec![summary_line(
        entry,
        entry.expanded,
        entry.has_detail,
        false,
        view.protocol,
    )];
    if let Some(changes) = &view.changes {
        if changes.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}no file changes during this turn"),
                Style::default().fg(palette::MUTED_TEXT),
            )));
        }
        for event in changes {
            lines.push(Line::from(""));
            lines.extend(detail_lines(event, view, false, None));
        }
        return lines;
    }
    let suspicious = view.mode == PresentationMode::Pretty && suspicious_shell_success(entry.event);
    lines.extend(detail_lines(
        entry.event,
        view,
        suspicious,
        view.link_highlight,
    ));
    lines
}

/// One event's presented detail blocks, a blank line between each.
fn detail_lines(
    event: &AgentEvent,
    view: &PreviewView<'_>,
    suspicious: bool,
    highlight: Option<EntryIndex>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut entries_before = 0;
    for (index, block) in view
        .protocol
        .presented_detail(event, view.mode)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(presented_block_lines(
            block,
            message_text_color(event.tag()),
            view.mode,
            suspicious,
            view.links,
            highlight,
            &mut entries_before,
        ));
    }
    lines
}

fn presented_block_lines(
    block: DetailBlock,
    color: Color,
    mode: PresentationMode,
    suspicious: bool,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
    entries_before: &mut EntryIndex,
) -> Vec<Line<'static>> {
    if mode == PresentationMode::Pretty {
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
                    DETAIL_INDENT.len()
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
