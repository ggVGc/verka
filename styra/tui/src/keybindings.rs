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
    GLOBAL_BACK: [Key::code(KeyCode::Esc)] ("other views") => Action::GlobalBack;
    GLOBAL_QUIT: [Key::ch('q')] => Action::GlobalQuit;
    GLOBAL_INTERRUPT: [Key::ch('s')] => Action::GlobalInterrupt;
    GLOBAL_STOP: [Key::ch('S')] => Action::GlobalStop;
    GLOBAL_BRANCH: [Key::ch('B')] => Action::GlobalBranch;
    GLOBAL_NEXT_LIVE: [Key::ch('n')] => Action::GlobalNextLive;
    GLOBAL_NEXT_WORKING: [Key::ch('N')] => Action::GlobalNextWorking;
    GLOBAL_NEW_SESSION: [Key::ctrl('n')] => Action::GlobalNewSession;
    GLOBAL_LAUNCHER: [Key::ch('l')] => Action::GlobalLauncher;
    GLOBAL_SHELL: [Key::ch('!')] => Action::GlobalShell;
    GLOBAL_DIRECTORY: [Key::ch('~')] => Action::GlobalDirectory;
    GLOBAL_DIFF: [Key::ch('d')] ("checkout with a branch point") => Action::GlobalDiff;
    GLOBAL_DIFF_EXTERNAL: [Key::ch('D')] ("not in details") => Action::GlobalDiffExternal;
    GLOBAL_INTERACTIONS: [Key::ch('a')] => Action::GlobalInteractions;
    GLOBAL_OVERVIEW: [Key::ch('v')] ("not in raw") => Action::GlobalOverview;
    GLOBAL_SESSIONS: [Key::ch('A')] => Action::GlobalSessions;
    GLOBAL_WORKSPACES: [Key::ch('V')] => Action::GlobalWorkspaces;
    GLOBAL_WORKTREES: [Key::ch('w')] ("not in details") => Action::GlobalWorktrees;
    GLOBAL_SESSION_WORKTREE: [Key::ch('W')] ("existing session")
        => Action::GlobalSessionWorktree;
    GLOBAL_TAGS: [Key::ch('T')] => Action::GlobalTags;
    GLOBAL_RAW: [Key::ch('r')] => Action::GlobalRaw;
    GLOBAL_LOG: [Key::ctrl('l')] => Action::GlobalLog;
    GLOBAL_TRANSCRIPT: [Key::ch('t')] => Action::GlobalTranscript;
    GLOBAL_DETAILS: [Key::ctrl('s')] => Action::GlobalDetails;
    GLOBAL_JUMP_BACK: [Key::ctrl('o')] => Action::GlobalJumpBack;
    GLOBAL_AUTO_COMMIT: [Key::ctrl('g')] => Action::GlobalAutoCommit;
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
    EVENTS_PREVIEW_SCROLL_DOWN: [Key::code(KeyCode::Down)] ("preview open")
        => Action::EventsPreviewScrollDown;
    EVENTS_PREVIEW_SCROLL_UP: [Key::code(KeyCode::Up)] ("preview open")
        => Action::EventsPreviewScrollUp;
    EVENTS_SCROLL_DOWN: [Key::ch('J')] => Action::EventsScrollDown;
    EVENTS_SCROLL_UP: [Key::ch('K')] => Action::EventsScrollUp;
    EVENTS_NEXT_ENTRY: [Key::code(KeyCode::Down)] => Action::EventsNextEntry;
    EVENTS_PREV_ENTRY: [Key::code(KeyCode::Up)] => Action::EventsPrevEntry;
    EVENTS_NEXT_LINE: [Key::ch('j')] => Action::EventsNextLine;
    EVENTS_PREV_LINE: [Key::ch('k')] => Action::EventsPrevLine;
    EVENTS_FIRST: [Key::ch('g')] => Action::EventsFirst;
    EVENTS_LAST: [Key::ch('G')] => Action::EventsLast;
    EVENTS_FOLLOW_BRANCH: [Key::ch('b')] ("or Enter on a branch marker")
        => Action::EventsFollowBranch;
    EVENTS_LINK_MENU: [Key::ch(' ')] ("link highlighted") => Action::EventsLinkMenu;
    EVENTS_TOGGLE_EXPAND: [Key::ch(' '), Key::code(KeyCode::Enter), Key::ch('o')]
        => Action::EventsToggleExpand;
    EVENTS_EXPAND_ONLY: [Key::ch('O')] => Action::EventsExpandOnly;
    EVENTS_EXPAND_ALL: [Key::ch('R')] as "z R" => Action::EventsExpandAll;
    EVENTS_COLLAPSE_ALL: [Key::ch('M')] as "z M" => Action::EventsCollapseAll;
    EVENTS_MINOR: [Key::ch('m')] => Action::EventsMinor;
    EVENTS_PREVIEW_PANEL: [Key::ch('p')] => Action::EventsPreviewPanel;
    EVENTS_ALL_EVENTS: [Key::ch('c')] => Action::EventsAllEvents;
    EVENTS_PREVIEW_TARGET: [Key::ch('C')] ("preview open") => Action::EventsPreviewTarget;
    EVENTS_COMPLETE: [Key::ch('C')] ("preview closed")
        => Action::EventsComplete;
    EVENTS_ABANDON: [Key::ch('Z')] => Action::EventsAbandon;
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
    READING_ALL_EVENTS: [Key::ch('c')] ("transcript")
        => Action::ReadingAllEvents;
    READING_RETRY: [Key::ch('R')] ("quota")
        => Action::ReadingRetry;
    READING_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::ReadingPageDown;
    READING_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::ReadingPageUp;
    READING_PROVIDER_RAW: [Key::ch('v')] ("raw")
        => Action::ReadingProviderRaw;
    READING_COPY: [Key::ch('y')] ("raw, diff") => Action::ReadingCopy;
}

