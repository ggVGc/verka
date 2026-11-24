use super::*;
use crate::{
    answer, driva, event_list, files, footer, interactions, launcher, log, messages, modal_input,
    overlays, overview, preview, quota, raw, recording, transcript, PanelId, RenderFeedback,
    ScrollFeedback,
};
use crate::{markdown, theme};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::Frame;

fn render_side_preview(
    frame: &mut Frame,
    view: SidePreview<'_>,
    session_id: &str,
    area: Rect,
    feedback: &mut RenderFeedback,
) {
    let (panel, measured) = match view {
        SidePreview::Entry(view) => (
            PanelId::Preview {
                session: session_id.into(),
                target: preview_panel(view),
            },
            preview::render(frame, view, area),
        ),
        SidePreview::CheckoutDiff(view) => {
            let measured = view.render(frame, area);
            feedback.diff_file_list_offset = measured.list_offset;
            feedback.diff_layout = measured.diff_layout;
            feedback.diff_report_page = measured.diff_report_page;
            (
                PanelId::CheckoutDiff {
                    session: session_id.into(),
                },
                measured.scroll,
            )
        }
    };
    note_preview_scroll(feedback, panel, measured);
}

pub fn render(frame: &mut Frame, view: &ApplicationView<'_>) -> RenderFeedback {
    let mut feedback = RenderFeedback::default();
    let status = view
        .overlays
        .busy
        .as_ref()
        .map(overlays::StatusView::busy)
        .or_else(|| {
            view.overlays
                .rebase_status
                .map(overlays::StatusView::rebase)
        });
    if let MainView::Preview(preview) = &view.main {
        let (preview_area, status_area) = split_status(frame.area(), status.as_ref());
        let measured = preview::render(frame, preview, preview_area);
        note_preview_scroll(
            &mut feedback,
            PanelId::Preview {
                session: view.session_id.into(),
                target: preview_panel(preview),
            },
            measured,
        );
        if let (Some(status), Some(area)) = (status.as_ref(), status_area) {
            status.render(frame, area);
        }
        render_reading_overlays(frame, &view.overlays);
        return feedback;
    }
    let message_height =
        messages::height(view.notices.len()).min(frame.area().height.saturating_sub(2));
    let [main_area, messages_area, footer_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(message_height),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let (main_area, status_area) = split_status(main_area, status.as_ref());
    match &view.main {
        MainView::Events(events) => {
            render_events(frame, events, view.session_id, main_area, &mut feedback)
        }
        MainView::Raw(raw_view) => {
            let measured = raw::render(frame, raw_view, main_area);
            note_scroll(
                &mut feedback,
                PanelId::Preview {
                    session: view.session_id.into(),
                    target: match raw_view.source {
                        raw::RawSource::Received(_) => crate::PreviewPanel::Messages,
                        raw::RawSource::Styra(_) => crate::PreviewPanel::StyraWire,
                        raw::RawSource::Provider(_) => crate::PreviewPanel::ProviderRaw,
                    },
                },
                measured.preview_limit,
                measured.effective_preview_scroll,
            );
        }
        MainView::Log {
            chrome,
            entries,
            scroll_back,
        } => log::render(
            frame,
            crate::chrome::panel_block(chrome),
            entries,
            *scroll_back,
            main_area,
        ),
        MainView::Quota(quota_view) => quota::render(frame, quota_view, main_area),
        MainView::Processes(processes_view) => {
            crate::processes::render(frame, processes_view, main_area)
        }
        MainView::Transcript(transcript_view) => {
            let limit = transcript::render(frame, transcript_view, main_area);
            note_scroll(
                &mut feedback,
                PanelId::Transcript {
                    session: view.session_id.into(),
                },
                limit,
                transcript_view.requested_scroll.min(limit),
            );
        }
        MainView::Driva(driva_view) => {
            let measured = driva::render(frame, driva_view, main_area);
            note_scroll(
                &mut feedback,
                PanelId::Driva {
                    session: driva_view.session_id.to_owned(),
                },
                measured.scroll_limit,
                measured.effective_scroll,
            );
        }
        MainView::Files(files_view) => {
            render_files(frame, files_view, view.session_id, main_area, &mut feedback)
        }
        MainView::Answer(answer_view) => answer::render(frame, answer_view, main_area),
        MainView::Overview(overview_view) => {
            feedback.overview_rows = Some(overview::render(frame, overview_view, main_area).rows);
        }
        MainView::Preview(_) => unreachable!(),
    }
    if let (Some(status), Some(area)) = (status.as_ref(), status_area) {
        status.render(frame, area);
    }
    if message_height > 0 {
        messages::render(frame, view.notices, messages_area);
    }
    let interaction = match &view.main {
        MainView::Events(events) => Some(&events.list.chrome),
        MainView::Files(files) => Some(&files.list.chrome),
        _ => None,
    };
    footer::render(frame, view.footer, interaction, footer_area);
    if view.overlays.busy.is_some() {
        // The bottom status replaces the input while the call runs.
    } else if let Some(capture) = view.overlays.recording {
        recording::render(frame, capture);
    } else if let Some(input) = view.overlays.input {
        modal_input::render(frame, input);
    }
    render_reading_overlays(frame, &view.overlays);
    if let Some(insert) = &view.overlays.insert {
        overlays::render_insert(frame, Some(insert.clone()), frame.area());
    }
    if let Some(menu) = view.overlays.link_menu {
        overlays::render_link_menu(frame, menu, frame.area());
    }
    if let Some(tags) = view.overlays.tags {
        overlays::render_tags(frame, tags);
    }
    if let Some(picker) = &view.overlays.diff_branches {
        overlays::render_centered_fuzzy_list(frame, picker, frame.area());
    }
    if let Some(picker) = &view.overlays.branches {
        overlays::render_centered_fuzzy_list(frame, picker, frame.area());
    }
    if let Some(menu) = view.overlays.action_menu {
        overlays::render_action_menu(frame, menu, frame.area());
    }
    if let Some(jump) = view.overlays.workspace_jump {
        overlays::render_workspace_jump(frame, jump, frame.area());
    }
    if let Some(launcher) = view.overlays.launcher {
        launcher::render_launcher(frame, launcher, frame.area());
    }
    feedback
}

fn render_events(
    frame: &mut Frame,
    view: &EventView<'_>,
    session_id: &str,
    area: Rect,
    feedback: &mut RenderFeedback,
) {
    let mut tint_area = area;
    // The checkout diff sits below the log rather than beside it.
    let diff = view
        .preview
        .filter(|preview| matches!(preview, SidePreview::CheckoutDiff(_)));
    let side_preview = view.preview.filter(|_| diff.is_none());
    if let (Some(diff), true) = (diff, view.diff_maximized) {
        render_side_preview(frame, diff, session_id, area, feedback);
        return;
    }
    let bottom_pane = view.entry_log.is_some() || view.processes.is_some();
    // With a pane below the list, the preview goes beside both of them;
    // otherwise beside the list alone.
    let interaction_area = match side_preview.filter(|_| bottom_pane) {
        Some(preview) => beside_preview(frame, preview, session_id, area, &mut tint_area, feedback),
        None => area,
    };
    let event_area = if let Some(navigator) = view.navigator {
        let [events, navigator_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(interactions::height(interaction_area.height)),
        ])
        .areas(interaction_area);
        feedback.navigator_offset = Some(interactions::render(frame, navigator, navigator_area));
        events
    } else {
        interaction_area
    };
    let event_area = if let Some(diff) = diff {
        let [events, diff_area] =
            Layout::vertical([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(event_area);
        tint_area.height = events.bottom() - tint_area.y;
        render_side_preview(frame, diff, session_id, diff_area, feedback);
        events
    } else {
        event_area
    };
    let list_area = if bottom_pane {
        let list_share = if view.entry_log.is_some() { 60 } else { 50 };
        let [list, below] = Layout::vertical([
            Constraint::Percentage(list_share),
            Constraint::Percentage(100 - list_share),
        ])
        .areas(event_area);
        if let Some(entry_log) = view.entry_log {
            let measured = event_list::render_entry_log(frame, entry_log, below);
            note_scroll(
                feedback,
                PanelId::EntryLog,
                measured.limit,
                measured.effective_scroll,
            );
        } else if let Some(processes) = view.processes {
            crate::processes::render(frame, processes, below);
        }
        if view.navigator.is_some() {
            fade_log_border(frame, below);
        }
        list
    } else if let Some(preview) = side_preview {
        beside_preview(
            frame,
            preview,
            session_id,
            event_area,
            &mut tint_area,
            feedback,
        )
    } else {
        event_area
    };
    render_list(frame, view.list, list_area, feedback);
    if view.navigator.is_some() {
        fade_log_border(frame, list_area);
    }
    if view.link_mode {
        tint_all_but_entries(frame, tint_area);
    }
}

/// Halve `area`, draw `preview` in its right half and hand back the left for
/// the log, narrowing the link-mode wash to it.
fn beside_preview(
    frame: &mut Frame,
    preview: SidePreview<'_>,
    session_id: &str,
    area: Rect,
    tint_area: &mut Rect,
    feedback: &mut RenderFeedback,
) -> Rect {
    let [log, preview_area] = Layout::horizontal([Constraint::Percentage(50); 2]).areas(area);
    tint_area.width = log.width;
    render_side_preview(frame, preview, session_id, preview_area, feedback);
    log
}

/// Reserve the status box below the main panel, above notices and the footer.
fn split_status(area: Rect, status: Option<&overlays::StatusView<'_>>) -> (Rect, Option<Rect>) {
    let Some(status) = status else {
        return (area, None);
    };
    let height = status.height(area.width).min(area.height.saturating_sub(1));
    if height == 0 {
        return (area, None);
    }
    let main = Rect {
        height: area.height - height,
        ..area
    };
    let status = Rect {
        y: main.bottom(),
        height,
        ..area
    };
    (main, Some(status))
}

/// Gently fade the log's border while navigation holds focus.
fn fade_log_border(frame: &mut Frame, area: Rect) {
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if x > area.left() && x + 1 < area.right() && y > area.top() && y + 1 < area.bottom() {
                continue;
            }
            let cell = &mut buffer[(x, y)];
            cell.set_style(event_list::faded_style(cell.style(), 2));
        }
    }
}

