//! Top-level main-application layout and overlay ordering.

use crate::{
    answer, driva, event_list, files, footer, interactions, launcher, log, messages, modal_input, overlays,
    preview, quota, raw, recording, transcript, PanelId, RenderFeedback, ScrollFeedback,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

pub struct EventView<'a> {
    pub list: &'a event_list::EventListView<'a>,
    pub navigator: Option<&'a interactions::InteractionNavigator<'a>>,
    pub entry_log: Option<&'a event_list::EntryLogView<'a>>,
    pub preview: Option<&'a preview::PreviewView<'a>>,
}

pub struct FilesView<'a> {
    pub list: &'a event_list::EventListView<'a>,
    pub preview: Option<&'a preview::PreviewView<'a>>,
    pub files: &'a [files::FileView],
    pub selected: usize,
    pub scope: &'a str,
    pub selected_name: Option<&'a str>,
    pub content: files::FileContent<'a>,
}

pub enum MainView<'a> {
    Events(EventView<'a>),
    Raw(&'a raw::RawView<'a>),
    Log {
        chrome: &'a crate::chrome::PanelChrome,
        entries: &'a [log::LogEntryView<'a>],
        scroll_back: usize,
    },
    Quota(&'a quota::QuotaView<'a>),
    Transcript(&'a transcript::TranscriptView<'a>),
    Driva(&'a driva::DrivaView<'a>),
    Files(FilesView<'a>),
    Answer(&'a answer::AnswerView<'a>),
    Preview(&'a preview::PreviewView<'a>),
}

#[derive(Default)]
pub struct ApplicationOverlays<'a> {
    pub launcher: Option<&'a crate::launcher::LauncherView>,
    pub input: Option<&'a modal_input::ModalInput<'a>>,
    /// A microphone capture in progress, which takes the message box's place:
    /// nothing is being typed while it runs, and the level is what the
    /// operator needs to see there instead.
    pub recording: Option<&'a recording::RecordingView>,
    pub references: Option<overlays::ReferencesView<'a>>,
    pub insert: Option<overlays::InsertPromptView<'a>>,
    pub branch: Option<overlays::BranchPromptView>,
    pub tags: Option<overlays::TagPickerView<'a>>,
}

pub struct ApplicationView<'a> {
    pub session_id: &'a str,
    pub main: MainView<'a>,
    pub notices: &'a [String],
    pub footer: &'a footer::FooterView<'a>,
    pub overlays: ApplicationOverlays<'a>,
}

