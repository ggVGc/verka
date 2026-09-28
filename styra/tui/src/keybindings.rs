//! Keyboard configuration for the terminal client.
//!
//! Edit the assignments in this file to customize the keyboard. Each section
//! maps physical keys to commands from [`crate::actions`]. Matching machinery
//! lives in [`crate::keyboard`], and command behavior lives in [`crate::input`].

use crate::actions::Action;
pub(crate) use crate::keyboard::ReferenceRow;
use crate::keyboard::{bindings, Binding, Key};
use crossterm::event::KeyCode;

bindings! { GLOBAL = "Global";
    HELP: [Key::ch('?')] => Action::Help;
    GLOBAL_FOCUS_MESSAGE: [Key::ch('i')] => Action::GlobalFocusMessage;
    GLOBAL_LEAVE_MESSAGE: [Key::code(KeyCode::Esc)] => Action::GlobalLeaveMessage;
    GLOBAL_QUIT: [Key::ch('q')] => Action::GlobalQuit;
    GLOBAL_INTERRUPT: [Key::ch('s')] => Action::GlobalInterrupt;
    GLOBAL_STOP: [Key::ch('S')] => Action::GlobalStop;
    GLOBAL_BRANCH: [Key::ch('B')] => Action::GlobalBranch;
    GLOBAL_NEXT_LIVE: [Key::ch('n')] => Action::GlobalNextLive;
    GLOBAL_NEW_SESSION: [Key::ch('N')] => Action::GlobalNewSession;
    GLOBAL_LAUNCHER: [Key::ch('l')] => Action::GlobalLauncher;
    GLOBAL_SHELL: [Key::ch('!')] => Action::GlobalShell;
    GLOBAL_DIRECTORY: [Key::ch('~')] => Action::GlobalDirectory;
    GLOBAL_INTERACTIONS: [Key::ch('a')] => Action::GlobalInteractions;
    GLOBAL_SESSIONS: [Key::ch('A')] => Action::GlobalSessions;
    GLOBAL_WORKSPACES: [Key::ch('V')] => Action::GlobalWorkspaces;
    GLOBAL_SESSION_WORKTREE: [Key::ch('W')] ("existing session")
        => Action::GlobalSessionWorktree;
    GLOBAL_TAGS: [Key::ch('T')] => Action::GlobalTags;
    GLOBAL_RAW: [Key::ch('r')] => Action::GlobalRaw;
    GLOBAL_LOG: [Key::ctrl('l')] => Action::GlobalLog;
    GLOBAL_TRANSCRIPT: [Key::ch('t')] => Action::GlobalTranscript;
    GLOBAL_DETAILS: [Key::ch('d')] => Action::GlobalDetails;
    GLOBAL_ENTRY_LOG: [Key::ch('e')] => Action::GlobalEntryLog;
    GLOBAL_ENTRY_LOG_FOCUS: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)]
        => Action::GlobalEntryLogFocus;
    GLOBAL_QUOTA: [Key::ch('Q')] => Action::GlobalQuota;
    GLOBAL_FILES: [Key::ch('F')] => Action::GlobalFiles;
    GLOBAL_FILES_ALIAS: [Key::ch('f')] ("other views")
        => Action::GlobalFiles;
    GLOBAL_ANSWER: [Key::ch('X')] => Action::GlobalAnswer;
    GLOBAL_PREVIEW: [Key::ch('P')] => Action::GlobalPreview;
    GLOBAL_COPY_CONVERSATION: [Key::ch('Y')] => Action::GlobalCopyConversation;
}

