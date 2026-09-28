//! Terminal keyboard bindings and their visible reference.
//!
//! Every binding is declared once, by the [`bindings!`] macro below: the keys
//! that trigger it, and the line the reference shows for it.  The dispatchers
//! in this file — and in the few other modules that read keys — ask a binding
//! whether it matches the keypress rather than spelling the key out again, so
//! rebinding a command is one edit and the reference cannot drift from it.
//!
//! The map is cut into sections, and each window names the sections that apply
//! to it: `?` answers for what is on screen rather than for the whole client.
//! Screens therefore no longer carry a strip of shortcuts along their top —
//! the strip could only ever list the few that fit, and it cost a line of the
//! list it sat above.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One key, as the operator presses it.
///
/// Shift is not compared: a shifted letter already arrives as its capital, and
/// terminals disagree about whether they also set the modifier for punctuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl Key {
    /// A key with no modifier: `Enter`, `Tab`, an arrow, a page key.
    pub(crate) const fn code(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A plain character.
    pub(crate) const fn ch(character: char) -> Self {
        Self::code(KeyCode::Char(character))
    }

    /// A character held with control. Terminals report the unshifted letter,
    /// so these are written lowercase.
    pub(crate) const fn ctrl(character: char) -> Self {
        Self {
            code: KeyCode::Char(character),
            modifiers: KeyModifiers::CONTROL,
        }
    }

    /// Control with a named key, such as ctrl-Enter.
    pub(crate) const fn ctrl_code(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::CONTROL,
        }
    }

    /// Alt with a named key, such as alt-Enter.
    pub(crate) const fn alt_code(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::ALT,
        }
    }

    fn matches(self, event: KeyEvent) -> bool {
        let compared = KeyModifiers::CONTROL | KeyModifiers::ALT;
        event.code == self.code && (event.modifiers & compared) == (self.modifiers & compared)
    }

    /// What the reference calls this key.
    fn label(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char(character) => character.to_string(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::BackTab => "Shift+Tab".to_owned(),
            KeyCode::Backspace => "Backspace".to_owned(),
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::Left => "←".to_owned(),
            KeyCode::Right => "→".to_owned(),
            KeyCode::PageUp => "PgUp".to_owned(),
            KeyCode::PageDown => "PgDn".to_owned(),
            other => format!("{other:?}"),
        };
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            format!("ctrl-{base}")
        } else if self.modifiers.contains(KeyModifiers::ALT) {
            format!("alt-{base}")
        } else {
            base
        }
    }
}

/// One command: the keys that reach it, and what the reference says about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    keys: &'static [Key],
    /// Used instead of the keys' own names where the command is not a single
    /// press — a chord, or something typed into the message box.
    label: Option<&'static str>,
    /// When the binding only applies in a narrower situation than the section
    /// it sits in, shown in parentheses after the keys.
    note: Option<&'static str>,
    action: &'static str,
}

impl Binding {
    /// Whether this keypress is this command. A binding with no keys of its
    /// own — one the reference describes but the event loop does not dispatch
    /// — never matches.
    pub(crate) fn matches(&self, event: KeyEvent) -> bool {
        self.keys.iter().any(|key| key.matches(event))
    }

    /// The keys column of the reference.
    pub(crate) fn label(&self) -> String {
        let keys = match self.label {
            Some(label) => label.to_owned(),
            None => self
                .keys
                .iter()
                .map(|key| key.label())
                .collect::<Vec<_>>()
                .join("/"),
        };
        match self.note {
            Some(note) => format!("{keys} ({note})"),
            None => keys,
        }
    }

    pub(crate) fn action(&self) -> &'static str {
        self.action
    }
}