pub fn render(frame: &mut Frame, view: &ApplicationView<'_>) -> RenderFeedback {
    let mut feedback = RenderFeedback::default();
    if let MainView::Preview(preview) = &view.main {
        let measured = preview::render(frame, preview, frame.area());
        note_scroll(
            &mut feedback,
            PanelId::Preview {
                session: view.session_id.into(),
                target: preview_panel(preview),
            },
            measured.limit,
            measured.effective_scroll,
        );
        render_reading_overlays(frame, &view.overlays);
        return feedback;
    }
    let message_height =
        messages::height(view.notices.len()).min(frame.area().height.saturating_sub(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(message_height),
            Constraint::Length(1),
        ])
        .split(frame.area());
    match &view.main {
        MainView::Events(events) => {
            render_events(frame, events, view.session_id, chunks[0], &mut feedback)
        }
        MainView::Raw(raw_view) => {
            let measured = raw::render(frame, raw_view, chunks[0]);
            note_scroll(
                &mut feedback,
                PanelId::Preview {
                    session: view.session_id.into(),
                    target: match raw_view.source {
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
            chunks[0],
        ),
        MainView::Quota(quota_view) => quota::render(frame, quota_view, chunks[0]),
        MainView::Transcript(transcript_view) => {
            let limit = transcript::render(frame, transcript_view, chunks[0]);
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
            let measured = driva::render(frame, driva_view, chunks[0]);
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
            render_files(frame, files_view, view.session_id, chunks[0], &mut feedback)
        }
        MainView::Answer(answer_view) => answer::render(frame, answer_view, chunks[0]),
        MainView::Preview(_) => unreachable!(),
    }
    if message_height > 0 {
        messages::render(frame, view.notices, chunks[1]);
    }
    footer::render(frame, view.footer, chunks[2]);
    if let Some(capture) = view.overlays.recording {
        recording::render(frame, capture);
    } else if let Some(input) = view.overlays.input {
        modal_input::render(frame, input);
    }
    render_reading_overlays(frame, &view.overlays);
    if let Some(insert) = &view.overlays.insert {
        overlays::render_insert(frame, Some(insert.clone()), frame.area());
    }
    if let Some(branch) = view.overlays.branch {
        overlays::render_branch(frame, branch, frame.area());
    }
    if let Some(tags) = view.overlays.tags {
        overlays::render_tags(frame, tags);
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
    let interaction_area = if view.preview.is_some() && view.entry_log.is_some() {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        let measured = preview::render(frame, view.preview.unwrap(), panes[1]);
        note_scroll(
            feedback,
            PanelId::Preview {
                session: session_id.into(),
                target: preview_panel(view.preview.unwrap()),
            },
            measured.limit,
            measured.effective_scroll,
        );
        panes[0]
    } else {
        area
    };
    let event_area = if let Some(navigator) = view.navigator {
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(interactions::height(navigator, interaction_area.height)),
                Constraint::Min(1),
            ])
            .split(interaction_area);
        interactions::render(frame, navigator, panes[0]);
        panes[1]
    } else {
        interaction_area
    };
    if let Some(entry_log) = view.entry_log {
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(event_area);
        feedback.list_offset =
            Some(event_list::render(frame, view.list, panes[0]).effective_offset);
        let measured = event_list::render_entry_log(frame, entry_log, panes[1]);
        note_scroll(
            feedback,
            PanelId::EntryLog,
            measured.limit,
            measured.effective_scroll,
        );
    } else {
        let list_area = if let Some(preview_view) = view.preview {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(event_area);
            let measured = preview::render(frame, preview_view, panes[1]);
            note_scroll(
                feedback,
                PanelId::Preview {
                    session: session_id.into(),
                    target: preview_panel(preview_view),
                },
                measured.limit,
                measured.effective_scroll,
            );
            panes[0]
        } else {
            event_area
        };
        feedback.list_offset =
            Some(event_list::render(frame, view.list, list_area).effective_offset);
    }
}

fn render_files(
    frame: &mut Frame,
    view: &FilesView<'_>,
    session_id: &str,
    area: Rect,
    feedback: &mut RenderFeedback,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(columns[0]);
    feedback.list_offset = Some(event_list::render(frame, view.list, left[0]).effective_offset);
    let content_area = if let Some(preview_view) = view.preview {
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(columns[1]);
        let measured = preview::render(frame, preview_view, right[0]);
        note_scroll(
            feedback,
            PanelId::Preview {
                session: session_id.into(),
                target: preview_panel(preview_view),
            },
            measured.limit,
            measured.effective_scroll,
        );
        right[1]
    } else {
        columns[1]
    };
    files::render_tree(frame, view.files, view.selected, view.scope, left[1]);
    files::render_content(frame, view.selected_name, view.content, content_area);
}

fn preview_panel(view: &preview::PreviewView<'_>) -> crate::PreviewPanel {
    match view.target {
        preview::PreviewTarget::Selection => crate::PreviewPanel::Selection,
        preview::PreviewTarget::Command => crate::PreviewPanel::Command,
    }
}

fn note_scroll(feedback: &mut RenderFeedback, panel: PanelId, limit: u16, effective_offset: u16) {
    feedback.scroll.push(ScrollFeedback {
        panel,
        limit,
        effective_offset,
    });
}

fn render_reading_overlays(frame: &mut Frame, view: &ApplicationOverlays<'_>) {
    if let Some(references) = view.references {
        overlays::render_references(frame, references, frame.area());
    }
}
