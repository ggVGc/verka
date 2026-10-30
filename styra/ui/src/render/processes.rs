use super::*;
use crate::chrome::panel_block;
use crate::theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, view: &ProcessesView<'_>, area: Rect) {
    // The pane sits under the event list, whose title already names the
    // interaction, so it is framed the way the entry log in the same place is.
    let block = if view.selected.is_some() {
        let mut chrome = view.chrome.clone();
        chrome.suffix = Some("processes".into());
        panel_block(&chrome)
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::INACTIVE))
            .title(Span::styled(
                " processes · H: full screen · h: close ",
                Style::default().fg(theme::MUTED_TEXT),
            ))
    };
    let processes = match view.listing {
        ProcessListing::Running(processes) if !processes.is_empty() => processes,
        ProcessListing::Running(_) => {
            return empty(frame, block, area, "  the sandbox is starting");
        }
        ProcessListing::Loading => {
            return empty(frame, block, area, "  asking the server…");
        }
        ProcessListing::NotRunning => {
            return empty(
                frame,
                block,
                area,
                "  no sandbox is running for this interaction",
            );
        }
        ProcessListing::Failed(error) => {
            let line = Line::from(Span::styled(
                format!("  could not list processes: {error}"),
                Style::default().fg(theme::ERROR),
            ));
            frame.render_widget(Paragraph::new(line).block(block), area);
            return;
        }
    };
    let rows = tree(processes);
    let selected = view.selected.map(|selected| selected.min(rows.len() - 1));
    let keys = if selected.is_some() {
        " · j/k: select · g/G: first/last · y: copy command"
    } else {
        ""
    };
    let block = block.title_bottom(Line::from(Span::styled(
        format!(
            " {} processes · {} resident{keys} ",
            rows.len(),
            memory(processes.iter().map(|process| process.rss_kib).sum()),
        ),
        Style::default().fg(theme::MUTED_TEXT),
    )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let header = Line::from(Span::styled(
        format!(
            "  {:>6} {:>8} {} {:>4} {:>7}  COMMAND",
            "PID", "HOST", "S", "THR", "RSS"
        ),
        Style::default()
            .fg(theme::MUTED_TEXT)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(header), Rect { height: 1, ..inner });
    let list_area = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, row)| ListItem::new(process_line(row, Some(index) == selected)))
        .collect::<Vec<_>>();
    let list = List::new(items).highlight_style(Style::default().bg(theme::SELECTION_BACKGROUND));
    let mut state = ListState::default();
    state.select(selected);
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn empty(frame: &mut Frame, block: Block<'_>, area: Rect, text: &'static str) {
    let line = Line::from(Span::styled(text, Style::default().fg(theme::MUTED_TEXT)));
    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn process_line(row: &TreeRow<'_>, selected: bool) -> Line<'static> {
    let process = row.process;
    let text = if selected {
        theme::WARNING
    } else {
        theme::TEXT
    };
    let state = match process.state.as_str() {
        "R" => theme::SUCCESS,
        "Z" | "D" => theme::ERROR,
        _ => theme::MUTED_TEXT,
    };
    let branch = if row.depth == 0 {
        String::new()
    } else {
        format!("{}└ ", "  ".repeat(row.depth - 1))
    };
    Line::from(vec![
        Span::styled(
            format!("  {:>6} ", process.sandbox_pid),
            Style::default().fg(text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>8} ", process.pid),
            Style::default().fg(theme::INACTIVE),
        ),
        Span::styled(format!("{} ", process.state), Style::default().fg(state)),
        Span::styled(
            format!("{:>4} {:>7}  ", process.threads, memory(process.rss_kib)),
            Style::default().fg(theme::MUTED_TEXT),
        ),
        Span::styled(branch, Style::default().fg(theme::INACTIVE)),
        Span::styled(process.command.clone(), Style::default().fg(text)),
    ])
}

fn memory(kib: u64) -> String {
    match kib {
        0..1024 => format!("{kib}K"),
        1024..1_048_576 => format!("{:.1}M", kib as f64 / 1024.0),
        _ => format!("{:.1}G", kib as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::StatusTone;
    use ratatui::{backend::TestBackend, Terminal};

    fn process(pid: u32, sandbox_pid: u32, parent_pid: u32, command: &str) -> SandboxProcess {
        SandboxProcess {
            pid,
            sandbox_pid,
            parent_pid,
            state: "S".into(),
            command: command.into(),
            threads: 1,
            rss_kib: 2048,
        }
    }

    fn chrome() -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: None,
            worktree: None,
            source_branch: None,
            parent: None,
            agent: "claude".into(),
            model: "opus".into(),
            model_reported: true,
            effort: None,
            effort_reported: false,
            status: "running".into(),
            status_tone: StatusTone::Idle,
            elapsed: None,
            suffix: None,
            session: None,
        }
    }

    fn screen(view: ProcessesView<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
        terminal
            .draw(|frame| render(frame, &view, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_tree_puts_children_under_their_parent_in_sandbox_pid_order() {
        let processes = [
            process(104, 4, 102, "cargo test"),
            process(101, 1, 100, "bwrap"),
            process(103, 3, 101, "sleep infinity"),
            process(102, 2, 101, "claude"),
        ];
        let rows: Vec<_> = tree(&processes)
            .iter()
            .map(|row| (row.process.sandbox_pid, row.depth))
            .collect();
        assert_eq!(rows, [(1, 0), (2, 1), (4, 2), (3, 1)]);
    }

    #[test]
    fn processes_render_as_an_indented_tree_with_both_pids() {
        let processes = [
            process(101, 1, 100, "bwrap"),
            process(102, 2, 101, "claude --print"),
        ];
        let output = screen(ProcessesView {
            chrome: chrome(),
            listing: ProcessListing::Running(&processes),
            selected: Some(1),
        });
        assert!(output.contains("processes"));
        assert!(output.contains("COMMAND"));
        assert!(output.contains("2.0M"));
        let claude = output
            .lines()
            .find(|line| line.contains("claude --print"))
            .unwrap();
        assert!(claude.contains(" 2 "));
        assert!(claude.contains("102"));
        assert!(claude.contains("└ claude --print"));
        assert!(output.contains("2 processes · 4.0M resident · j/k: select"));
    }

    #[test]
    fn the_pane_below_the_event_list_selects_nothing_and_offers_full_screen() {
        let processes = [process(101, 1, 100, "bwrap")];
        let output = screen(ProcessesView {
            chrome: chrome(),
            listing: ProcessListing::Running(&processes),
            selected: None,
        });
        assert!(output.contains("H: full screen"));
        assert!(!output.contains("j/k: select"));
    }

    #[test]
    fn a_stopped_interaction_says_no_sandbox_runs() {
        let output = screen(ProcessesView {
            chrome: chrome(),
            listing: ProcessListing::NotRunning,
            selected: None,
        });
        assert!(output.contains("no sandbox is running for this interaction"));
    }
}
