//! Keyboard configuration for the terminal client.
//!
//! Edit the assignments in this file to customize the keyboard. Each section
//! maps physical keys to commands from [`crate::actions`]. Matching machinery
//! lives in [`crate::keyboard`], and command behavior lives in [`crate::input`].

use crate::actions::Action;
pub(crate) use crate::keyboard::ReferenceRow;
use crate::keyboard::{bindings, Binding, Key, Windows};
use crossterm::event::KeyCode;

bindings! { GLOBAL = "Global";
    HELP: [Key::ch('?')] => Action::Help;
    // A highlighted link keeps Space for its own menu of actions.
    GLOBAL_ACTION_MENU: [Key::ch(' ')] => Action::GlobalActionMenu;
    GLOBAL_FOCUS_MESSAGE: [Key::ch('i')] except [Window::Preview] => Action::GlobalFocusMessage;
    GLOBAL_LEAVE_MESSAGE: [Key::code(KeyCode::Esc)] => Action::GlobalLeaveMessage;
    GLOBAL_BACK: [Key::code(KeyCode::Esc)] except [Window::Events] => Action::GlobalBack;
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
    GLOBAL_DIFF_FULLSCREEN: [Key::ch('D')] => Action::GlobalDiffFullscreen;
    GLOBAL_INTERACTIONS: [Key::ch('a')] => Action::GlobalInteractions;
    GLOBAL_OVERVIEW: [Key::ch('o')] => Action::GlobalOverview;
    GLOBAL_SESSIONS: [Key::ch('A')] => Action::GlobalSessions;
    GLOBAL_WORKSPACES: [Key::ctrl('v')] => Action::GlobalWorkspaces;
    GLOBAL_WORKSPACE_JUMP: [Key::ch('v')] => Action::GlobalWorkspaceJump;
    GLOBAL_WORKSPACE_JUMP_LISTED: [Key::ch('V')] => Action::GlobalWorkspaceJumpListed;
    GLOBAL_WORKTREES: [Key::ch('w')] => Action::GlobalWorktrees;
    GLOBAL_SESSION_WORKTREE: [Key::ctrl('w')] ("existing session")
        => Action::GlobalSessionWorktree;
    GLOBAL_TAGS: [Key::ch('T')] => Action::GlobalTags;
    GLOBAL_REBASE: [Key::ctrl('b')] => Action::GlobalRebase;
    GLOBAL_RETARGET: [Key::alt_code(KeyCode::Char('b'))] => Action::GlobalRetarget;
    GLOBAL_INTEGRATE: [Key::ch('I')] => Action::GlobalIntegrate;
    GLOBAL_MESSAGES: [Key::ch('M')] => Action::GlobalMessages;
    GLOBAL_RAW: [Key::ctrl('r')] => Action::GlobalRaw;
    GLOBAL_LOG: [Key::ctrl('l')] => Action::GlobalLog;
    GLOBAL_TRANSCRIPT: [Key::ch('t')] => Action::GlobalTranscript;
    GLOBAL_DETAILS: [Key::ctrl('s')] => Action::GlobalDetails;
    GLOBAL_JUMP_BACK: [Key::ctrl('o')] => Action::GlobalJumpBack;
    GLOBAL_JUMP_FORWARD: [Key::ctrl('u')] => Action::GlobalJumpForward;
    GLOBAL_INTERACTION_HISTORY: [Key::alt_code(KeyCode::Char('o'))]
        => Action::GlobalInteractionHistory;
    GLOBAL_AUTO_COMMIT: [Key::ctrl('g')] => Action::GlobalAutoCommit;
    GLOBAL_ENTRY_LOG: [Key::ch('e')] => Action::GlobalEntryLog;
    GLOBAL_QUOTA: [Key::ch('Q')] => Action::GlobalQuota;
    GLOBAL_PROCESSES_PANE: [Key::ch('h')] => Action::GlobalProcessesPane;
    GLOBAL_PROCESSES: [Key::ch('H')] => Action::GlobalProcesses;
    GLOBAL_FILES: [Key::ch('F')] => Action::GlobalFiles;
    GLOBAL_FILES_ALIAS: [Key::ch('f')] => Action::GlobalFiles;
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
    EVENTS_LINK_MENU: [Key::ch(' ')] ("link highlighted") => Action::EventsLinkMenu;
    EVENTS_TOGGLE_EXPAND: [Key::code(KeyCode::Enter)]
        => Action::EventsToggleExpand;
    EVENTS_ENTRY_LOG_FOCUS: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)]
        ("entry log open") => Action::EventsEntryLogFocus;
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
    EVENTS_RENAME: [Key::alt_code(KeyCode::Char('r'))] => Action::EventsRename;
    EVENTS_RUN: [Key::ch('r')] => Action::EventsRun;
    EVENTS_TEST: [Key::ch('R')] => Action::EventsTest;
}