/// Declare a section of the reference, and with it the bindings it is made of.
///
/// ```ignore
/// bindings! { SECTION = "Heading";
///     NAME: [Key::ch('j'), Key::code(KeyCode::Down)] => "what it does";
///     CHORD: [Key::ch('R')] as "z R" => "what it does";
///     NARROW: [Key::ch('v')] ("preview open") => "what it does";
/// }
/// ```
macro_rules! bindings {
    (@label) => { None };
    (@label $label:literal) => { Some($label) };
    (@note) => { None };
    (@note $note:literal) => { Some($note) };
    (
        $rows:ident = $title:literal;
        $(
            $(#[$meta:meta])*
            $name:ident: [$($key:expr),* $(,)?] $(as $label:literal)? $(($note:literal))? => $action:literal;
        )*
    ) => {
        $(
            $(#[$meta])*
            pub(crate) const $name: Binding = Binding {
                keys: &[$($key),*],
                label: bindings!(@label $($label)?),
                note: bindings!(@note $($note)?),
                action: $action,
            };
        )*
        /// Not every group is a window's reference: some only exist to give
        /// their bindings a home.
        #[allow(dead_code)]
        const $rows: &[ReferenceRow] = &[
            ReferenceRow::Section($title),
            $(ReferenceRow::Binding(&$name),)*
        ];
    };
}

/// One row in the keyboard reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceRow {
    Section(&'static str),
    Binding(&'static Binding),
    Blank,
}

bindings! { GLOBAL = "Global";
    HELP: [Key::ch('?')] => "show/close the reference for this window";
    GLOBAL_FOCUS_MESSAGE: [Key::ch('i')] => "focus the message box";
    GLOBAL_LEAVE_MESSAGE: [Key::code(KeyCode::Esc)] => "return to the list";
    GLOBAL_QUIT: [Key::ch('q')] => "quit";
    GLOBAL_INTERRUPT: [Key::ch('s')] => "interrupt the active turn";
    GLOBAL_STOP: [Key::ch('S')] => "stop the interaction";
    GLOBAL_BRANCH: [Key::ch('B')] => "branch from history through, or only, the selected entry";
    GLOBAL_NEXT_LIVE: [Key::ch('n')] => "step to the next interaction that is still running";
    GLOBAL_NEW_SESSION: [Key::ch('N')] => "new session where this one works";
    GLOBAL_LAUNCHER: [Key::ch('l')] => "choose model for an idle agent turn";
    GLOBAL_SHELL: [Key::ch('!')] => "open session shell in a new terminal";
    GLOBAL_DIRECTORY: [Key::ch('~')] => "open a terminal in the interaction's working directory";
    GLOBAL_INTERACTIONS: [Key::ch('a')] => "live interactions";
    GLOBAL_SESSIONS: [Key::ch('A')] => "stored sessions";
    GLOBAL_WORKSPACES: [Key::ch('V')] => "Workspaces";
    GLOBAL_SESSION_WORKTREE: [Key::ch('W')] ("existing session")
        => "create and associate a Git branch and workspace";
    GLOBAL_TAGS: [Key::ch('T')] => "edit the current interaction's tags";
    GLOBAL_NEXT_IDLE: [Key::ctrl('a')] => "go to the next interaction that went idle unseen";
    GLOBAL_RAW: [Key::ch('r')] => "raw view; press again for events";
    GLOBAL_LOG: [Key::ctrl('l')] => "log view; press again for events";
    GLOBAL_TRANSCRIPT: [Key::ch('t')] => "transcript view; press again for events";
    GLOBAL_DETAILS: [Key::ch('d')] => "details view; press again for events";
    GLOBAL_ENTRY_LOG: [Key::ch('e')] => "toggle the entry log below the event list";
    GLOBAL_ENTRY_LOG_FOCUS: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)]
        => "move between the event list and the open entry log";
    GLOBAL_QUOTA: [Key::ch('Q')] => "plan quota readings, refreshed from the server";
    GLOBAL_FILES: [Key::ch('F')] => "files mentioned by the focused entry";
    GLOBAL_FILES_ALIAS: [Key::ch('f')] ("other views")
        => "files mentioned by the focused entry";
    GLOBAL_ANSWER: [Key::ch('X')] => "the last turn's typed answer";
    GLOBAL_PREVIEW: [Key::ch('P')] => "toggle full-screen preview";
    GLOBAL_COPY_CONVERSATION: [Key::ch('Y')] => "copy the whole conversation (any view)";
}

bindings! { EVENTS = "Events and previews";
    EVENTS_NEXT_ENTRY: [Key::ch('J'), Key::code(KeyCode::Down)] => "next entry";
    EVENTS_PREV_ENTRY: [Key::ch('K'), Key::code(KeyCode::Up)] => "previous entry";
    EVENTS_NEXT_LINE: [Key::ch('j')] => "next line, or next link while links are highlighted";
    EVENTS_PREV_LINE: [Key::ch('k')] => "previous line, or previous link";
    EVENTS_FIRST: [Key::ch('g')] => "first entry";
    EVENTS_LAST: [Key::ch('G')] => "last entry";
    EVENTS_FOLLOW_BRANCH: [Key::ch('b')] ("or Enter on a branch marker")
        => "follow the linked Session";
    EVENTS_TOGGLE_EXPAND: [Key::ch(' '), Key::code(KeyCode::Enter), Key::ch('o')]
        => "toggle selected entry";
    EVENTS_EXPAND_ONLY: [Key::ch('O')] => "expand only selected";
    EVENTS_EXPAND_ALL: [Key::ch('R')] as "z R" => "expand all";
    EVENTS_COLLAPSE_ALL: [Key::ch('M')] as "z M" => "collapse all";
    EVENTS_MINOR: [Key::ch('m')] => "toggle minor events";
    EVENTS_PREVIEW_PANEL: [Key::ch('p')] => "toggle preview panel";
    EVENTS_CONVERSATION_ONLY: [Key::ch('c')] => "toggle conversation-only events";
    EVENTS_PREVIEW_MODE: [Key::ch('v')] ("preview open") => "pretty/diff preview";
    EVENTS_PREVIEW_TARGET: [Key::ch('C')] ("preview open") => "preview the newest command";
    EVENTS_COMPLETE: [Key::ch('C')] ("preview closed")
        => "mark this interaction completed and stop it";
    EVENTS_LINK_DESTINATIONS: [Key::ch('u')] => "toggle link destinations";
    EVENTS_SEARCH: [Key::ch('/')]
        => "search: mark words matching a term of 3+ characters (Esc clears)";
    EVENTS_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => "scroll the preview or the entry log down";
    EVENTS_PAGE_UP: [Key::code(KeyCode::PageUp)] => "scroll the preview or the entry log up";
    EVENTS_LINKS: [Key::ch('f')]
        => "highlight conversation links (j/k moves, Enter opens, Esc exits)";
    EVENTS_COPY: [Key::ch('y')] => "copy selected entry to clipboard";
}

bindings! { READING = "Raw, log, quota, and transcript";
    READING_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => "move or scroll down";
    READING_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => "move or scroll up";
    READING_FIRST: [Key::ch('g')] => "first line, or top";
    READING_LAST: [Key::ch('G')] => "last line, or bottom";
    READING_LINKS: [Key::ch('f')] ("transcript")
        => "highlight conversation links (j/k moves, Enter opens, Esc exits)";
    READING_CONVERSATION_ONLY: [Key::ch('c')] ("transcript")
        => "toggle conversation-only events";
    READING_RETRY: [Key::ch('R')] ("quota")
        => "after a rate limit, ask this session again once the window resets";
    READING_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => "scroll the raw-line preview down";
    READING_PAGE_UP: [Key::code(KeyCode::PageUp)] => "scroll the raw-line preview up";
    READING_PROVIDER_RAW: [Key::ch('v')] ("raw")
        => "switch Styra wire capture / provider-native session JSONL";
    READING_COPY: [Key::ch('y')] ("raw") => "copy selected line to clipboard";
}

bindings! { PREVIEW = "Full-screen preview";
    PREVIEW_SCROLL_DOWN: [Key::ch('j')] => "scroll 10 lines down, or step to the next link";
    PREVIEW_SCROLL_UP: [Key::ch('k')] => "scroll 10 lines up, or step to the previous link";
    PREVIEW_NEXT_ENTRY: [Key::ch('J'), Key::code(KeyCode::Down)] => "next entry";
    PREVIEW_PREV_ENTRY: [Key::ch('K'), Key::code(KeyCode::Up)] => "previous entry";
    PREVIEW_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => "page down";
    PREVIEW_PAGE_UP: [Key::code(KeyCode::PageUp)] => "page up";
    PREVIEW_FIRST: [Key::ch('g')] => "first entry";
    PREVIEW_LAST: [Key::ch('G')] => "last entry";
    PREVIEW_MODE: [Key::ch('v')] => "pretty/diff preview";
    PREVIEW_TARGET: [Key::ch('C')] => "preview the newest command";
    PREVIEW_LINKS: [Key::ch('f')] => "highlight conversation links";
    PREVIEW_LINK_DESTINATIONS: [Key::ch('u')] => "toggle link destinations";
    PREVIEW_COPY: [Key::ch('y')] => "copy the previewed entry to clipboard";
}

bindings! { DRIVA = "Details (Workspace, interaction, and launch policy)";
    DRIVA_SCOPE: [Key::code(KeyCode::Tab), Key::code(KeyCode::BackTab)]
        => "edit the Workspace's policy / this interaction's";
    DRIVA_NEXT_MOUNT: [Key::ch('j'), Key::code(KeyCode::Down)] => "next mount";
    DRIVA_PREV_MOUNT: [Key::ch('k'), Key::code(KeyCode::Up)] => "previous mount";
    DRIVA_NETWORK: [Key::ch('w')] => "permit/forbid agent networking, in the focused layer";
    DRIVA_ACCESS: [Key::ch('R')]
        => "mount the workspace read-write/read-only, in the focused layer";
    DRIVA_TEMPLATES: [Key::ch('T')] => "choose Driva templates";
    DRIVA_ADD_MOUNT: [Key::ch('m')] => "add a mount";
    DRIVA_GIT_CHECKOUT: [Key::ch('G')] => "set this Workspace's Git checkout";
    DRIVA_REMOVE_MOUNT: [Key::ch('x')] => "remove selected mount";
    DRIVA_IGNORE_WORKSPACE: [Key::ch('I')]
        => "this interaction adds to / ignores the Workspace policy";
    DRIVA_PROMOTE: [Key::ch('U')] => "move this interaction's policy up into the Workspace's";
    DRIVA_SAVE_DEFAULT: [Key::ch('D')] => "save this interaction's policy for new clients";
    DRIVA_PAGE_DOWN: [Key::code(KeyCode::PageDown)]
        => "scroll the sandbox account down: mounts, floor, environment, private root";
    DRIVA_PAGE_UP: [Key::code(KeyCode::PageUp)] => "scroll the sandbox account up";
}

bindings! { FILES = "Files";
    FILES_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => "next file";
    FILES_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "previous file";
    FILES_NEXT_ENTRY: [Key::ch('J')] => "next interaction-log entry";
    FILES_PREV_ENTRY: [Key::ch('K')] => "previous interaction-log entry";
    FILES_FIRST: [Key::ch('g')] => "first file";
    FILES_LAST: [Key::ch('G')] => "last file";
    FILES_EDIT: [Key::ch('e')] => "open selected file in editor";
    FILES_PREVIEW: [Key::ch('p')] => "toggle interaction preview";
    FILES_SCOPE: [Key::ch('a')] => "focused-entry / all-session files";
    FILES_COPY: [Key::ch('y')] => "copy the path";
}

bindings! { ANSWER = "Typed answer";
    ANSWER_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => "next item";
    ANSWER_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "previous item";
    ANSWER_FIRST: [Key::ch('g')] => "first item";
    ANSWER_LAST: [Key::ch('G')] => "last item";
    ANSWER_EDIT: [Key::ch('e')] => "open location in editor";
    ANSWER_COPY: [Key::ch('y')] => "copy";
    ANSWER_AS_TEXT: [Key::ch('T')] => "re-read as text";
    ANSWER_AS_LINES: [Key::ch('L')] => "re-read as lines";
    ANSWER_AS_FILES: [Key::ch('F')] => "re-read as files";
    ANSWER_AS_JSON: [Key::ch('J')] => "re-read as json";
    ANSWER_REREAD: [Key::ch('R')] => "re-read as the turn asked";
}

bindings! { MESSAGE_EDITOR = "Message editor";
    EDITOR_RECORD: [Key::ctrl('r')]
        => "record from the microphone; the box becomes a level meter";
    EDITOR_RECORD_FINISH: [Key::code(KeyCode::Enter)] ("while recording")
        => "transcribe into the message";
    EDITOR_RECORD_CANCEL: [Key::code(KeyCode::Esc)] ("while recording") => "discard the recording";
    EDITOR_RECORD_LOUDER: [Key::code(KeyCode::Up), Key::ch('+'), Key::ch('=')] ("while recording")
        => "boost a quiet input";
    EDITOR_RECORD_QUIETER: [Key::code(KeyCode::Down), Key::ch('-')] ("while recording")
        => "take a loud input down";
    EDITOR_CONTRACT: [Key::ctrl('t')]
        => "ask this message's reply for a shape (text/lines/files/json)";
    EDITOR_SEND: [Key::code(KeyCode::Enter)] => "send message";
    EDITOR_SEND_IN_BRANCH: [Key::ctrl_code(KeyCode::Enter)] ("first prompt")
        => "send in a new Git branch and workspace";
    EDITOR_CHANGE_DIRECTORY: [] as "/cd <directory>"
        => "change the live Codex interaction directory";
    EDITOR_NEWLINE: [Key::alt_code(KeyCode::Enter)] => "insert newline";
    EDITOR_HISTORY_OLDER: [Key::code(KeyCode::Up)] => "older message history";
    EDITOR_HISTORY_NEWER: [Key::code(KeyCode::Down)] => "newer message history";
    EDITOR_DELETE_WORD: [Key::ctrl('w')] => "delete previous word";
    EDITOR_LAUNCHER: [Key::ctrl('l')]
        => "choose model before first message or idle agent turn";
    EDITOR_INSERT_PATH: [Key::ctrl('f')] => "insert a file path (Tab completes, Enter inserts)";
    EDITOR_MOUNT_READABLE: [Key::ch('r')] ("unmounted path") => "mount it readable";
    EDITOR_MOUNT_WRITABLE: [Key::ch('w')] ("unmounted path") => "mount it writable";
    EDITOR_MOUNT_NEITHER: [Key::ch('n')] ("unmounted path") => "insert it with no mount";
}

bindings! { INTERACTIONS = "Interactions";
    INTERACTIONS_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)]
        => "move the cursor down; the rested-on interaction becomes current";
    INTERACTIONS_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "move the cursor up";
    INTERACTIONS_NEXT_WORKSPACE: [Key::ch('J')]
        => "first interaction of the next Workspace, in All";
    INTERACTIONS_PREV_WORKSPACE: [Key::ch('K')]
        => "first interaction of the previous Workspace, in All";
    INTERACTIONS_NEXT_LIVE: [Key::ctrl('n')] => "next interaction that is still running";
    INTERACTIONS_NEXT_WORKING: [Key::ch('N')]
        => "next interaction actively working, skipping idle ones";
    INTERACTIONS_NEXT_IDLE: [Key::ctrl('a')] => "next interaction that went idle unseen";
    INTERACTIONS_SCOPE: [Key::ch('w')] => "current Workspace / all Workspaces";
    INTERACTIONS_COMPLETED: [Key::ch('c')] => "show/hide completed";
    INTERACTIONS_COMPLETE: [Key::ch('C')] => "mark selected completed and stop it";
    INTERACTIONS_TAGS: [Key::ch('T')] => "edit the selected interaction's tags";
    INTERACTIONS_STOP: [Key::ch('S')] => "stop the selected interaction";
    INTERACTIONS_DELETE: [Key::ch('D')] => "delete it once stopped";
    INTERACTIONS_CLOSE: [Key::code(KeyCode::Enter), Key::ch('a'), Key::code(KeyCode::Esc)]
        => "close the list";
}

