//! Files mentioned by the focused event (or by the session), arranged by root.

use crate::app::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use styra_server::agent::SandboxLayout;

use super::{render_list, render_placeholder, render_preview, SELECTION_BG, SELECTION_MARKER};

struct FileItem {
    reported: String,
    resolved: PathBuf,
    root: PathBuf,
    relative: PathBuf,
}

fn resolve(root: &Path, reported: &str) -> PathBuf {
    let path = Path::new(reported);
    if path.is_absolute() {
        if let Ok(relative) = path.strip_prefix(&SandboxLayout::default().workspace) {
            return root.join(relative);
        }
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn items(app: &App) -> Vec<FileItem> {
    let cwd = app
        .workspace_root
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut items: Vec<_> = app
        .file_paths()
        .into_iter()
        .map(|reported| {
            let resolved = resolve(&cwd, &reported);
            let (root, relative) = match resolved.strip_prefix(&cwd) {
                Ok(relative) => (cwd.clone(), relative.to_path_buf()),
                Err(_) => {
                    let external_root = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
                    let relative = resolved
                        .strip_prefix(&external_root)
                        .unwrap_or(&resolved)
                        .to_path_buf();
                    (external_root, relative)
                }
            };
            FileItem {
                reported,
                resolved,
                root,
                relative,
            }
        })
        .collect();
    items.sort_by(|a, b| (&a.root, &a.relative).cmp(&(&b.root, &b.relative)));
    items
}

pub(crate) fn render_files(frame: &mut Frame, app: &App, area: Rect) {
    let scope = if app.file_show_all {
        "files · all session · a: focused"
    } else {
        "files · focused entry · a: all"
    };
    let files = items(app);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(columns[0]);
    render_list(frame, app, left[0]);
    let (preview_area, content_area) = if app.show_preview {
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(columns[1]);
        (Some(right[0]), right[1])
    } else {
        (None, columns[1])
    };
    if let Some(preview_area) = preview_area {
        render_preview(frame, app, preview_area);
    }
    let tree_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" {scope} "),
            Style::default().fg(Color::Gray),
        ));
    if files.is_empty() {
        render_placeholder(
            frame,
            tree_block,
            left[1],
            "no files mentioned by this entry",
        );
        render_empty_content(frame, content_area);
    } else {
        render_tree(
            frame,
            &files,
            app.file_selected.min(files.len() - 1),
            tree_block,
            left[1],
        );
        render_content(
            frame,
            &files[app.file_selected.min(files.len() - 1)],
            content_area,
        );
    }
}

fn render_tree(
    frame: &mut Frame,
    files: &[FileItem],
    selected: usize,
    block: Block<'static>,
    area: Rect,
) {
    let mut lines = Vec::new();
    let mut selected_line = 0usize;
    let mut last_root: Option<&Path> = None;
    let mut shown_dirs = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        if last_root != Some(file.root.as_path()) {
            if last_root.is_some() {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                file.root.display().to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            last_root = Some(&file.root);
        }
        let components: Vec<_> = file.relative.components().collect();
        let mut prefix = PathBuf::new();
        for (depth, component) in components
            .iter()
            .take(components.len().saturating_sub(1))
            .enumerate()
        {
            prefix.push(component);
            let key = (file.root.clone(), prefix.clone());
            if shown_dirs.insert(key) {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{}▾ {}/",
                        "  ".repeat(depth + 1),
                        component.as_os_str().to_string_lossy()
                    ),
                    Style::default().fg(Color::Gray),
                )));
            }
        }
        let name = file
            .relative
            .file_name()
            .unwrap_or(file.relative.as_os_str())
            .to_string_lossy();
        let style = if index == selected {
            Style::default()
                .fg(Color::White)
                .bg(SELECTION_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let indent = "  ".repeat(components.len());
        let marker = if index == selected { "›" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(indent, style),
            Span::styled(
                marker,
                if index == selected {
                    Style::default().fg(SELECTION_MARKER).bg(SELECTION_BG)
                } else {
                    style
                },
            ),
            Span::styled(format!(" {name}"), style),
        ]));
        if index == selected {
            selected_line = lines.len().saturating_sub(1);
        }
    }
    let scroll = selected_line
        .saturating_sub(usize::from(area.height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

fn render_content(frame: &mut Frame, file: &FileItem, area: Rect) {
    let title = format!(" {} ", file.reported);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(title, Style::default().fg(Color::Cyan)));
    let text = match std::fs::read_to_string(&file.resolved) {
        Ok(content) if content.is_empty() => Text::from(Line::from(Span::styled(
            "(empty file)",
            Style::default().fg(Color::Gray),
        ))),
        Ok(content) => Text::from(content.replace('\t', "    ")),
        Err(error) => Text::from(Line::from(Span::styled(
            format!("could not read file: {error}"),
            Style::default().fg(Color::Red),
        ))),
    };
    frame.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_empty_content(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " file preview ",
            Style::default().fg(Color::Gray),
        ));
    render_placeholder(frame, block, area, "no file selected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_server::event::AgentEvent;

    #[test]
    fn files_view_renders_tree_and_selected_content() {
        let root = std::env::temp_dir().join(format!("styra-files-ui-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.set_workspace_root(root.clone());
        app.push_event(AgentEvent::FileChanged {
            id: "1".into(),
            paths: vec!["src/main.rs".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.view = crate::app::View::Files;
        app.show_preview = true;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("src/"));
        assert!(screen.contains("main.rs"));
        assert!(screen.contains("fn main() {}"));
        let buffer = terminal.backend().buffer();
        let find = |needle: &str, start_x: u16, end_x: u16, start_y: u16| {
            (start_y..buffer.area.height).find_map(|y| {
                let row = (start_x..end_x)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>();
                row.find(needle).map(|x| (start_x + x as u16, y))
            })
        };
        let (tree_x, tree_y) = find("› main.rs", 0, 54, 0).unwrap();
        let (content_x, content_y) = find("fn main() {}", 54, 90, 8).unwrap();
        assert!(
            tree_x < 54 && tree_y >= 8,
            "tree belongs in lower-left pane, got ({tree_x}, {tree_y})"
        );
        assert!(
            content_x >= 54 && content_y >= 8,
            "file content belongs in lower-right pane"
        );
        assert_eq!(
            buffer.cell((tree_x, tree_y)).unwrap().style().bg,
            Some(SELECTION_BG),
            "the selected file uses the shared current-line highlight"
        );
        let (_, preview_y) = find("preview · pretty", 54, 90, 0).unwrap();
        assert!(preview_y < 8, "entry preview belongs in upper-right pane");
        let _ = std::fs::remove_dir_all(root);
    }
}