bindings! { EVENTS = "Events and previews";
    EVENTS_NEXT_ENTRY: [Key::ch('J'), Key::code(KeyCode::Down)] => Action::EventsNextEntry;
    EVENTS_PREV_ENTRY: [Key::ch('K'), Key::code(KeyCode::Up)] => Action::EventsPrevEntry;
    EVENTS_NEXT_LINE: [Key::ch('j')] => Action::EventsNextLine;
    EVENTS_PREV_LINE: [Key::ch('k')] => Action::EventsPrevLine;
    EVENTS_FIRST: [Key::ch('g')] => Action::EventsFirst;
    EVENTS_LAST: [Key::ch('G')] => Action::EventsLast;
    EVENTS_FOLLOW_BRANCH: [Key::ch('b')] ("or Enter on a branch marker")
        => Action::EventsFollowBranch;
    EVENTS_TOGGLE_EXPAND: [Key::ch(' '), Key::code(KeyCode::Enter), Key::ch('o')]
        => Action::EventsToggleExpand;
    EVENTS_EXPAND_ONLY: [Key::ch('O')] => Action::EventsExpandOnly;
    EVENTS_EXPAND_ALL: [Key::ch('R')] as "z R" => Action::EventsExpandAll;
    EVENTS_COLLAPSE_ALL: [Key::ch('M')] as "z M" => Action::EventsCollapseAll;
    EVENTS_MINOR: [Key::ch('m')] => Action::EventsMinor;
    EVENTS_PREVIEW_PANEL: [Key::ch('p')] => Action::EventsPreviewPanel;
    EVENTS_CONVERSATION_ONLY: [Key::ch('c')] => Action::EventsConversationOnly;
    EVENTS_PREVIEW_MODE: [Key::ch('v')] ("preview open") => Action::EventsPreviewMode;
    EVENTS_PREVIEW_TARGET: [Key::ch('C')] ("preview open") => Action::EventsPreviewTarget;
    EVENTS_COMPLETE: [Key::ch('C')] ("preview closed")
        => Action::EventsComplete;
    EVENTS_LINK_DESTINATIONS: [Key::ch('u')] => Action::EventsLinkDestinations;
    EVENTS_SEARCH: [Key::ch('/')]
        => Action::EventsSearch;
    EVENTS_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::EventsPageDown;
    EVENTS_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::EventsPageUp;
    EVENTS_LINKS: [Key::ch('f')]
        => Action::EventsLinks;
    EVENTS_COPY: [Key::ch('y')] => Action::EventsCopy;
}

bindings! { READING = "Raw, log, quota, and transcript";
    READING_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::ReadingDown;
    READING_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::ReadingUp;
    READING_FIRST: [Key::ch('g')] => Action::ReadingFirst;
    READING_LAST: [Key::ch('G')] => Action::ReadingLast;
    READING_LINKS: [Key::ch('f')] ("transcript")
        => Action::ReadingLinks;
    READING_CONVERSATION_ONLY: [Key::ch('c')] ("transcript")
        => Action::ReadingConversationOnly;
    READING_RETRY: [Key::ch('R')] ("quota")
        => Action::ReadingRetry;
    READING_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::ReadingPageDown;
    READING_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::ReadingPageUp;
    READING_PROVIDER_RAW: [Key::ch('v')] ("raw")
        => Action::ReadingProviderRaw;
    READING_COPY: [Key::ch('y')] ("raw") => Action::ReadingCopy;
}

bindings! { PREVIEW = "Full-screen preview";
    PREVIEW_SCROLL_DOWN: [Key::ch('j')] => Action::PreviewScrollDown;
    PREVIEW_SCROLL_UP: [Key::ch('k')] => Action::PreviewScrollUp;
    PREVIEW_NEXT_ENTRY: [Key::ch('J'), Key::code(KeyCode::Down)] => Action::PreviewNextEntry;
    PREVIEW_PREV_ENTRY: [Key::ch('K'), Key::code(KeyCode::Up)] => Action::PreviewPrevEntry;
    PREVIEW_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::PreviewPageDown;
    PREVIEW_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::PreviewPageUp;
    PREVIEW_FIRST: [Key::ch('g')] => Action::PreviewFirst;
    PREVIEW_LAST: [Key::ch('G')] => Action::PreviewLast;
    PREVIEW_MODE: [Key::ch('v')] => Action::PreviewMode;
    PREVIEW_TARGET: [Key::ch('C')] => Action::PreviewTarget;
    PREVIEW_LINKS: [Key::ch('f')] => Action::PreviewLinks;
    PREVIEW_LINK_DESTINATIONS: [Key::ch('u')] => Action::PreviewLinkDestinations;
    PREVIEW_COPY: [Key::ch('y')] => Action::PreviewCopy;
}