bindings! { CHECKOUT_DIFF = "Checkout diff";
    DIFF_SEARCH: [Key::ch('/')] ("per-file view") => Action::DiffSearch;
    DIFF_TOGGLE_FILES: [Key::ch('f')] => Action::DiffToggleFiles;
    DIFF_TOGGLE_REMOVED: [Key::ch('h')] => Action::DiffToggleRemoved;
    DIFF_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::DiffDown;
    DIFF_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::DiffUp;
    DIFF_SCROLL_DOWN: [Key::ch('J')] => Action::DiffScrollDown;
    DIFF_SCROLL_UP: [Key::ch('K')] => Action::DiffScrollUp;
    DIFF_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::DiffPageDown;
    DIFF_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::DiffPageUp;
    DIFF_FIRST: [Key::ch('g')] => Action::ReadingFirst;
    DIFF_LAST: [Key::ch('G')] => Action::ReadingLast;
    DIFF_COPY: [Key::ch('y')] => Action::ReadingCopy;
}

bindings! { PREVIEW = "Full-screen preview";
    PREVIEW_SCROLL_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::PreviewScrollDown;
    PREVIEW_SCROLL_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::PreviewScrollUp;
    PREVIEW_NEXT_ENTRY: [Key::ch('J')] => Action::PreviewNextEntry;
    PREVIEW_PREV_ENTRY: [Key::ch('K')] => Action::PreviewPrevEntry;
    PREVIEW_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::PreviewPageDown;
    PREVIEW_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::PreviewPageUp;
    PREVIEW_FIRST: [Key::ch('g')] => Action::PreviewFirst;
    PREVIEW_LAST: [Key::ch('G')] => Action::PreviewLast;
    PREVIEW_TARGET: [Key::ch('C')] => Action::PreviewTarget;
    PREVIEW_LINKS: [Key::ch('f')] => Action::PreviewLinks;
    PREVIEW_LINK_DESTINATIONS: [Key::ch('u')] => Action::PreviewLinkDestinations;
    PREVIEW_COPY: [Key::ch('y')] => Action::PreviewCopy;
}