bindings! { BRANCH = "Branch from selected entry";
    BRANCH_NEXT: [Key::ch('j'), Key::ch('J'), Key::code(KeyCode::Down)]
        => "entire interaction through this entry";
    BRANCH_PREV: [Key::ch('k'), Key::ch('K'), Key::code(KeyCode::Up)] => "only this entry";
    BRANCH_CONFIRM: [Key::code(KeyCode::Enter)] => "branch, and open the result";
    BRANCH_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => "cancel";
}

bindings! { TAGS = "Interaction tags";
    TAGS_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => "move selection down";
    TAGS_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "move selection up";
    TAGS_TOGGLE: [Key::ch(' ')] => "toggle the selected tag";
    TAGS_NEW: [Key::ch('n')] => "add a new tag (Enter adds and saves)";
    TAGS_SAVE: [Key::code(KeyCode::Enter)] => "save";
    TAGS_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => "cancel";
}

bindings! { SESSION_PICKER = "Stored sessions";
    SESSIONS_HELP: [Key::ch('?')] => "show/close this reference";
    SESSIONS_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => "move selection down";
    SESSIONS_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "move selection up";
    SESSIONS_NEXT_ROOT: [Key::ch('J')] => "next root conversation, skipping its branches";
    SESSIONS_PREV_ROOT: [Key::ch('K')] => "previous root conversation";
    SESSIONS_FIRST: [Key::ch('g')] => "first session";
    SESSIONS_LAST: [Key::ch('G')] => "last session";
    SESSIONS_OPEN: [Key::code(KeyCode::Enter)] => "open the selected session";
    SESSIONS_NEW: [Key::ch('n')] => "start a new session in this Workspace";
    SESSIONS_COMPLETED: [Key::ch('c')] => "show/hide completed";
    SESSIONS_COMPLETE: [Key::ch('C')] => "mark selected completed or not";
    SESSIONS_FILTER: [Key::ch('/')] => "filter by name or first prompt (Esc clears)";
    SESSIONS_SORT: [Key::ch('s')] => "sort by last activity / by creation";
    SESSIONS_OLDER: [Key::ch('a')] => "show/hide history older than a week";
    SESSIONS_RENAME: [Key::ch('r')] => "rename the selected session";
    SESSIONS_CONVERT: [Key::ch('x')] => "convert the selected session to the other provider";
    SESSIONS_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => "cancel";
}

bindings! { WORKSPACE_PICKER = "Workspaces";
    WORKSPACES_HELP: [Key::ch('?')] => "show/close this reference";
    WORKSPACES_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => "move selection down";
    WORKSPACES_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "move selection up";
    WORKSPACES_OPEN: [Key::code(KeyCode::Enter)] => "open the selected Workspace";
    WORKSPACES_CREATE: [Key::ch('n')] => "create a Workspace for the current directory";
    WORKSPACES_RENAME: [Key::ch('r')] => "rename the selected Workspace";
    WORKSPACES_FILTER: [Key::ch('/')] => "filter by name (Esc clears)";
    WORKSPACES_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => "cancel";
}