/// Wash `area` down the way [`modal_input::render`] washes the screen behind
/// the message box, sparing the cells [`markdown::is_entry_style`] recognises,
/// so the links being walked stand out from the conversation around them.
fn tint_all_but_entries(frame: &mut Frame, area: Rect) {
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            if !markdown::is_entry_style(cell.style()) {
                cell.set_fg(theme::MODAL_BACKDROP);
                cell.modifier.insert(Modifier::DIM);
            }
        }
    }
}

fn render_files(
    frame: &mut Frame,
    view: &FilesView<'_>,
    session_id: &str,
    area: Rect,
    feedback: &mut RenderFeedback,
) {
    let halves = [Constraint::Percentage(50); 2];
    let [left, right] = Layout::horizontal(halves).areas(area);
    let [list_area, tree_area] = Layout::vertical(halves).areas(left);
    render_list(frame, view.list, list_area, feedback);
    let content_area = if let Some(preview_view) = view.preview {
        let [preview_area, content_area] = Layout::vertical(halves).areas(right);
        render_side_preview(frame, preview_view, session_id, preview_area, feedback);
        content_area
    } else {
        right
    };
    files::render_tree(frame, view.files, view.selected, view.scope, tree_area);
    files::render_content(
        frame,
        view.selected_name,
        view.content,
        None,
        0,
        content_area,
    );
}