bindings! { DRIVA = "Details (Workspace, interaction, and launch policy)";
    DRIVA_TAB: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)] => Action::DrivaTab;
    DRIVA_SCOPE: [Key::code(KeyCode::Up), Key::code(KeyCode::Down)]
        => Action::DrivaScope;
    DRIVA_NEXT_MOUNT: [Key::ch('j')] => Action::DrivaNextMount;
    DRIVA_PREV_MOUNT: [Key::ch('k')] => Action::DrivaPrevMount;
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
    EDITOR_SEND_IN_BRANCH: [Key::ctrl_code(KeyCode::Enter)]
        => Action::EditorSendInBranch;
    EDITOR_CHANGE_DIRECTORY: [] as "/cd <directory>"
        => Action::EditorChangeDirectory;
    EDITOR_NEWLINE: [Key::alt_code(KeyCode::Enter)] => Action::EditorNewline;
    EDITOR_HISTORY_OLDER: [Key::code(KeyCode::Up)] => Action::EditorHistoryOlder;
    EDITOR_HISTORY_NEWER: [Key::code(KeyCode::Down)] => Action::EditorHistoryNewer;
    EDITOR_DELETE_WORD: [Key::ctrl('w')] => Action::EditorDeleteWord;
    EDITOR_ADD_BOX: [Key::ctrl('n')] => Action::EditorAddBox;
    EDITOR_CHOOSE_BOX: [Key::code(KeyCode::Esc)] ("several boxes") => Action::EditorChooseBox;
    EDITOR_BOX_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] ("choosing a box")
        => Action::EditorBoxNext;
    EDITOR_BOX_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] ("choosing a box")
        => Action::EditorBoxPrev;
    EDITOR_BOX_EDIT: [Key::code(KeyCode::Enter), Key::ch('i')] ("choosing a box")
        => Action::EditorBoxEdit;
    EDITOR_BOX_DELETE: [Key::ch('d')] ("choosing a box") => Action::EditorBoxDelete;
    EDITOR_BOX_LEAVE: [Key::code(KeyCode::Esc)] ("choosing a box") => Action::EditorBoxLeave;
    EDITOR_LAUNCHER: [Key::ctrl('l')]
        => Action::EditorLauncher;
    EDITOR_INSERT_PATH: [Key::ctrl('f')] => Action::EditorInsertPath;
    EDITOR_AUTO_COMMIT: [Key::ctrl('g')] => Action::GlobalAutoCommit;
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
    INTERACTIONS_NEXT_WORKING: [Key::ch('N'), Key::code(KeyCode::Tab)]
        => Action::InteractionsNextWorking;
    INTERACTIONS_PREV_WORKING: [Key::code(KeyCode::BackTab)]
        => Action::InteractionsPrevWorking;
    INTERACTIONS_NEXT_IDLE: [Key::ctrl('a')] => Action::InteractionsNextIdle;
    INTERACTIONS_SCOPE: [Key::ch('w')] => Action::InteractionsScope;
    INTERACTIONS_COMPLETED: [Key::ch('c')] => Action::InteractionsCompleted;
    INTERACTIONS_COMPLETE: [Key::ch('C')] => Action::InteractionsComplete;
    INTERACTIONS_ABANDON: [Key::ch('Z')] => Action::InteractionsAbandon;
    INTERACTIONS_TAGS: [Key::ch('T')] => Action::InteractionsTags;
    INTERACTIONS_STOP: [Key::ch('S')] => Action::InteractionsStop;
    INTERACTIONS_FILTER: [Key::ch('/')] => Action::InteractionsFilter;
    INTERACTIONS_TAG_FILTER: [Key::ctrl('t')] => Action::InteractionsTagFilter;
    INTERACTIONS_CLOSE: [Key::code(KeyCode::Enter), Key::ch('a'), Key::code(KeyCode::Esc)]
        => Action::InteractionsClose;
}

bindings! { OVERVIEW = "Overview";
    // `l` is the launcher everywhere else; here the grid needs all four
    // directions, so the arrows' vim keys win.
    OVERVIEW_LEFT: [Key::ch('h'), Key::code(KeyCode::Left)] => Action::OverviewLeft;
    OVERVIEW_RIGHT: [Key::ch('l'), Key::code(KeyCode::Right)] => Action::OverviewRight;
    OVERVIEW_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::OverviewDown;
    OVERVIEW_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::OverviewUp;
    OVERVIEW_NEXT: [Key::code(KeyCode::Tab)] => Action::OverviewNext;
    OVERVIEW_PREV: [Key::code(KeyCode::BackTab)] => Action::OverviewPrev;
    OVERVIEW_FIRST: [Key::ch('g')] => Action::OverviewFirst;
    OVERVIEW_LAST: [Key::ch('G')] => Action::OverviewLast;
    OVERVIEW_OPEN: [Key::code(KeyCode::Enter)] => Action::OverviewOpen;
    OVERVIEW_MESSAGE: [Key::ch('i')] => Action::OverviewMessage;
    OVERVIEW_RUNNING_ONLY: [Key::ch('r')] => Action::OverviewRunningOnly;
    OVERVIEW_CLOSE: [Key::ch('v'), Key::code(KeyCode::Esc)] => Action::OverviewClose;
}

bindings! { BRANCH = "Branch from selected entry";
    BRANCH_NEXT: [Key::ch('j'), Key::ch('J'), Key::code(KeyCode::Down)]
        => Action::BranchNext;
    BRANCH_PREV: [Key::ch('k'), Key::ch('K'), Key::code(KeyCode::Up)] => Action::BranchPrev;
    BRANCH_CONFIRM: [Key::code(KeyCode::Enter)] => Action::BranchConfirm;
    BRANCH_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::BranchCancel;
}

