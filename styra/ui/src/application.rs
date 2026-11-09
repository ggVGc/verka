//! Top-level main-application layout and overlay ordering.

use crate::{
    answer, busy, driva, event_list, files, footer, interactions, log, messages, modal_input,
    overlays, overview, preview, quota, raw, recording, transcript,
};

#[derive(Clone, Copy)]
pub enum SidePreview<'a> {
    Entry(&'a preview::PreviewView<'a>),
    CheckoutDiff(&'a dyn Panel),
}

pub struct EventView<'a> {
    pub list: &'a event_list::EventListView<'a>,
    pub navigator: Option<&'a interactions::InteractionNavigator<'a>>,
    pub entry_log: Option<&'a event_list::EntryLogView<'a>>,
    /// The sandbox's processes, in the bottom half below the event list.
    /// Takes the place of the entry log, which the client closes for it.
    pub processes: Option<&'a crate::processes::ProcessesView<'a>>,
    pub preview: Option<SidePreview<'a>>,
    /// Whether the checkout diff takes the whole view, the log and every
    /// other pane hidden, rather than the bottom of it.
    pub diff_maximized: bool,
    /// Whether link navigation is on, which washes out everything in the
    /// event list but the links and file references it walks.
    pub link_mode: bool,
}

pub struct FilesView<'a> {
    pub list: &'a event_list::EventListView<'a>,
    pub preview: Option<SidePreview<'a>>,
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
    Processes(&'a crate::processes::ProcessesView<'a>),
    Transcript(&'a transcript::TranscriptView<'a>),
    Driva(&'a driva::DrivaView<'a>),
    Files(FilesView<'a>),
    Answer(&'a answer::AnswerView<'a>),
    Preview(&'a preview::PreviewView<'a>),
    Overview(&'a overview::OverviewView<'a>),
}

/// A main view drawn by a crate built on this one rather than by this crate,
/// given the area the layout leaves it.
/// Terminal drawing is an optional extension to an external panel model.
/// Builds without a renderer retain the model boundary without frame types.
pub trait Panel {
    #[cfg(feature = "renderer")]
    fn render(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) -> PanelFeedback;
}

/// What a [`Panel`] measured while drawing, for the host to scroll by.
#[derive(Clone, Copy, Debug, Default)]
pub struct PanelFeedback {
    pub scroll: preview::PreviewFeedback,
    /// First visible row of a list the panel keeps beside its scrolling text.
    pub list_offset: Option<usize>,
    /// Effective diff layout and horizontal offset, after width-dependent fallback.
    pub diff_layout: Option<(bool, u16)>,
    /// The file selected on the diff's report page, the page's scroll and
    /// its file patch's, as drawn, while the page is open.
    pub diff_report_page: Option<(usize, u16, Option<u16>)>,
}

#[derive(Default)]
pub struct ApplicationOverlays<'a> {
    /// A rebase this client started. Not drawn over anything: it is a box
    /// across the bottom of the main panel, under the log the rebase's
    /// conflict-resolution turn is followed in.
    pub rebase_status: Option<&'a styra_protocol::RebaseStatus>,
    pub rename_prompt: Option<&'a str>,
    pub integration_message: Option<&'a str>,
    /// The branch the integration commit will land on, named in the
    /// message prompt's corner.
    pub integration_target: Option<&'a str>,
    pub launcher: Option<&'a crate::launcher::LauncherView>,
    pub input: Option<&'a modal_input::ModalInput<'a>>,
    /// A microphone capture in progress, which takes the message box's place:
    /// nothing is being typed while it runs, and the level is what the
    /// operator needs to see there instead.
    pub recording: Option<&'a recording::RecordingView>,
    pub references: Option<overlays::ReferencesView<'a>>,
    pub insert: Option<overlays::InsertPromptView<'a>>,
    pub link_menu: Option<overlays::LinkMenuView<'a>>,
    /// The current window's actions, searchable, opened with Space.
    pub action_menu: Option<overlays::ActionMenuView<'a>>,
    pub diff_branches: Option<crate::fuzzy_list::FuzzyListView<'a>>,
    /// The branch a new interaction's checkout starts from, or the one the
    /// current interaction is retargeted onto, both opened with alt-b.
    pub branches: Option<crate::fuzzy_list::FuzzyListView<'a>>,
    /// The Workspaces with live work, searchable, opened with `v`.
    pub workspace_jump: Option<overlays::WorkspaceJumpView<'a>>,
    pub tags: Option<overlays::TagPickerView<'a>>,
    /// A synchronous call the operator started, shown in the same bottom
    /// status box as a rebase. The input is hidden until the call returns.
    pub busy: Option<busy::BusyView<'a>>,
}

pub struct ApplicationView<'a> {
    pub session_id: &'a str,
    pub main: MainView<'a>,
    pub notices: &'a [messages::MessageView<'a>],
    pub footer: &'a footer::FooterView<'a>,
    pub overlays: ApplicationOverlays<'a>,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(all(test, feature = "renderer"))]
mod fixture_tests;
#[cfg(feature = "renderer")]
#[path = "render/application.rs"]
mod rendering;