bindings! { DRIVA = "Details (Workspace, interaction, and launch policy)";
    DRIVA_SCOPE: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)]
        => Action::DrivaScope;
    DRIVA_NEXT_MOUNT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::DrivaNextMount;
    DRIVA_PREV_MOUNT: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::DrivaPrevMount;
    DRIVA_NETWORK: [Key::ch('w')] => Action::DrivaNetwork;
    DRIVA_ACCESS: [Key::ch('R')]
        => Action::DrivaAccess;
    DRIVA_TEMPLATES: [Key::ch('T')] => Action::DrivaTemplates;
    DRIVA_ADD_MOUNT: [Key::ch('m')] => Action::DrivaAddMount;
    DRIVA_GIT_CHECKOUT: [Key::ch('G')] => Action::DrivaGitCheckout;
    DRIVA_REMOVE_MOUNT: [Key::ch('x')] => Action::DrivaRemoveMount;
    DRIVA_IGNORE_WORKSPACE: [Key::ch('I')]
        => Action::DrivaIgnoreWorkspace;
    DRIVA_PROMOTE: [Key::ch('U')] => Action::DrivaPromote;
    DRIVA_SAVE_DEFAULT: [Key::ch('D')] => Action::DrivaSaveDefault;
    DRIVA_PAGE_DOWN: [Key::code(KeyCode::PageDown)]
        => Action::DrivaPageDown;
    DRIVA_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::DrivaPageUp;
}

bindings! { FILES = "Files";
    FILES_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::FilesNext;
    FILES_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::FilesPrev;
    FILES_NEXT_ENTRY: [Key::ch('J')] => Action::FilesNextEntry;
    FILES_PREV_ENTRY: [Key::ch('K')] => Action::FilesPrevEntry;
    FILES_FIRST: [Key::ch('g')] => Action::FilesFirst;
    FILES_LAST: [Key::ch('G')] => Action::FilesLast;
    FILES_EDIT: [Key::ch('e')] => Action::FilesEdit;
    FILES_PREVIEW: [Key::ch('p')] => Action::FilesPreview;
    FILES_SCOPE: [Key::ch('a')] => Action::FilesScope;
    FILES_COPY: [Key::ch('y')] => Action::FilesCopy;
}

bindings! { ANSWER = "Typed answer";
    ANSWER_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::AnswerNext;
    ANSWER_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::AnswerPrev;
    ANSWER_FIRST: [Key::ch('g')] => Action::AnswerFirst;
    ANSWER_LAST: [Key::ch('G')] => Action::AnswerLast;
    ANSWER_EDIT: [Key::ch('e')] => Action::AnswerEdit;
    ANSWER_COPY: [Key::ch('y')] => Action::AnswerCopy;
    ANSWER_AS_TEXT: [Key::ch('T')] => Action::AnswerAsText;
    ANSWER_AS_LINES: [Key::ch('L')] => Action::AnswerAsLines;
    ANSWER_AS_FILES: [Key::ch('F')] => Action::AnswerAsFiles;
    ANSWER_AS_JSON: [Key::ch('J')] => Action::AnswerAsJson;
    ANSWER_REREAD: [Key::ch('R')] => Action::AnswerReread;
}