bindings! { LINK_MENU = "Actions on the highlighted link";
    LINK_MENU_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::LinkMenuNext;
    LINK_MENU_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::LinkMenuPrev;
    LINK_MENU_CONFIRM: [Key::code(KeyCode::Enter)] => Action::LinkMenuConfirm;
    LINK_MENU_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::LinkMenuCancel;
}

bindings! { TAGS = "Interaction tags";
    // The list is typed at, as the launcher's is, so letters are the query and
    // the commands are on Space, Enter, Esc, the arrows and control chords.
    // Space can stay a command because the filter ignores whitespace.
    TAGS_FILTER: [] as "any letter" => Action::TagsFilter;
    TAGS_NEXT: [Key::code(KeyCode::Down), Key::ctrl('j')] => Action::TagsNext;
    TAGS_PREV: [Key::code(KeyCode::Up), Key::ctrl('k')] => Action::TagsPrev;
    TAGS_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::TagsPageDown;
    TAGS_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::TagsPageUp;
    TAGS_DELETE_WORD: [Key::ctrl('w')] => Action::TagsDeleteWord;
    TAGS_TOGGLE: [Key::ch(' ')] => Action::TagsToggle;
    TAGS_CLEAR: [Key::ctrl('t')] => Action::TagsClear;
    TAGS_NEW: [Key::ctrl('n')] => Action::TagsNew;
    TAGS_SAVE: [Key::code(KeyCode::Enter)] => Action::TagsSave;
    TAGS_CANCEL: [Key::code(KeyCode::Esc)] => Action::TagsCancel;
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
    SESSIONS_ABANDON: [Key::ch('Z')] => Action::SessionsAbandon;
    SESSIONS_FILTER: [Key::ch('/')] => Action::SessionsFilter;
    SESSIONS_SORT: [Key::ch('s')] => Action::SessionsSort;
    SESSIONS_OLDER: [Key::ch('a')] => Action::SessionsOlder;
    SESSIONS_RENAME: [Key::ch('r')] => Action::SessionsRename;
    SESSIONS_CONVERT: [Key::ch('x')] => Action::SessionsConvert;
    SESSIONS_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => Action::SessionsCancel;
}

bindings! { WORKSPACE_PICKER = "Workspaces";
    // The list is typed at, as the launcher's is, so letters are the query and
    // the commands are on Enter, Esc, the arrows and control chords.
    WORKSPACES_FILTER: [] as "any letter" => Action::WorkspacesFilter;
    // `?` stays a command, as in the launcher: a Workspace name that needs
    // one to be told apart is not worth giving up the reference for.
    WORKSPACES_HELP: [Key::ch('?')] => Action::WorkspacesHelp;
    WORKSPACES_NEXT: [Key::code(KeyCode::Down), Key::ctrl('j')] => Action::WorkspacesNext;
    WORKSPACES_PREV: [Key::code(KeyCode::Up), Key::ctrl('k')] => Action::WorkspacesPrev;
    WORKSPACES_DELETE_WORD: [Key::ctrl('w')] => Action::WorkspacesDeleteWord;
    WORKSPACES_OPEN: [Key::code(KeyCode::Enter)] => Action::WorkspacesOpen;
    WORKSPACES_NEW: [Key::ctrl('n')] => Action::WorkspacesNew;
    WORKSPACES_CREATE: [Key::ctrl('c')] => Action::WorkspacesCreate;
    WORKSPACES_RENAME: [Key::ctrl('r')] => Action::WorkspacesRename;
    WORKSPACES_DIRECTORY: [Key::ctrl('d')] => Action::WorkspacesDirectory;
    WORKSPACES_CANCEL: [Key::code(KeyCode::Esc)] => Action::WorkspacesCancel;
}

bindings! { WORKTREE_PICKER = "Worktrees";
    // Typed at, as the Workspace list is: letters are the query, and the
    // commands are on Enter, Esc, Tab, the arrows and control chords.
    WORKTREES_FILTER: [] as "any letter" => Action::WorktreesFilter;
    WORKTREES_HELP: [Key::ch('?')] => Action::WorktreesHelp;
    WORKTREES_NEXT: [Key::code(KeyCode::Down), Key::ctrl('j')] => Action::WorktreesNext;
    WORKTREES_PREV: [Key::code(KeyCode::Up), Key::ctrl('k')] => Action::WorktreesPrev;
    WORKTREES_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::WorktreesPageDown;
    WORKTREES_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::WorktreesPageUp;
    WORKTREES_DELETE_WORD: [Key::ctrl('w')] => Action::WorktreesDeleteWord;
    WORKTREES_SCOPE: [Key::code(KeyCode::Tab)] => Action::WorktreesScope;
    WORKTREES_OPEN: [Key::code(KeyCode::Enter)] => Action::WorktreesOpen;
    WORKTREES_CANCEL: [Key::code(KeyCode::Esc)] => Action::WorktreesCancel;
}