bindings! { READING = "Raw, messages, log, quota, processes, and transcript";
    READING_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::ReadingDown;
    READING_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => Action::ReadingUp;
    READING_FIRST: [Key::ch('g')] except [Window::Quota] => Action::ReadingFirst;
    READING_LAST: [Key::ch('G')] except [Window::Quota] => Action::ReadingLast;
    READING_LINKS: [Key::ch('f')] in [Window::Transcript] => Action::ReadingLinks;
    READING_ALL_EVENTS: [Key::ch('c')] in [Window::Transcript] => Action::ReadingAllEvents;
    READING_RETRY: [Key::ch('R')] in [Window::Quota] => Action::ReadingRetry;
    READING_PAGE_DOWN: [Key::code(KeyCode::PageDown)] in [Window::Raw, Window::Messages]
        => Action::ReadingPageDown;
    READING_PAGE_UP: [Key::code(KeyCode::PageUp)] in [Window::Raw, Window::Messages]
        => Action::ReadingPageUp;
    READING_PROVIDER_RAW: [Key::ch('v')] in [Window::Raw] => Action::ReadingProviderRaw;
    READING_COPY: [Key::ch('y')] in [Window::Raw, Window::Messages, Window::Processes]
        => Action::ReadingCopy;
}

bindings! { CHECKOUT_DIFF = "Diff";
    DIFF_CONVERSATION: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)] ("fullscreen")
        => Action::DiffConversation;
    DIFF_BRANCH: [Key::ch('b')] => Action::DiffBranch;
    DIFF_TURN: [Key::ch('u')] => Action::DiffTurn;
    DIFF_TURN_HIGHLIGHT: [Key::ch('H')] => Action::DiffTurnHighlight;
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
    DRIVA_RUN_COMMAND: [Key::ch('c')] => Action::DrivaRunCommand;
    DRIVA_TEST_COMMAND: [Key::ch('C')] => Action::DrivaTestCommand;
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
    EDITOR_CYCLE_PROVIDER: [Key::ctrl('p')]
        => Action::EditorCycleProvider;
    EDITOR_CYCLE_MODEL: [Key::ctrl('o')]
        => Action::EditorCycleModel;
    EDITOR_CYCLE_EFFORT: [Key::ctrl('e')]
        => Action::EditorCycleEffort;
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
    INTERACTIONS_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::InteractionsPageDown;
    INTERACTIONS_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::InteractionsPageUp;
    INTERACTIONS_FIRST: [Key::ch('g')] => Action::InteractionsFirst;
    INTERACTIONS_LAST: [Key::ch('G')] => Action::InteractionsLast;
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
    INTERACTIONS_STOPPED: [Key::ch('s')] => Action::InteractionsStopped;
    INTERACTIONS_RESPONSES: [Key::ch('m')] => Action::InteractionsResponses;
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
    OVERVIEW_NTH: [
        Key::ch('1'), Key::ch('2'), Key::ch('3'), Key::ch('4'), Key::ch('5'),
        Key::ch('6'), Key::ch('7'), Key::ch('8'), Key::ch('9'),
    ] as "1-9" => Action::OverviewNth;
    OVERVIEW_OPEN: [Key::code(KeyCode::Enter)] => Action::OverviewOpen;
    OVERVIEW_MESSAGE: [Key::ch('i')] => Action::OverviewMessage;
    OVERVIEW_SCOPE: [Key::ch('w')] => Action::InteractionsScope;
    OVERVIEW_RUNNING_ONLY: [Key::ch('r')] => Action::OverviewRunningOnly;
    OVERVIEW_SEARCH: [Key::ch('/')] => Action::OverviewSearch;
    OVERVIEW_CLOSE: [Key::ch('o'), Key::code(KeyCode::Esc)] => Action::OverviewClose;
}