bindings! { MESSAGE_EDITOR = "Message editor";
    EDITOR_RECORD: [Key::ctrl('r')]
        => Action::EditorRecord;
    EDITOR_RECORD_FINISH: [Key::code(KeyCode::Enter)] ("while recording")
        => Action::EditorRecordFinish;
    EDITOR_RECORD_CANCEL: [Key::code(KeyCode::Esc)] ("while recording") => Action::EditorRecordCancel;
    EDITOR_RECORD_LOUDER: [Key::code(KeyCode::Up), Key::ch('+'), Key::ch('=')] ("while recording")
        => Action::EditorRecordLouder;
    EDITOR_RECORD_QUIETER: [Key::code(KeyCode::Down), Key::ch('-')] ("while recording")
        => Action::EditorRecordQuieter;
    EDITOR_CONTRACT: [Key::ctrl('t')]
        => Action::EditorContract;
    EDITOR_SEND: [Key::code(KeyCode::Enter)] => Action::EditorSend;
    EDITOR_SEND_IN_BRANCH: [Key::ctrl_code(KeyCode::Enter)] ("first prompt")
        => Action::EditorSendInBranch;
    EDITOR_CHANGE_DIRECTORY: [] as "/cd <directory>"
        => Action::EditorChangeDirectory;
    EDITOR_NEWLINE: [Key::alt_code(KeyCode::Enter)] => Action::EditorNewline;
    EDITOR_HISTORY_OLDER: [Key::code(KeyCode::Up)] => Action::EditorHistoryOlder;
    EDITOR_HISTORY_NEWER: [Key::code(KeyCode::Down)] => Action::EditorHistoryNewer;
    EDITOR_DELETE_WORD: [Key::ctrl('w')] => Action::EditorDeleteWord;
    EDITOR_LAUNCHER: [Key::ctrl('l')]
        => Action::EditorLauncher;
    EDITOR_INSERT_PATH: [Key::ctrl('f')] => Action::EditorInsertPath;
    EDITOR_MOUNT_READABLE: [Key::ch('r')] ("unmounted path") => Action::EditorMountReadable;
    EDITOR_MOUNT_WRITABLE: [Key::ch('w')] ("unmounted path") => Action::EditorMountWritable;
    EDITOR_MOUNT_NEITHER: [Key::ch('n')] ("unmounted path") => Action::EditorMountNeither;
}

bindings! { INTERACTIONS = "Interactions";
    INTERACTIONS_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)]
        => Action::InteractionsNext;
    INTERACTIONS_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::InteractionsPrev;
    INTERACTIONS_NEXT_WORKSPACE: [Key::ch('J')]
        => Action::InteractionsNextWorkspace;
    INTERACTIONS_PREV_WORKSPACE: [Key::ch('K')]
        => Action::InteractionsPrevWorkspace;
    INTERACTIONS_NEXT_LIVE: [Key::ctrl('n')] => Action::InteractionsNextLive;
    INTERACTIONS_NEXT_WORKING: [Key::ch('N')]
        => Action::InteractionsNextWorking;
    INTERACTIONS_NEXT_IDLE: [Key::ctrl('a')] => Action::InteractionsNextIdle;
    INTERACTIONS_SCOPE: [Key::ch('w')] => Action::InteractionsScope;
    INTERACTIONS_COMPLETED: [Key::ch('c')] => Action::InteractionsCompleted;
    INTERACTIONS_COMPLETE: [Key::ch('C')] => Action::InteractionsComplete;
    INTERACTIONS_TAGS: [Key::ch('T')] => Action::InteractionsTags;
    INTERACTIONS_STOP: [Key::ch('S')] => Action::InteractionsStop;
    INTERACTIONS_DELETE: [Key::ch('D')] => Action::InteractionsDelete;
    INTERACTIONS_CLOSE: [Key::code(KeyCode::Enter), Key::ch('a'), Key::code(KeyCode::Esc)]
        => Action::InteractionsClose;
}

bindings! { BRANCH = "Branch from selected entry";
    BRANCH_NEXT: [Key::ch('j'), Key::ch('J'), Key::code(KeyCode::Down)]
        => Action::BranchNext;
    BRANCH_PREV: [Key::ch('k'), Key::ch('K'), Key::code(KeyCode::Up)] => Action::BranchPrev;
    BRANCH_CONFIRM: [Key::code(KeyCode::Enter)] => Action::BranchConfirm;
    BRANCH_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::BranchCancel;
}

bindings! { TAGS = "Interaction tags";
    TAGS_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::TagsNext;
    TAGS_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::TagsPrev;
    TAGS_TOGGLE: [Key::ch(' ')] => Action::TagsToggle;
    TAGS_NEW: [Key::ch('n')] => Action::TagsNew;
    TAGS_SAVE: [Key::code(KeyCode::Enter)] => Action::TagsSave;
    TAGS_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::TagsCancel;
}

