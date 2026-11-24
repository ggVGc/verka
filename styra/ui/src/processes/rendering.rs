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
mod tests;