bindings! { LAUNCHER = "Launch";
    LAUNCHER_HELP: [Key::ch('?')] => "show/close this reference";
    LAUNCHER_NEXT: [Key::ch('j'), Key::ch('J'), Key::code(KeyCode::Down)] => "move selection down";
    LAUNCHER_PREV: [Key::ch('k'), Key::ch('K'), Key::code(KeyCode::Up)] => "move selection up";
    LAUNCHER_NEXT_COLUMN: [Key::ch('l'), Key::code(KeyCode::Right), Key::code(KeyCode::Tab)]
        => "next launch column";
    LAUNCHER_PREV_COLUMN: [Key::ch('h'), Key::code(KeyCode::Left), Key::code(KeyCode::BackTab)]
        => "previous launch column";
    LAUNCHER_PROVIDER_DOWN: [Key::ch('p')] => "move down the provider column";
    LAUNCHER_PROVIDER_UP: [Key::ch('P')] => "move up the provider column";
    LAUNCHER_MODEL_DOWN: [Key::ch('m')] => "move down the model column";
    LAUNCHER_MODEL_UP: [Key::ch('M')] => "move up the model column";
    LAUNCHER_EFFORT_DOWN: [Key::ch('e')] => "move down the effort column";
    LAUNCHER_EFFORT_UP: [Key::ch('E')] => "move up the effort column";
    LAUNCHER_SELECT: [Key::code(KeyCode::Enter)] => "select";
    LAUNCHER_DEFAULT: [Key::ch('D')] => "select and save launch default";
    LAUNCHER_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => "cancel";
}

bindings! { TEMPLATE_PICKER = "Driva templates";
    TEMPLATES_HELP: [Key::ch('?')] => "show/close this reference";
    TEMPLATES_NEXT: [Key::ch('j'), Key::code(KeyCode::Down)] => "move selection down";
    TEMPLATES_PREV: [Key::ch('k'), Key::code(KeyCode::Up)] => "move selection up";
    TEMPLATES_TOGGLE: [Key::ch(' ')] => "add/remove the selected template";
    TEMPLATES_APPLY: [Key::code(KeyCode::Enter)] => "apply; templates layer in the order chosen";
    TEMPLATES_CANCEL: [Key::code(KeyCode::Esc), Key::ch('q')] => "cancel";
}

// Bindings that belong to no window's reference: the prefix of a chord, and
// the keys of the reference overlay itself, which says how to leave in its own
// footer rather than in a list of rows.
bindings! { REFERENCE_OVERLAY = "Reference";
    EVENTS_FOLD_PREFIX: [Key::ch('z')] => "fold prefix: z R expands all, z M collapses all";
    CLOSE_REFERENCE: [Key::ch('?'), Key::code(KeyCode::Esc), Key::ch('q')]
        => "close the reference";
    REFERENCE_DOWN: [Key::ch('j'), Key::code(KeyCode::Down)] => "scroll down";
    REFERENCE_UP: [Key::ch('k'), Key::code(KeyCode::Up)] => "scroll up";
    REFERENCE_PAGE_DOWN: [Key::code(KeyCode::PageDown)] => "page down";
    REFERENCE_PAGE_UP: [Key::code(KeyCode::PageUp)] => "page up";
    REFERENCE_TOP: [Key::ch('g')] => "back to the top";
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
                    ReferenceRow::Binding(binding) if binding.action == "cancel"
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
        assert_eq!(GLOBAL_NEXT_IDLE.label(), "ctrl-a");
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

use std::path::Path;

use crate::app::{App, Request, View};
use crate::insert;
use crate::launch;
use crate::preferences;
use crate::session::{self, Attachment};
use styra_protocol::{Contract, LogEntry};
use styra_server::Client;

/// Keys for the event list's `/` search prompt. It is modal — every printable
/// key is part of the term, including the letters bound to commands on the
/// list underneath — so the event loop routes keys here ahead of the view.
///
/// Enter hands the keys back with the term still marked; Esc leaves the list
/// unmarked, as does backspacing the term away.
pub fn handle_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.search.cancel(),
        KeyCode::Enter => app.search.accept(),
        KeyCode::Backspace => app.search.backspace(),
        KeyCode::Char(character) if !character.is_control() => app.search.push(character),
        _ => {}
    }
}

/// Keys for the driva view's "add a mount" prompt. It is modal — every
/// printable key is part of the path being typed, `?` included — so the event
/// loop routes keys here ahead of the keybind reference and every view and
/// global binding.
pub fn handle_mount_prompt_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => launch::cancel_prompt(app),
        KeyCode::Enter => launch::confirm_prompt(app),
        KeyCode::Backspace => {
            if let Some(text) = app.launch.prompt.as_mut() {
                text.pop();
            }
        }
        KeyCode::Char(ch) if !ch.is_control() => {
            if let Some(text) = app.launch.prompt.as_mut() {
                text.push(ch);
            }
        }
        _ => {}
    }
}

/// Keys for the Workspace Git-checkout prompt. This is durable Workspace
/// metadata, rather than an individual sandbox grant.
pub fn handle_git_repository_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.git_repository_prompt = None,
        KeyCode::Enter => {
            let Some(path) = app.git_repository_prompt.take() else {
                return;
            };
            let Some(workspace_id) = app.workspace.id.as_deref() else {
                return app.show_action_message("no Workspace is selected");
            };
            let repository = (!path.trim().is_empty()).then(|| Path::new(path.trim()));
            match client.set_workspace_git_repository(workspace_id, repository) {
                Ok(workspace) => {
                    app.show_workspace(&workspace);
                    app.show_action_message(if repository.is_some() {
                        "Git checkout associated for future launches"
                    } else {
                        "Git checkout association cleared"
                    });
                }
                Err(error) => {
                    app.show_action_message(format!("could not set Git checkout: {error:#}"))
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(text) = app.git_repository_prompt.as_mut() {
                text.pop();
            }
        }
        KeyCode::Char(ch) if !ch.is_control() => {
            if let Some(text) = app.git_repository_prompt.as_mut() {
                text.push(ch);
            }
        }
        _ => {}
    }
}

/// Keys for the two-way branch choice. Confirming closes the modal before the
/// server call, so success can replace the screen and failure returns to it.
pub fn handle_branch_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(prompt) = app.branch_prompt.as_mut() else {
        return;
    };
    match key {
        k if BRANCH_NEXT.matches(k) => prompt.select_next(),
        k if BRANCH_PREV.matches(k) => prompt.select_previous(),
        k if BRANCH_CONFIRM.matches(k) => {
            let at_ms = prompt.at_ms();
            let history = prompt.selected();
            app.branch_prompt = None;
            session::branch_session(app, client, at_ms, history);
        }
        k if BRANCH_CANCEL.matches(k) => app.branch_prompt = None,
        _ => {}
    }
}