bindings! { SESSION_PICKER = "Stored sessions";
    SESSIONS_HELP: [Key::ch('?')] => Action::SessionsHelp;
    SESSIONS_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::SessionsNext;
    SESSIONS_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::SessionsPrev;
    SESSIONS_NEXT_ROOT: [Key::ch('J')] => Action::SessionsNextRoot;
    SESSIONS_PREV_ROOT: [Key::ch('K')] => Action::SessionsPrevRoot;
    SESSIONS_FIRST: [Key::ch('g')] => Action::SessionsFirst;
    SESSIONS_LAST: [Key::ch('G')] => Action::SessionsLast;
    SESSIONS_OPEN: [Key::code(KeyCode::Enter)] => Action::SessionsOpen;
    SESSIONS_NEW: [Key::ch('n')] => Action::SessionsNew;
    SESSIONS_COMPLETED: [Key::ch('c')] => Action::SessionsCompleted;
    SESSIONS_COMPLETE: [Key::ch('C')] => Action::SessionsComplete;
    SESSIONS_FILTER: [Key::ch('/')] => Action::SessionsFilter;
    SESSIONS_SORT: [Key::ch('s')] => Action::SessionsSort;
    SESSIONS_OLDER: [Key::ch('a')] => Action::SessionsOlder;
    SESSIONS_RENAME: [Key::ch('r')] => Action::SessionsRename;
    SESSIONS_CONVERT: [Key::ch('x')] => Action::SessionsConvert;
    SESSIONS_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::SessionsCancel;
}

bindings! { WORKSPACE_PICKER = "Workspaces";
    WORKSPACES_HELP: [Key::ch('?')] => Action::WorkspacesHelp;
    WORKSPACES_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::WorkspacesNext;
    WORKSPACES_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::WorkspacesPrev;
    WORKSPACES_OPEN: [Key::code(KeyCode::Enter)] => Action::WorkspacesOpen;
    WORKSPACES_CREATE: [Key::ch('n')] => Action::WorkspacesCreate;
    WORKSPACES_RENAME: [Key::ch('r')] => Action::WorkspacesRename;
    WORKSPACES_FILTER: [Key::ch('/')] => Action::WorkspacesFilter;
    WORKSPACES_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::WorkspacesCancel;
}

bindings! { LAUNCHER = "Launch";
    LAUNCHER_HELP: [Key::ch('?')] => Action::LauncherHelp;
    LAUNCHER_NEXT: [Key::ch('j'), Key::ch('J'), Key::code(KeyCode::Down)] => Action::LauncherNext;
    LAUNCHER_PREV: [Key::ch('k'), Key::ch('K'), Key::code(KeyCode::Up)] => Action::LauncherPrev;
    LAUNCHER_NEXT_COLUMN: [Key::ch('l'), Key::code(KeyCode::Right), Key::code(KeyCode::Tab)]
        => Action::LauncherNextColumn;
    LAUNCHER_PREV_COLUMN: [Key::ch('h'), Key::code(KeyCode::Left), Key::code(KeyCode::BackTab)]
        => Action::LauncherPrevColumn;
    LAUNCHER_PROVIDER_DOWN: [Key::ch('p')] => Action::LauncherProviderDown;
    LAUNCHER_PROVIDER_UP: [Key::ch('P')] => Action::LauncherProviderUp;
    LAUNCHER_MODEL_DOWN: [Key::ch('m')] => Action::LauncherModelDown;
    LAUNCHER_MODEL_UP: [Key::ch('M')] => Action::LauncherModelUp;
    LAUNCHER_EFFORT_DOWN: [Key::ch('e')] => Action::LauncherEffortDown;
    LAUNCHER_EFFORT_UP: [Key::ch('E')] => Action::LauncherEffortUp;
    LAUNCHER_SELECT: [Key::code(KeyCode::Enter)] => Action::LauncherSelect;
    LAUNCHER_DEFAULT: [Key::ch('D')] => Action::LauncherDefault;
    LAUNCHER_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::LauncherCancel;
}