bindings! { BRANCH = "Branch from selected entry";
    BRANCH_NEXT: [Key::ch('j'), Key::ch('J'), Key::code(KeyCode::Down)]
        => Action::BranchNext;
    BRANCH_PREV: [Key::ch('k'), Key::ch('K'), Key::code(KeyCode::Up)] => Action::BranchPrev;
    BRANCH_CHANGE: [Key::code(KeyCode::Left), Key::code(KeyCode::Right), Key::ch('h'), Key::ch('l'), Key::ch(' '), Key::code(KeyCode::Tab)] => Action::BranchChange;
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
    // Typing into a query — letters, Backspace, ctrl-w — is
    // `styra_ui::query`'s, the same in every search and filter field, so the
    // pickers' ctrl-w rows only describe it.
    TAGS_DELETE_WORD: [] as "ctrl-w" => Action::TagsDeleteWord;
    TAGS_TOGGLE: [Key::ch(' ')] => Action::TagsToggle;
    TAGS_EXCLUDE: [Key::ch('-')] ("filtering interactions") => Action::TagsExclude;
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
    WORKSPACES_DELETE_WORD: [] as "ctrl-w" => Action::WorkspacesDeleteWord;
    WORKSPACES_OPEN: [Key::code(KeyCode::Enter)] => Action::WorkspacesOpen;
    WORKSPACES_NEW: [Key::ctrl('n')] => Action::WorkspacesNew;
    WORKSPACES_CREATE: [Key::ctrl('c')] => Action::WorkspacesCreate;
    WORKSPACES_RENAME: [Key::ctrl('r')] => Action::WorkspacesRename;
    WORKSPACES_DIRECTORY: [Key::ctrl('d')] => Action::WorkspacesDirectory;
    WORKSPACES_DELETE: [Key::ctrl('x')] => Action::WorkspacesDelete;
    WORKSPACES_DELETED: [Key::ctrl('t')] => Action::WorkspacesDeleted;
    WORKSPACES_RESTORE: [Key::ctrl('u')] => Action::WorkspacesRestore;
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
    WORKTREES_DELETE_WORD: [] as "ctrl-w" => Action::WorktreesDeleteWord;
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
    LAUNCHER_DELETE_WORD: [] as "ctrl-w" => Action::LauncherDeleteWord;
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

bindings! { ACTION_MENU = "Actions";
    // Typed at, as the launcher is: letters are the query, and the commands
    // are on Enter, Esc, the arrows and control chords. `?` stays the
    // reference: no action's description needs one to be found.
    ACTION_MENU_FILTER: [] as "any letter" => Action::ActionsFilter;
    ACTION_MENU_NEXT: [Key::code(KeyCode::Down), Key::ctrl('j'), Key::ctrl('n')]
        => Action::ActionsNext;
    ACTION_MENU_PREV: [Key::code(KeyCode::Up), Key::ctrl('k'), Key::ctrl('p')]
        => Action::ActionsPrev;
    ACTION_MENU_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => Action::ActionsPageDown;
    ACTION_MENU_PAGE_UP: [Key::code(KeyCode::PageUp)] => Action::ActionsPageUp;
    ACTION_MENU_DELETE_WORD: [] as "ctrl-w" => Action::ActionsDeleteWord;
    ACTION_MENU_RUN: [Key::code(KeyCode::Enter)] => Action::ActionsRun;
    ACTION_MENU_CANCEL: [Key::code(KeyCode::Esc)] => Action::ActionsCancel;
}

bindings! { WORKSPACE_JUMP = "Jump to a Workspace";
    // Typed at, as the action menu is.
    WORKSPACE_JUMP_FILTER: [] as "any letter" => Action::WorkspaceJumpFilter;
    WORKSPACE_JUMP_NEXT: [Key::code(KeyCode::Down), Key::ctrl('j')]
        => Action::WorkspaceJumpNext;
    WORKSPACE_JUMP_PREV: [Key::code(KeyCode::Up), Key::ctrl('k'), Key::ctrl('p')]
        => Action::WorkspaceJumpPrev;
    WORKSPACE_JUMP_DELETE_WORD: [] as "ctrl-w" => Action::WorkspaceJumpDeleteWord;
    WORKSPACE_JUMP_OPEN: [Key::code(KeyCode::Enter)] => Action::WorkspaceJumpOpen;
    WORKSPACE_JUMP_NEW: [Key::ctrl('n')] => Action::WorkspaceJumpNew;
    WORKSPACE_JUMP_CANCEL: [Key::code(KeyCode::Esc)] => Action::WorkspaceJumpCancel;
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
    Messages,
    Log,
    Quota,
    Processes,
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
    ActionMenu,
    WorkspaceJump,
}

impl Window {
    /// What the reference calls this window, shown in its frame so the
    /// operator can tell a window-specific reference from the one they saw on
    /// the screen before.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Raw => "raw",
            Self::Messages => "messages",
            Self::Log => "log",
            Self::Quota => "quota",
            Self::Processes => "processes",
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
            Self::ActionMenu => "actions",
            Self::WorkspaceJump => "jump to a Workspace",
        }
    }

    /// The sections this window's reference is made of, most specific first:
    /// what the window itself does, then what still holds around it.
    fn sections(self) -> &'static [&'static [ReferenceRow]] {
        match self {
            Self::Events => &[EVENTS, MESSAGE_EDITOR, GLOBAL],
            // Its one section is `styra-diff`'s table; see `reference`.
            Self::CheckoutDiff => &[],
            Self::Raw
            | Self::Messages
            | Self::Log
            | Self::Quota
            | Self::Processes
            | Self::Transcript => &[READING, MESSAGE_EDITOR, GLOBAL],
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
            Self::ActionMenu => &[ACTION_MENU],
            Self::WorkspaceJump => &[WORKSPACE_JUMP],
        }
    }

    /// Whether Space offers this window's actions as a menu: the session's
    /// own views and the navigator over them. The modal overlays and the
    /// standalone choosers are short lists already, and most of them are
    /// typed at.
    pub(crate) fn has_action_menu(self) -> bool {
        matches!(
            self,
            Self::Events
                | Self::Raw
                | Self::Messages
                | Self::Log
                | Self::Quota
                | Self::Processes
                | Self::Transcript
                | Self::Driva
                | Self::Files
                | Self::Answer
                | Self::Preview
                | Self::Overview
                | Self::Interactions
        )
    }

    /// Whether this window's own section takes `key` from the global one: a
    /// view's own command on a key wins over the global command on the same
    /// key. A binding narrowed by a note (a link highlighted, the preview
    /// open) only holds its key some of the time, so it does not take it, and
    /// nor does the second key of a chord.
    pub(crate) fn claims(self, key: crossterm::event::KeyEvent) -> bool {
        let Some(own) = self.sections().first() else {
            return false;
        };
        own.first() != GLOBAL.first()
            && own.iter().any(|row| {
                matches!(row, ReferenceRow::Binding(binding)
                    if binding.note().is_none()
                        && !is_chord(binding)
                        && binding.matches_in(key, self))
            })
    }

    /// Whether `binding`, listed in `section`, acts in this window: it is not
    /// narrowed to other windows, and its key is not one this window's own
    /// section claims.
    fn offers(self, section: &[ReferenceRow], binding: &Binding) -> bool {
        binding.applies_in(self)
            && !(section.first() == GLOBAL.first()
                && binding.press().is_some_and(|key| self.claims(key)))
    }

    /// What the action menu offers here: the reference's bindings that a
    /// keypress reaches from the list, most specific first, each command once.
    ///
    /// The message editor's keys are left out — the menu is opened from the
    /// list, where they mean something else — as is everything on Space, which
    /// is the menu's own key wherever it can be opened.
    pub(crate) fn actions(self) -> Vec<&'static Binding> {
        let mut bindings: Vec<&'static Binding> = Vec::new();
        for section in self.sections() {
            if section.first() == MESSAGE_EDITOR.first() {
                continue;
            }
            for row in *section {
                let ReferenceRow::Binding(binding) = row else {
                    continue;
                };
                if binding.press().is_none()
                    || !self.offers(section, binding)
                    || matches!(
                        binding.action(),
                        Action::GlobalActionMenu
                            | Action::GlobalLeaveMessage
                            | Action::EventsLinkMenu
                    )
                    || bindings
                        .iter()
                        .any(|seen| seen.action() == binding.action())
                {
                    continue;
                }
                bindings.push(binding);
            }
        }
        bindings
    }

    /// This window's reference, in display order, with a blank line between
    /// sections.
    pub(crate) fn reference(self) -> Vec<ReferenceRow> {
        // The diff owns the keyboard: nothing outside its own keys, and the
        // one Styra adds to them, acts.
        if self == Self::CheckoutDiff {
            return std::iter::once(ReferenceRow::Section("Checkout diff"))
                .chain(styra_diff::keys::BINDINGS.iter().map(ReferenceRow::Diff))
                .chain(CHECKOUT_DIFF.iter().skip(1).copied())
                .collect();
        }
        let mut rows = Vec::new();
        for section in self.sections() {
            if !rows.is_empty() {
                rows.push(ReferenceRow::Blank);
            }
            rows.extend(section.iter().copied().filter(|row| match row {
                ReferenceRow::Binding(binding) => self.offers(section, binding),
                _ => true,
            }));
        }
        rows
    }
}