pub fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    key: KeyEvent,
    pending_fold: &mut bool,
    preferences_path: &Path,
) {
    if std::mem::take(pending_fold) {
        match key {
            k if EVENTS_EXPAND_ALL.matches(k) => app.timeline.expand_all(),
            k if EVENTS_COLLAPSE_ALL.matches(k) => app.timeline.collapse_all(),
            _ => {}
        }
        return;
    }
    match key {
        k if GLOBAL_QUIT.matches(k) => return app.ask(Request::Quit),
        k if GLOBAL_INTERRUPT.matches(k) => {
            return session::interrupt_interaction(app, client, live)
        }
        k if GLOBAL_STOP.matches(k) => return session::pause_interaction(app, client, live),
        k if GLOBAL_BRANCH.matches(k) => return session::open_branch_prompt(app),
        k if GLOBAL_SHELL.matches(k) => {
            let Attachment::Attached { .. } = live else {
                return app.show_action_message("no live interaction to open a shell for");
            };
            return app.ask(Request::OpenShell);
        }
        // Beside `!`, and deliberately not the same shell: `!` attaches to the
        // agent's sandbox, `~` opens the operator's own shell on the host,
        // standing where the interaction is working. That works with no live
        // interaction — a finished one still has a directory to look at.
        k if GLOBAL_DIRECTORY.matches(k) => return app.ask(Request::OpenDirectory),
        k if GLOBAL_FOCUS_MESSAGE.matches(k) && app.view != View::Preview => {
            return app.enter_input()
        }
        // Global, unlike `y`: what it copies is the session's exchange, which
        // does not change with the view the operator happens to be in.
        k if GLOBAL_COPY_CONVERSATION.matches(k) => return copy_conversation(app),
        k if GLOBAL_RAW.matches(k) => return app.toggle_raw(),
        k if GLOBAL_LOG.matches(k) => return app.toggle_view(View::Log),
        k if GLOBAL_LAUNCHER.matches(k) => return app.open_launcher(),
        // Opening the view also refreshes it: the log lives in the daemon's
        // memory, so there is nothing local to show without asking.
        k if GLOBAL_QUOTA.matches(k) => {
            app.toggle_view(View::Quota);
            return app.ask(Request::Quota);
        }
        k if GLOBAL_TRANSCRIPT.matches(k) => return app.toggle_view(View::Transcript),
        // `e` toggles the pane below the event list. The files and answer
        // views keep the key for opening the editor, which is the one thing
        // `e` already meant there.
        k if GLOBAL_ENTRY_LOG.matches(k) && !matches!(app.view, View::Files | View::Answer) => {
            return app.toggle_entry_log()
        }
        k if GLOBAL_DETAILS.matches(k) => return app.toggle_view(View::Driva),
        k if GLOBAL_FILES.matches(k) && app.view != View::Answer => return app.toggle_files(),
        k if GLOBAL_FILES_ALIAS.matches(k)
            && !matches!(app.view, View::Events | View::Transcript | View::Preview) =>
        {
            return app.toggle_files()
        }
        k if GLOBAL_ANSWER.matches(k) => return app.toggle_answer(),
        k if GLOBAL_PREVIEW.matches(k) => return app.toggle_view(View::Preview),
        // Beside `a` because it is the same list: `a` opens it to be walked,
        // ctrl-a skips the walk and goes to what the footer is counting.
        k if GLOBAL_NEXT_IDLE.matches(k) => return app.ask(Request::NextIdleInteraction),
        k if GLOBAL_NEW_SESSION.matches(k) => return app.ask(Request::NewSession),
        k if GLOBAL_INTERACTIONS.matches(k) && app.view != View::Files => {
            return app.ask(Request::Interactions)
        }
        k if GLOBAL_WORKSPACES.matches(k) => return app.ask(Request::Workspace),
        k if GLOBAL_SESSION_WORKTREE.matches(k) && !app.session_id.is_empty() => {
            return app.ask(Request::CreateSessionWorktree)
        }
        k if GLOBAL_SESSIONS.matches(k) => return app.ask(Request::Sessions),
        // Beside ctrl-a, and the same jump with a wider net: ctrl-a goes to
        // work that is waiting to be read, plain `n` steps through every
        // interaction still running.
        k if GLOBAL_NEXT_LIVE.matches(k) => return app.ask(Request::NextLiveInteraction),
        _ => {}
    }
    match app.view {
        View::Events => match key {
            k if GLOBAL_TAGS.matches(k) => edit_current_interaction_tags(app, client),
            k if EVENTS_LINKS.matches(k) => app.highlight_first_link(),
            k if EVENTS_SEARCH.matches(k) => app.search.open(),
            // A search that stands after the prompt has closed is cleared
            // where it is being read, rather than by reopening the prompt in
            // order to cancel it.
            k if k.code == KeyCode::Esc && app.link_highlight.is_some() => {
                app.clear_link_highlight()
            }
            k if k.code == KeyCode::Esc && app.search.query().is_some() => app.search.cancel(),
            k if EVENTS_LINK_DESTINATIONS.matches(k) => app.toggle_link_display(),
            k if EVENTS_FOLLOW_BRANCH.matches(k) => session::follow_branch(app),
            k if EVENTS_CONVERSATION_ONLY.matches(k) => app.toggle_conversation_only(),
            k if EVENTS_PREVIEW_MODE.matches(k) && app.preview.open => app.preview.toggle_mode(),
            k if EVENTS_PREVIEW_TARGET.matches(k) && app.preview.open => {
                app.preview.toggle_target()
            }
            // Same key as the live-interactions navigator's `C`, and the same
            // action: finish the interaction on screen without first having to
            // open the navigator to find the row for it. Guarded so it does
            // not steal the preview pane's own `C`, which claims the key while
            // that pane is open.
            k if EVENTS_COMPLETE.matches(k) && !app.preview.open => {
                session::complete_interaction(app, client, live);
            }
            // The Events screen shows two windows when the entry-log pane is
            // open, and Tab is what moves the navigation keys between them.
            k if GLOBAL_ENTRY_LOG_FOCUS.matches(k) && app.entry_log.open => {
                app.toggle_entry_log_focus()
            }
            // With the pane holding the keys, the movement keys walk its
            // entries and the preview follows them. The list stands still, so
            // the operator keeps their place in it.
            k if (EVENTS_NEXT_ENTRY.matches(k) || EVENTS_NEXT_LINE.matches(k))
                && app.entry_log.focused() =>
            {
                app.entry_log_select_next()
            }
            k if (EVENTS_PREV_ENTRY.matches(k) || EVENTS_PREV_LINE.matches(k))
                && app.entry_log.focused() =>
            {
                app.entry_log_select_prev()
            }
            k if EVENTS_FIRST.matches(k) && app.entry_log.focused() => app.entry_log_select_first(),
            k if EVENTS_LAST.matches(k) && app.entry_log.focused() => app.entry_log_select_last(),
            // A focused pane scrolls by moving its cursor: the pane always
            // shows where the cursor is, so paging the offset on its own would
            // be undone by the next draw.
            k if EVENTS_PAGE_DOWN.matches(k) && app.entry_log.focused() => {
                app.entry_log_page_down()
            }
            k if EVENTS_PAGE_UP.matches(k) && app.entry_log.focused() => app.entry_log_page_up(),
            k if EVENTS_PAGE_DOWN.matches(k) && app.preview.open => app.preview.scroll.page_down(),
            k if EVENTS_PAGE_UP.matches(k) && app.preview.open => app.preview.scroll.page_up(),
            k if EVENTS_PAGE_DOWN.matches(k) && app.entry_log.open => {
                app.entry_log.scroll.page_down()
            }
            k if EVENTS_PAGE_UP.matches(k) && app.entry_log.open => app.entry_log.scroll.page_up(),
            k if EVENTS_NEXT_ENTRY.matches(k) => app.select_next(),
            k if EVENTS_PREV_ENTRY.matches(k) => app.select_prev(),
            k if EVENTS_NEXT_LINE.matches(k) && app.link_highlight.is_some() => {
                app.highlight_next_link()
            }
            k if EVENTS_PREV_LINE.matches(k) && app.link_highlight.is_some() => {
                app.highlight_prev_link()
            }
            k if EVENTS_NEXT_LINE.matches(k) => app.select_next_line(),
            k if EVENTS_PREV_LINE.matches(k) => app.select_prev_line(),
            k if k.code == KeyCode::Enter && app.link_highlight.is_some() => {
                app.open_highlighted_link()
            }
            // Branch markers are reciprocal links between the source and its
            // child Session. Enter follows either direction; all other
            // entries retain Enter's usual fold/unfold behavior.
            k if k.code == KeyCode::Enter
                && app
                    .timeline
                    .selected_entry()
                    .is_some_and(|entry| entry.event.branch_target().is_some()) =>
            {
                session::follow_branch(app)
            }
            k if EVENTS_TOGGLE_EXPAND.matches(k) => app.timeline.toggle_expand(),
            k if EVENTS_EXPAND_ONLY.matches(k) => app.timeline.expand_only_selected(),
            k if EVENTS_FIRST.matches(k) => app.select_first(),
            k if EVENTS_LAST.matches(k) => app.select_last(),
            k if EVENTS_FOLD_PREFIX.matches(k) => *pending_fold = true,
            k if EVENTS_MINOR.matches(k) => app.toggle_minor(),
            k if EVENTS_PREVIEW_PANEL.matches(k) => app.preview.toggle(),
            k if EVENTS_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Raw => match key {
            k if READING_PROVIDER_RAW.matches(k) => toggle_provider_raw(app, client),
            k if READING_PAGE_DOWN.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().preview.page_down()
            }
            k if READING_PAGE_UP.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().preview.page_up()
            }
            k if READING_DOWN.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_next()
            }
            k if READING_UP.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_prev()
            }
            k if READING_FIRST.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_first()
            }
            k if READING_LAST.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_last()
            }
            k if READING_PAGE_DOWN.matches(k) => app.raw.preview.page_down(),
            k if READING_PAGE_UP.matches(k) => app.raw.preview.page_up(),
            k if READING_DOWN.matches(k) => app.raw.select_next(),
            k if READING_UP.matches(k) => app.raw.select_prev(),
            k if READING_FIRST.matches(k) => app.raw.select_first(),
            k if READING_LAST.matches(k) => app.raw.select_last(),
            k if READING_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Log => match key {
            k if READING_DOWN.matches(k) => app.log.scroll_down(),
            k if READING_UP.matches(k) => app.log.scroll_up(),
            k if READING_FIRST.matches(k) => app.log.scroll_to_top(),
            k if READING_LAST.matches(k) => app.log.scroll_to_bottom(),
            _ => {}
        },
        // `R` for retry, in the view that shows the limit and the minute it
        // resets. It belongs here rather than among the global bindings
        // because that is where an operator whose session has just been cut
        // off is already looking: the notice saying the window is exhausted is
        // the reason they pressed `Q`.
        View::Quota => match key {
            k if READING_RETRY.matches(k) => app.ask(Request::SetAutoRetry(!app.auto_retry)),
            k if READING_DOWN.matches(k) => app.quota.scroll_down(),
            k if READING_UP.matches(k) => app.quota.scroll_up(),
            _ => {}
        },
        View::Transcript => match key {
            k if READING_LINKS.matches(k) => app.highlight_first_link(),
            k if READING_CONVERSATION_ONLY.matches(k) => app.toggle_conversation_only(),
            k if READING_DOWN.matches(k) => app.transcript.line_down(),
            k if READING_UP.matches(k) => app.transcript.line_up(),
            k if READING_FIRST.matches(k) => app.transcript.reset(),
            k if READING_LAST.matches(k) => app.transcript.scroll_to_end(),
            _ => {}
        },
        // Editing the launch policy. These keys deliberately avoid the letters
        // the global bindings above already claim (`t`, `n`, `d`, …), since
        // reaching the transcript or a new session from this view must keep
        // working while the policy is being edited.
        //
        // Every editing key acts on whichever of the two layers `Tab` has
        // focused, so there is one set of them to learn rather than one per
        // layer — and the view says which layer that is.
        View::Driva => match key {
            // Git checkout association is Workspace metadata, so it does not
            // depend on which policy pane happens to be focused.
            k if DRIVA_GIT_CHECKOUT.matches(k) => {
                app.git_repository_prompt = Some(
                    app.workspace
                        .git_repository
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                )
            }
            k if DRIVA_SCOPE.matches(k) => launch::toggle_scope(app),
            k if DRIVA_NETWORK.matches(k) => launch::cycle_network(app),
            // `R` for read-only: the workspace mount's access. Lowercase `r`
            // is claimed globally above (the raw view) and never gets here.
            k if DRIVA_ACCESS.matches(k) => launch::cycle_workspace_access(app),
            // `I` for whether this launch inherits: `S` is claimed globally
            // above (stopping the interaction) and never reaches this match.
            k if DRIVA_IGNORE_WORKSPACE.matches(k) => launch::toggle_ignore_workspace(app),
            k if DRIVA_TEMPLATES.matches(k) => {
                if app.allow_launch_edit() {
                    app.ask(Request::Templates);
                }
            }
            k if DRIVA_ADD_MOUNT.matches(k) => launch::open_prompt(app),
            k if DRIVA_REMOVE_MOUNT.matches(k) => launch::remove_selected_mount(app),
            // Mirrors `D` in the launch picker: keep this policy as the one a
            // brand-new client starts from, rather than only this session's.
            // Only this interaction's own settings are saved — the Workspace's
            // are already durable, and saving the merge would make every launch
            // elsewhere carry grants meant for this Workspace.
            k if DRIVA_SAVE_DEFAULT.matches(k) => {
                if app.allow_launch_edit() {
                    let launch = app.launch.interaction.clone();
                    match preferences::save_launch(preferences_path, &launch) {
                        Ok(()) => app.show_action_message(
                            "saved this interaction's settings as the default for new clients",
                        ),
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not save the default launch policy: {error:#}"
                        ))),
                    }
                }
            }
            // Move what this interaction added up into the Workspace's standing
            // policy, once it turns out not to be particular to this
            // conversation after all.
            k if DRIVA_PROMOTE.matches(k) => launch::promote_to_workspace(app),
            k if DRIVA_NEXT_MOUNT.matches(k) => launch::select_next_mount(app),
            k if DRIVA_PREV_MOUNT.matches(k) => launch::select_prev_mount(app),
            // The sandbox account above the panes is longer than a terminal —
            // mounts, the backend's floor, the environment, the private root —
            // and all of it is meant to be readable, so what does not fit is
            // paged rather than lost. `j`/`k` are the mount cursor's.
            k if DRIVA_PAGE_DOWN.matches(k) => app.launch.scroll.page_down(),
            k if DRIVA_PAGE_UP.matches(k) => app.launch.scroll.page_up(),
            _ => {}
        },
        // Re-reading is on the capitals so `j` and `k` stay navigation, as
        // they are in every other view.
        View::Answer => match key {
            k if ANSWER_AS_TEXT.matches(k) => app.reread_answer(Contract::Text),
            k if ANSWER_AS_LINES.matches(k) => app.reread_answer(Contract::Lines),
            k if ANSWER_AS_FILES.matches(k) => app.reread_answer(Contract::Files),
            k if ANSWER_AS_JSON.matches(k) => app.reread_answer(Contract::Json),
            k if ANSWER_REREAD.matches(k) => app.ask(Request::Answer { contract: None }),
            k if ANSWER_EDIT.matches(k) && app.answer.selected_file().is_some() => {
                app.ask(Request::EditFile)
            }
            k if ANSWER_NEXT.matches(k) => app.answer.select_next(),
            k if ANSWER_PREV.matches(k) => app.answer.select_prev(),
            k if ANSWER_FIRST.matches(k) => app.answer.select_first(),
            k if ANSWER_LAST.matches(k) => app.answer.select_last(),
            k if ANSWER_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Files => match key {
            k if FILES_EDIT.matches(k) && app.selected_file_path().is_some() => {
                app.ask(Request::EditFile)
            }
            k if FILES_NEXT.matches(k) => app.file_select_next(),
            k if FILES_PREV.matches(k) => app.file_select_prev(),
            k if FILES_NEXT_ENTRY.matches(k) => {
                app.select_next_line();
                app.files.select_first();
            }
            k if FILES_PREV_ENTRY.matches(k) => {
                app.select_prev_line();
                app.files.select_first();
            }
            k if FILES_FIRST.matches(k) => app.files.select_first(),
            k if FILES_LAST.matches(k) => {
                let last = app.file_paths().len().saturating_sub(1);
                app.files.select_last(last);
            }
            k if FILES_SCOPE.matches(k) => app.toggle_file_scope(),
            k if FILES_PREVIEW.matches(k) => app.preview.toggle(),
            k if FILES_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        // Full-screen preview is the one view where the text, not the entry
        // list, is what the reader is moving through: `j`/`k` scroll it ten
        // lines at a time and the shifted pair changes entry.
        View::Preview => match key {
            k if PREVIEW_LINKS.matches(k) => app.highlight_first_link(),
            k if PREVIEW_LINK_DESTINATIONS.matches(k) => app.toggle_link_display(),
            k if PREVIEW_MODE.matches(k) => app.preview.toggle_mode(),
            k if PREVIEW_TARGET.matches(k) => app.preview.toggle_target(),
            k if PREVIEW_PAGE_DOWN.matches(k) => app.preview.scroll.page_down(),
            k if PREVIEW_PAGE_UP.matches(k) => app.preview.scroll.page_up(),
            k if PREVIEW_SCROLL_DOWN.matches(k) && app.link_highlight.is_some() => {
                app.highlight_next_link()
            }
            k if PREVIEW_SCROLL_UP.matches(k) && app.link_highlight.is_some() => {
                app.highlight_prev_link()
            }
            k if PREVIEW_SCROLL_DOWN.matches(k) => app.preview.scroll.page_down(),
            k if PREVIEW_SCROLL_UP.matches(k) => app.preview.scroll.page_up(),
            k if PREVIEW_NEXT_ENTRY.matches(k) => app.select_next_line(),
            k if PREVIEW_PREV_ENTRY.matches(k) => app.select_prev_line(),
            k if PREVIEW_FIRST.matches(k) => app.select_first(),
            k if PREVIEW_LAST.matches(k) => app.select_last(),
            k if PREVIEW_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
    }
}

/// Open the tag editor for the Interaction currently on screen. The
/// interaction snapshot is refreshed here because the log is useful even
/// before the live-interactions navigator has ever been opened.
fn edit_current_interaction_tags(app: &mut App, client: &Client) {
    let session_id = app.session_id.clone();
    let interactions = match client.list_interactions() {
        Ok(interactions) => interactions,
        Err(error) => {
            app.show_action_message(format!("could not list interactions: {error:#}"));
            return;
        }
    };
    let Some(selected_tags) = interactions
        .iter()
        .find(|item| item.id == session_id)
        .map(|item| item.tags.clone())
    else {
        app.show_action_message("current interaction is no longer available");
        return;
    };
    let tags = match client.list_tags() {
        Ok(tags) => tags,
        Err(error) => {
            app.show_action_message(format!("could not list tags: {error:#}"));
            return;
        }
    };
    app.interactions.refresh(interactions);
    app.tag_picker = Some(crate::tag_picker::TagPicker::new(tags, selected_tags));
}

/// Switch the raw panel between Styra's wire capture and the provider's
/// native persisted JSONL. Read freshly when opening it: a live Codex thread
/// can append records after the previous visit.
fn toggle_provider_raw(app: &mut App, client: &Client) {
    if app.provider_raw_open {
        app.provider_raw_open = false;
        return;
    }
    match client.provider_raw(&app.session_id) {
        Ok(raw) => {
            app.provider_raw = Some(crate::raw::ProviderRawView::new(raw.text));
            app.provider_raw_open = true;
        }
        Err(error) => app.show_action_message(format!("could not read provider raw: {error:#}")),
    }
}

/// Copy whatever the current view treats as the selected entry to the
/// clipboard (see `App::copy_text`).
fn copy_selection(app: &mut App) {
    let Some(text) = app.copy_text() else {
        return app.show_action_message("nothing selected to copy");
    };
    copy(app, text, "copied to clipboard");
}

/// Copy the session's whole conversation — messages, errors, and model
/// changes, without the tool calls between them.
fn copy_conversation(app: &mut App) {
    let Some(text) = app.conversation_text() else {
        return app.show_action_message("no conversation to copy yet");
    };
    copy(app, text, "copied the conversation to clipboard");
}

/// Send text to the clipboard, reporting the outcome the same way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
fn copy(app: &mut App, text: String, done: &str) {
    match crate::clipboard::copy(&text) {
        Ok(()) => app.show_action_message(done),
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not copy to clipboard: {error:#}"
        ))),
    }
}