bindings! { TEMPLATE_PICKER = "Driva templates";
    TEMPLATES_HELP: [Key::ch('?')] => Action::TemplatesHelp;
    TEMPLATES_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::TemplatesNext;
    TEMPLATES_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::TemplatesPrev;
    TEMPLATES_TOGGLE: [Key::ch(' ')] => Action::TemplatesToggle;
    TEMPLATES_APPLY: [Key::code(KeyCode::Enter)] => Action::TemplatesApply;
    TEMPLATES_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::TemplatesCancel;
}

// Bindings that belong to no window's reference: the prefix of a chord, and
// the keys of the reference overlay itself, which says how to leave in its own
// footer rather than in a list of rows.
bindings! { REFERENCE_OVERLAY = "Reference";
    EVENTS_FOLD_PREFIX: [Key::ch('z')] => Action::EventsFoldPrefix;
    CLOSE_REFERENCE: [Key::ch('?'), Key::code(KeyCode::Esc), Key::ch('q')]
        => Action::CloseReference;
    REFERENCE_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::ReferenceDown;
    REFERENCE_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::ReferenceUp;
    REFERENCE_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::ReferencePageDown;
    REFERENCE_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::ReferencePageUp;
    REFERENCE_TOP: [Key::ch('g')] => Action::ReferenceTop;
}

/// A screen the operator can be looking at, and so a reference `?` can answer
/// with. The main application's windows are its views and its modal overlays;
/// the rest run their own loops before or beside a loaded session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Window {
    Events,
    Raw,
    Log,
    Quota,
    Transcript,
    Driva,
    Files,
    Answer,
    Preview,
    Interactions,
    Branch,
    Tags,
    SessionPicker,
    WorkspacePicker,
    Launcher,
    TemplatePicker,
}