fn preview_panel(view: &preview::PreviewView<'_>) -> crate::PreviewPanel {
    match view.target {
        preview::PreviewTarget::Selection => crate::PreviewPanel::Selection,
        preview::PreviewTarget::Command => crate::PreviewPanel::Command,
    }
}

/// Draw the event list and record where it was drawn from.
fn render_list(
    frame: &mut Frame,
    view: &event_list::EventListView<'_>,
    area: Rect,
    feedback: &mut RenderFeedback,
) {
    let list = event_list::render(frame, view, area);
    feedback.list_offset = Some(list.effective_offset);
    feedback.list_row_offset = Some(list.effective_row_offset);
    feedback.list_selection_row = list.selection_row;
    feedback.list_selection_overflow = list.selection_overflow;
}

fn note_scroll(feedback: &mut RenderFeedback, panel: PanelId, limit: u16, effective_offset: u16) {
    feedback.scroll.push(ScrollFeedback {
        panel,
        limit,
        effective_offset,
        viewport: 0,
    });
}

/// As [`note_scroll`], for a preview, which also reports how many lines it
/// shows at once.
fn note_preview_scroll(
    feedback: &mut RenderFeedback,
    panel: PanelId,
    measured: preview::PreviewFeedback,
) {
    feedback.scroll.push(ScrollFeedback {
        panel,
        limit: measured.limit,
        effective_offset: measured.effective_scroll,
        viewport: measured.viewport,
    });
}

fn render_reading_overlays(frame: &mut Frame, view: &ApplicationOverlays<'_>) {
    if let Some(name) = view.rename_prompt {
        let prompt = crate::text_prompt::TextPrompt::new(" rename interaction ", name)
            .bottom(" Enter save · Ctrl-G generate · Ctrl-U clear · Esc cancel ");
        crate::text_prompt::render(frame, &prompt, frame.area());
    }
    if let Some(message) = view.integration_message {
        overlays::render_integration_message(frame, message, view.integration_target);
    }
    if let Some(references) = view.references {
        overlays::render_references(frame, references, frame.area());
    }
}

#[cfg(test)]
mod tests;