/// Open the path prompt over the message box, against what this session's
/// sandbox carries and whether that sandbox can still be changed.
fn open_insert(app: &mut App) {
    app.insert = Some(insert::Prompt::new(
        app.workspace.root().map(std::path::Path::to_path_buf),
        app.launch.driva.as_ref(),
        app.can_edit_launch(),
    ));
}

/// Route a key to the open path prompt and apply what it decided to the
/// message being composed. The prompt itself knows nothing about [`App`]; this
/// is where its outcome becomes message text, a mount request, and a notice.
pub fn handle_insert_key(app: &mut App, key: KeyEvent) {
    let Some(prompt) = app.insert.as_mut() else {
        return;
    };
    match prompt.key(key) {
        insert::Outcome::Open => {}
        insert::Outcome::Closed => app.insert = None,
        insert::Outcome::Notice(notice) => app.show_action_message(notice),
        insert::Outcome::Insert { path, notice } => {
            app.insert = None;
            app.composer.insert(&path.display().to_string());
            if let Some(notice) = notice {
                app.show_action_message(notice);
            }
        }
        insert::Outcome::Grant { mount, path } => {
            app.insert = None;
            let label = crate::mount::label(&mount);
            let message = match app.launch.add_interaction_mount(mount) {
                // The mount is a request, not a live change: nothing rebinds
                // the sandbox of an interaction that has already started, so
                // the message says when it will actually apply rather than
                // only that it was added.
                Ok(()) => format!("added {label} — applies when this Session next launches"),
                Err(reason) => reason.to_owned(),
            };
            app.composer.insert(&path.display().to_string());
            app.show_action_message(message);
        }
    }
}

