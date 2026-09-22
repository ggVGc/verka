//! Filtered plain-text transcript presentation.

use crate::chrome::{panel_block, uncommitted_title, PanelChrome};
use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub struct TranscriptView<'a> {
    pub chrome: PanelChrome,
    /// Owned by the adapter because this is the same derived text used for
    /// clipboard and export behavior; the renderer only borrows it.
    pub text: &'a str,
    pub has_entries: bool,
    pub conversation_only: bool,
    /// See [`crate::chrome::uncommitted_title`].
    pub uncommitted_changes: bool,
    pub requested_scroll: u16,
}

pub fn render(frame: &mut Frame, view: &TranscriptView<'_>, area: Rect) -> u16 {
    let mut block = panel_block(&view.chrome);
    if view.conversation_only {
        block = block.title_bottom(Line::from(Span::styled(
            " conversation only ",
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if view.uncommitted_changes {
        block = uncommitted_title(block);
    }
    if !view.has_entries {
        frame.render_widget(Paragraph::new("  nothing to render yet").block(block), area);
        return 0;
    }
    let lines = view
        .text
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line.to_owned(),
                Style::default().fg(palette::TEXT),
            ))
        })
        .collect::<Vec<_>>();
    let viewport = area.height.saturating_sub(2) as usize;
    let limit = lines.len().saturating_sub(viewport) as u16;
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((view.requested_scroll.min(limit), 0)),
        area,
    );
    limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::StatusTone;
    use ratatui::{backend::TestBackend, Terminal};

    fn chrome() -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: None,
            agent: "codex".into(),
            model: "gpt".into(),
            model_reported: true,
            effort: None,
            effort_reported: false,
            status: "idle".into(),
            status_tone: StatusTone::Idle,
            elapsed: None,
            suffix: Some("transcript".into()),
            session: None,
        }
    }
    fn screen(
        text: &str,
        has_entries: bool,
        conversation_only: bool,
        uncommitted_changes: bool,
        requested_scroll: u16,
    ) -> (String, u16) {
        let mut terminal = Terminal::new(TestBackend::new(50, 8)).unwrap();
        let mut limit = 0;
        terminal
            .draw(|frame| {
                limit = render(
                    frame,
                    &TranscriptView {
                        chrome: chrome(),
                        text,
                        has_entries,
                        conversation_only,
                        uncommitted_changes,
                        requested_scroll,
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .chunks(50)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        (output, limit)
    }
    #[test]
    fn content_and_filter_marker_are_rendered() {
        let (output, _) = screen("user: hello\nagent: hi", true, true, false, 0);
        assert!(output.contains("conversation only"));
        assert!(output.contains("user: hello"));
    }
    #[test]
    fn uncommitted_work_is_marked_beside_the_filter() {
        let (output, _) = screen("user: hello", true, true, true, 0);
        let bottom = output.lines().last().unwrap().to_owned();
        assert!(
            bottom.find("conversation only") < bottom.find("uncommitted changes"),
            "{bottom}"
        );
        let (clean, _) = screen("user: hello", true, true, false, 0);
        assert!(!clean.contains("uncommitted"));
    }
    #[test]
    fn scroll_is_measured_and_clamped() {
        let text = (0..10)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (output, limit) = screen(&text, true, false, false, u16::MAX);
        assert_eq!(limit, 4);
        assert!(output.contains("line 9"));
    }
    #[test]
    fn empty_state_has_no_scroll() {
        let (output, limit) = screen("", false, false, false, 20);
        assert_eq!(limit, 0);
        assert!(output.contains("nothing to render yet"));
    }
}