impl Window {
    /// What the reference calls this window, shown in its frame so the
    /// operator can tell a window-specific reference from the one they saw on
    /// the screen before.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Raw => "raw",
            Self::Log => "log",
            Self::Quota => "quota",
            Self::Transcript => "transcript",
            Self::Driva => "details",
            Self::Files => "files",
            Self::Answer => "typed answer",
            Self::Preview => "preview",
            Self::Interactions => "interactions",
            Self::Branch => "branch",
            Self::Tags => "tags",
            Self::SessionPicker => "sessions",
            Self::WorkspacePicker => "Workspaces",
            Self::Launcher => "launch",
            Self::TemplatePicker => "Driva templates",
        }
    }

    /// The sections this window's reference is made of, most specific first:
    /// what the window itself does, then what still holds around it.
    fn sections(self) -> &'static [&'static [ReferenceRow]] {
        match self {
            Self::Events => &[EVENTS, MESSAGE_EDITOR, GLOBAL],
            Self::Raw | Self::Log | Self::Quota | Self::Transcript => {
                &[READING, MESSAGE_EDITOR, GLOBAL]
            }
            Self::Preview => &[PREVIEW, MESSAGE_EDITOR, GLOBAL],
            Self::Driva => &[DRIVA, GLOBAL],
            Self::Files => &[FILES, GLOBAL],
            Self::Answer => &[ANSWER, GLOBAL],
            Self::Interactions => &[INTERACTIONS, GLOBAL],
            Self::Branch => &[BRANCH, GLOBAL],
            Self::Tags => &[TAGS, GLOBAL],
            Self::SessionPicker => &[SESSION_PICKER],
            Self::WorkspacePicker => &[WORKSPACE_PICKER],
            Self::Launcher => &[LAUNCHER],
            Self::TemplatePicker => &[TEMPLATE_PICKER],
        }
    }

    /// This window's reference, in display order, with a blank line between
    /// sections.
    pub(crate) fn reference(self) -> Vec<ReferenceRow> {
        let mut rows = Vec::new();
        for section in self.sections() {
            if !rows.is_empty() {
                rows.push(ReferenceRow::Blank);
            }
            rows.extend_from_slice(section);
        }
        rows
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    const WINDOWS: [Window; 16] = [
        Window::Events,
        Window::Raw,
        Window::Log,
        Window::Quota,
        Window::Transcript,
        Window::Driva,
        Window::Files,
        Window::Answer,
        Window::Preview,
        Window::Interactions,
        Window::Branch,
        Window::Tags,
        Window::SessionPicker,
        Window::WorkspacePicker,
        Window::Launcher,
        Window::TemplatePicker,
    ];

    /// Every window answers `?` with something, and with its own commands
    /// first: the reference is for what is on screen, not a catalogue the
    /// operator has to search.
    #[test]
    fn every_window_leads_with_its_own_section() {
        for window in WINDOWS {
            let rows = window.reference();
            assert!(
                matches!(rows.first(), Some(ReferenceRow::Section(_))),
                "{} opens with a section heading",
                window.name()
            );
            assert!(
                rows.iter()
                    .any(|row| matches!(row, ReferenceRow::Binding(_))),
                "{} lists at least one binding",
                window.name()
            );
        }
    }

    /// A screen that stands on its own — no session under it — must still say
    /// how to leave and how to reach the reference again, because none of the
    /// global bindings apply there.
    #[test]
    fn standalone_screens_carry_their_own_help_and_cancel_keys() {
        for window in [
            Window::SessionPicker,
            Window::WorkspacePicker,
            Window::Launcher,
            Window::TemplatePicker,
        ] {
            let rows = window.reference();
            let help = KeyEvent::from(KeyCode::Char('?'));
            assert!(
                rows.iter().any(|row| matches!(
                    row,
                    ReferenceRow::Binding(binding) if binding.matches(help)
                )),
                "{} says how to reopen the reference",
                window.name()
            );
            assert!(
                rows.iter().any(|row| matches!(
                    row,
                    ReferenceRow::Binding(binding) if binding.action().description() == "cancel"
                )),
                "{} says how to leave",
                window.name()
            );
        }
    }

    /// Sections are separated by a blank line, and nothing else is: a window
    /// built from several sections must not run them together.
    #[test]
    fn sections_are_separated_by_a_blank_line() {
        let rows = Window::Events.reference();
        let headings = rows
            .iter()
            .filter(|row| matches!(row, ReferenceRow::Section(_)))
            .count();
        let blanks = rows
            .iter()
            .filter(|row| matches!(row, ReferenceRow::Blank))
            .count();

        assert_eq!(headings, 3, "events, message editor, global");
        assert_eq!(blanks, headings - 1);
    }

    /// The reference shows the key the dispatcher actually compares against,
    /// so a rebinding cannot leave the documentation behind.
    #[test]
    fn a_bindings_label_is_made_of_its_own_keys() {
        assert_eq!(GLOBAL_FILES.action(), Action::GlobalFiles);
        assert_eq!(GLOBAL_FILES_ALIAS.action(), Action::GlobalFiles);
        assert_eq!(EVENTS_NEXT_ENTRY.label(), "J/↓");
        assert_eq!(EVENTS_TOGGLE_EXPAND.label(), "Space/Enter/o");
        assert_eq!(EDITOR_NEWLINE.label(), "alt-Enter");
        assert_eq!(
            GLOBAL_SESSION_WORKTREE.label(),
            "W (existing session)",
            "a narrower binding says where it applies"
        );
        assert_eq!(EVENTS_EXPAND_ALL.label(), "z R", "a chord names its prefix");
    }

    /// Control is part of the binding, so ctrl-l is not plain `l`.
    #[test]
    fn modifiers_are_part_of_the_match() {
        let plain = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        let control = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);

        assert!(GLOBAL_LAUNCHER.matches(plain));
        assert!(!GLOBAL_LAUNCHER.matches(control));
        assert!(GLOBAL_LOG.matches(control));
        assert!(!GLOBAL_LOG.matches(plain));
    }

    /// Shift is not: a capital already arrives as its own character, and
    /// terminals disagree about reporting the modifier for punctuation.
    #[test]
    fn shift_is_ignored_so_capitals_and_punctuation_still_match() {
        assert!(GLOBAL_NEW_SESSION.matches(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)));
        assert!(GLOBAL_DIRECTORY.matches(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::SHIFT)));
    }
}