/// Whether this keypress submits the first prompt into a new Git workspace.
///
/// Ctrl-Enter is a distinct first-prompt submission: it creates the Session's
/// branch and linked workspace as it sends the prompt, with no standing option
/// to leak into a later Session.
///
/// Asked from outside as well as here, because branching takes long enough to
/// be worth saying on screen before the send blocks on it.
pub fn creates_worktree(app: &App, key: KeyEvent) -> bool {
    EDITOR_SEND_IN_BRANCH.matches(key)
        && app.session_id.is_empty()
        // Nothing is sent, and so nothing is branched, for a blank box.
        && !app.composer.text.trim().is_empty()
}

pub fn handle_input_key(
    app: &mut App,
    client: &Client,
    workspace_id: &str,
    live: &mut Attachment,
    key: KeyEvent,
) {
    let create_worktree = creates_worktree(app, key);
    match key {
        k if GLOBAL_LEAVE_MESSAGE.matches(k) => app.enter_list(),
        // Choosing a shape is part of writing the message, so it lives in the
        // box rather than being a mode entered from outside it.
        k if EDITOR_CONTRACT.matches(k) => app.outbox.cycle_contract(),
        k if EDITOR_NEWLINE.matches(k) => app.composer.newline(),
        k if EDITOR_SEND.matches(k) || EDITOR_SEND_IN_BRANCH.matches(k) => {
            if let Some(message) = app.take_message() {
                app.enter_list();
                session::submit_message(app, client, workspace_id, live, message, create_worktree);
            }
        }
        k if EDITOR_DELETE_WORD.matches(k) => app.composer.delete_word(),
        k if EDITOR_LAUNCHER.matches(k) => app.open_launcher(),
        // Naming a file is part of writing the message, so it opens from the
        // box rather than from the driva view that the grant it may ask for
        // would otherwise have to be made in.
        k if EDITOR_INSERT_PATH.matches(k) => open_insert(app),
        k if EDITOR_HISTORY_OLDER.matches(k) => app.composer.history_previous(),
        k if EDITOR_HISTORY_NEWER.matches(k) => app.composer.history_next(),
        k if k.code == KeyCode::Backspace => app.composer.backspace(),
        // Everything else printable is the message itself, once the modified
        // keys above have had their turn.
        k if !k.modifiers.contains(KeyModifiers::CONTROL)
            && !k.modifiers.contains(KeyModifiers::ALT) =>
        {
            if let KeyCode::Char(ch) = k.code {
                app.composer.char(ch)
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_protocol::{
        AttributedMount, DrivaOptions, LaunchMount, Mount, MountAccess, MountOrigin,
    };

    /// A session whose sandbox binds `root` at `/workspace` and nothing else,
    /// with nothing launched — so the launch policy is still open to editing.
    fn app(root: &Path) -> App {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.workspace.enter(root.to_path_buf());
        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/workspace"),
            network: false,
            base: Vec::new(),
            mounts: vec![AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: root.to_path_buf(),
                    destination: PathBuf::from("/workspace"),
                    access: MountAccess::ReadWrite,
                },
            }],
            ..Default::default()
        });
        app.enter_input();
        app
    }

    fn tree(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("styra-keys-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("reports")).unwrap();
        std::fs::write(base.join("reports/summary.md"), "x").unwrap();
        std::fs::write(base.join("notes.txt"), "x").unwrap();
        std::fs::canonicalize(base).unwrap()
    }

    fn typed(app: &mut App, text: &str) {
        open_insert(app);
        for ch in text.chars() {
            handle_insert_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        handle_insert_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    #[test]
    fn uppercase_g_in_details_opens_the_git_checkout_prompt_prefilled() {
        let root = tree("git-checkout-prompt");
        let mut app = app(&root);
        app.enter_list();
        app.workspace.git_repository = Some(root.join("repository"));
        app.toggle_view(View::Driva);
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.git_repository_prompt.as_deref(),
            Some(root.join("repository").to_string_lossy().as_ref())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The two halves of the same list: `a` opens it, and ctrl-a asks to be
    /// taken to the interaction the footer is counting without walking it.
    #[test]
    fn control_a_asks_for_the_next_unseen_idle_interaction() {
        let root = tree("idle-jump");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        let mut press = |app: &mut App, modifiers| {
            handle_list_key(
                app,
                &client,
                &mut live,
                KeyEvent::new(KeyCode::Char('a'), modifiers),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
        };

        press(&mut app, KeyModifiers::CONTROL);
        assert_eq!(app.take_request(), Some(Request::NextIdleInteraction));

        press(&mut app, KeyModifiers::NONE);
        assert_eq!(app.take_request(), Some(Request::Interactions));

        let _ = std::fs::remove_dir_all(root);
    }

    /// N starts a new session, while plain `n` moves to another live one.
    #[test]
    fn uppercase_n_starts_a_new_session() {
        let root = tree("live-step");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        let mut press = |app: &mut App, code, modifiers| {
            handle_list_key(
                app,
                &client,
                &mut live,
                KeyEvent::new(code, modifiers),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
        };

        press(&mut app, KeyCode::Char('N'), KeyModifiers::SHIFT);
        assert_eq!(app.take_request(), Some(Request::NewSession));

        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.take_request(), Some(Request::NextLiveInteraction));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plain_l_opens_the_launcher_from_the_main_view() {
        let root = tree("launcher-shortcut");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert!(app.launcher.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn control_l_toggles_the_log_view() {
        let root = tree("log-shortcut");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(app.view, View::Log);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The event loop asks this before dispatching the key, so that it can
    /// paint the "creating…" notice over the pause the branching costs. It has
    /// to agree with the send itself about which presses actually branch.
    #[test]
    fn only_a_typed_first_prompt_sent_with_control_enter_branches() {
        let root = tree("creates-worktree");
        let mut app = app(&root);
        let press = |modifiers| KeyEvent::new(KeyCode::Enter, modifiers);

        app.composer.set("start here".into());
        assert!(creates_worktree(&app, press(KeyModifiers::CONTROL)));
        assert!(
            !creates_worktree(&app, press(KeyModifiers::NONE)),
            "a plain Enter sends into the current workspace"
        );

        app.composer.set("   ".into());
        assert!(
            !creates_worktree(&app, press(KeyModifiers::CONTROL)),
            "a blank box sends nothing, so it branches nothing"
        );

        app.composer.set("continue".into());
        app.session_id = "session-1".into();
        assert!(
            !creates_worktree(&app, press(KeyModifiers::CONTROL)),
            "branching is a first-prompt choice only"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uppercase_w_requests_a_worktree_for_an_existing_session() {
        let root = tree("session-worktree");
        let mut app = app(&root);
        app.enter_list();
        // `W` only means anything for a session that exists; the guard on the
        // key reads the id the list is sitting on.
        app.session_id = "session-1".into();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(app.take_request(), Some(Request::CreateSessionWorktree));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enter_on_a_branch_marker_opens_the_linked_interaction() {
        let root = tree("follow-branch-enter");
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::Branched {
            direction: styra_protocol::event::BranchDirection::To,
            session: "branch-2".into(),
            name: None,
        });
        app.select_last();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::OpenSession("branch-2".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enter_on_a_from_branch_marker_opens_the_linked_interaction() {
        let root = tree("follow-branch-enter-source");
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::Branched {
            direction: styra_protocol::event::BranchDirection::From,
            session: "source-1".into(),
            name: None,
        });
        app.select_last();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::OpenSession("source-1".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enter_on_a_regular_event_still_toggles_its_expansion() {
        let root = tree("regular-enter");
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
            text: "ordinary reply".into(),
        });
        app.select_last();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert!(app.timeline.selected_entry().unwrap().expanded);
        assert!(app.take_request().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// `~` asks for a terminal wherever the interaction is working, with no
    /// live attachment needed: unlike `!`, which attaches to a running
    /// sandbox, a finished interaction still has a directory to stand in.
    #[test]
    fn tilde_asks_for_a_terminal_in_the_interaction_directory() {
        let root = tree("terminal-here");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('~'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(app.take_request(), Some(Request::OpenDirectory));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the prompt decides reaches the message: a path the sandbox already
    /// carries goes in under the name the agent knows it by.
    #[test]
    fn a_decided_path_goes_into_the_message_being_composed() {
        let root = tree("mounted");
        let mut app = app(&root);

        typed(&mut app, "reports/summary.md");

        assert!(app.insert.is_none());
        assert_eq!(app.composer.text, "/workspace/reports/summary.md");
        assert!(app.launch.interaction.mounts.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// And a granted one also reaches the launch policy, as this
    /// interaction's own mount.
    #[test]
    fn a_granted_path_is_added_to_this_interactions_mounts() {
        let root = tree("granted");
        let outside = tree("granted-elsewhere");
        let mut app = app(&root);
        let host = outside.join("notes.txt");

        typed(&mut app, &host.display().to_string());
        handle_insert_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
        );

        assert!(app.insert.is_none());
        assert_eq!(
            app.launch.interaction.mounts,
            vec![LaunchMount {
                source: host.clone(),
                destination: None,
                writable: true,
            }]
        );
        assert_eq!(app.composer.text, host.display().to_string());
        assert!(app.notices.iter().any(|message| message
            .text
            .contains("applies when this Session next launches")));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