bindings! { LAUNCHER = "Launch";
    // The list is typed at, so every printable key is a letter of the query
    // and none of them can also be a command. What is left is Enter, Esc, the
    // arrows and control chords — which is what a launch needs anyway.
    LAUNCHER_FILTER: [] as "any letter" => Action::LauncherFilter;
    // `?` is safe as a command here: no agent, model or rung name contains
    // one, so it can never be a letter of the query.
    LAUNCHER_HELP: [Key::ch('?')] => Action::LauncherHelp;
    LAUNCHER_NEXT: [Key::code(KeyCode::Down), Key::ctrl('n')] => Action::LauncherNext;
    LAUNCHER_PREV: [Key::code(KeyCode::Up), Key::ctrl('p')] => Action::LauncherPrev;
    LAUNCHER_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::LauncherPageDown;
    LAUNCHER_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::LauncherPageUp;
    LAUNCHER_DELETE_WORD: [Key::ctrl('w')] => Action::LauncherDeleteWord;
    LAUNCHER_SELECT: [Key::code(KeyCode::Enter)] => Action::LauncherSelect;
    LAUNCHER_DEFAULT: [Key::ctrl('d')] => Action::LauncherDefault;
    LAUNCHER_CANCEL: [Key::code(KeyCode::Esc)] => Action::LauncherCancel;
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
    CheckoutDiff,
    Driva,
    Files,
    Answer,
    Preview,
    Overview,
    Interactions,
    Branch,
    LinkMenu,
    Tags,
    SessionPicker,
    WorkspacePicker,
    WorktreePicker,
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
            Self::CheckoutDiff => "diff",
            Self::Driva => "details",
            Self::Files => "files",
            Self::Answer => "typed answer",
            Self::Preview => "preview",
            Self::Overview => "overview",
            Self::Interactions => "interactions",
            Self::Branch => "branch",
            Self::LinkMenu => "link actions",
            Self::Tags => "tags",
            Self::SessionPicker => "sessions",
            Self::WorkspacePicker => "Workspaces",
            Self::WorktreePicker => "worktrees",
            Self::Launcher => "launch",
            Self::TemplatePicker => "Driva templates",
        }
    }

    /// The sections this window's reference is made of, most specific first:
    /// what the window itself does, then what still holds around it.
    fn sections(self) -> &'static [&'static [ReferenceRow]] {
        match self {
            Self::Events => &[EVENTS, MESSAGE_EDITOR, GLOBAL],
            Self::CheckoutDiff => &[CHECKOUT_DIFF, MESSAGE_EDITOR, GLOBAL],
            Self::Raw | Self::Log | Self::Quota | Self::Transcript => {
                &[READING, MESSAGE_EDITOR, GLOBAL]
            }
            Self::Preview => &[PREVIEW, MESSAGE_EDITOR, GLOBAL],
            Self::Driva => &[DRIVA, GLOBAL],
            Self::Files => &[FILES, GLOBAL],
            Self::Answer => &[ANSWER, GLOBAL],
            Self::Overview => &[OVERVIEW, GLOBAL],
            Self::Interactions => &[INTERACTIONS, GLOBAL],
            Self::Branch => &[BRANCH, GLOBAL],
            Self::LinkMenu => &[LINK_MENU, GLOBAL],
            Self::Tags => &[TAGS, GLOBAL],
            Self::SessionPicker => &[SESSION_PICKER],
            Self::WorkspacePicker => &[WORKSPACE_PICKER],
            Self::WorktreePicker => &[WORKTREE_PICKER],
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

    const WINDOWS: [Window; 20] = [
        Window::Events,
        Window::Raw,
        Window::Log,
        Window::Quota,
        Window::Transcript,
        Window::CheckoutDiff,
        Window::Driva,
        Window::Files,
        Window::Answer,
        Window::Preview,
        Window::Overview,
        Window::Interactions,
        Window::Branch,
        Window::LinkMenu,
        Window::Tags,
        Window::SessionPicker,
        Window::WorkspacePicker,
        Window::WorktreePicker,
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
            Window::WorktreePicker,
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
        assert_eq!(EVENTS_SCROLL_DOWN.label(), "J");
        assert_eq!(EVENTS_NEXT_ENTRY.label(), "↓");
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
        assert!(GLOBAL_NEXT_WORKING.matches(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)));
        assert!(GLOBAL_DIRECTORY.matches(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::SHIFT)));
    }
}