/// Whether `binding` is the second key of a chord, reached only after
/// [`EVENTS_FOLD_PREFIX`].
fn is_chord(binding: &Binding) -> bool {
    [EVENTS_EXPAND_ALL, EVENTS_COLLAPSE_ALL].contains(binding)
}

/// The keypresses that run `binding`, in order: its first key, after the
/// prefix of the chord it completes.
pub(crate) fn presses(binding: &Binding) -> Vec<crossterm::event::KeyEvent> {
    let prefix = is_chord(binding)
        .then(|| EVENTS_FOLD_PREFIX.press())
        .flatten();
    prefix.into_iter().chain(binding.press()).collect()
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    const WINDOWS: [Window; 24] = [
        Window::Events,
        Window::Raw,
        Window::Messages,
        Window::Log,
        Window::Quota,
        Window::Processes,
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
        Window::ActionMenu,
        Window::WorkspaceJump,
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
                    .any(|row| matches!(row, ReferenceRow::Binding(_) | ReferenceRow::Diff(_))),
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
        assert_eq!(OVERVIEW_CLOSE.label(), "o/Esc");
        assert_eq!(EDITOR_NEWLINE.label(), "alt-Enter");
        assert_eq!(
            GLOBAL_SESSION_WORKTREE.label(),
            "ctrl-w (existing session)",
            "a narrower binding says where it applies"
        );
        assert_eq!(EVENTS_EXPAND_ALL.label(), "z R", "a chord names its prefix");
    }

    /// The menu offers what the window's reference lists, so the operator
    /// finds the same commands there — but only those a keypress from the list
    /// reaches, and each command once however many keys it has.
    #[test]
    fn the_action_menu_offers_the_windows_dispatchable_commands() {
        let actions: Vec<Action> = Window::Events
            .actions()
            .iter()
            .map(|binding| binding.action())
            .collect();
        assert_eq!(actions.first(), Some(&Action::EventsPreviewScrollDown));
        assert!(actions.contains(&Action::EventsToggleExpand));
        assert!(actions.contains(&Action::GlobalQuit));
        assert!(!actions.contains(&Action::GlobalActionMenu), "not itself");
        assert!(!actions.contains(&Action::EditorSend), "not the editor's");
        assert!(!actions.contains(&Action::GlobalLeaveMessage));
        assert_eq!(
            actions
                .iter()
                .filter(|action| **action == Action::GlobalFiles)
                .count(),
            1,
            "F and its alias are one command"
        );
        assert!(Window::Overview
            .actions()
            .iter()
            .any(|binding| binding.action() == Action::OverviewSearch));
        assert!(!Window::Launcher.has_action_menu());
        assert!(!Window::ActionMenu.has_action_menu());
    }

    /// A global command whose key the window's own section takes, or that is
    /// narrowed to other windows, is neither listed nor offered there: the
    /// menu would press a key that does something else.
    #[test]
    fn a_window_offers_only_the_commands_its_keys_reach() {
        let offers = |window: Window, action: Action| {
            let listed = window.reference().iter().any(
                |row| matches!(row, ReferenceRow::Binding(binding) if binding.action() == action),
            );
            let offered = window
                .actions()
                .iter()
                .any(|binding| binding.action() == action);
            assert_eq!(listed, offered, "{} lists what it offers", window.name());
            offered
        };
        // Taken by the view's own keys.
        assert!(!offers(Window::Driva, Action::GlobalDiffFullscreen));
        assert!(!offers(Window::Driva, Action::GlobalWorktrees));
        assert!(!offers(Window::Driva, Action::GlobalIntegrate));
        assert!(!offers(Window::Driva, Action::GlobalTags));
        assert!(!offers(Window::Files, Action::GlobalEntryLog));
        assert!(!offers(Window::Files, Action::GlobalInteractions));
        assert!(!offers(Window::Raw, Action::GlobalWorkspaceJump));
        assert!(!offers(Window::Overview, Action::GlobalLauncher));
        assert!(!offers(Window::Interactions, Action::GlobalInterrupt));
        // Narrowed to other windows.
        assert!(!offers(Window::Events, Action::GlobalBack));
        assert!(!offers(Window::Preview, Action::GlobalFocusMessage));
        assert!(!offers(Window::Log, Action::ReadingPageDown));
        assert!(!offers(Window::Log, Action::ReadingProviderRaw));
        assert!(!offers(Window::Quota, Action::ReadingFirst));
        // Still there where nothing takes the key.
        assert!(offers(Window::Raw, Action::GlobalFiles));
        assert!(offers(Window::Raw, Action::GlobalTags));
        assert!(offers(Window::Raw, Action::ReadingPageDown));
        assert!(offers(Window::Driva, Action::GlobalBack));
        assert!(offers(Window::Events, Action::GlobalWorktrees));
        // The answer view takes `F`, so files is offered on its other key.
        let files: Vec<String> = Window::Answer
            .actions()
            .iter()
            .filter(|binding| binding.action() == Action::GlobalFiles)
            .map(|binding| binding.keys_label())
            .collect();
        assert_eq!(files, ["f"]);
        // A conditional key and a chord's second key take nothing.
        assert!(
            offers(Window::Events, Action::GlobalMessages),
            "z M is a chord"
        );
        assert!(!Window::Events.claims(KeyEvent::from(KeyCode::Char('M'))));
    }

    /// A chord is run by pressing its prefix first, as the operator would.
    #[test]
    fn a_chord_is_run_with_its_prefix() {
        assert_eq!(
            presses(&EVENTS_EXPAND_ALL),
            [
                KeyEvent::from(KeyCode::Char('z')),
                KeyEvent::from(KeyCode::Char('R'))
            ]
        );
        assert_eq!(
            presses(&GLOBAL_LOG),
            [KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)]
        );
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
