//! The terminal client's visible keyboard map.
//!
//! Keep a command's displayed keys and description here.  Renderers consume
//! this catalogue rather than carrying their own copies of the shortcut text.
//!
//! The map is cut into sections, and each window names the sections that apply
//! to it: `?` answers for what is on screen rather than for the whole client.
//! Screens therefore no longer carry a strip of shortcuts along their top —
//! the strip could only ever list the few that fit, and it cost a line of the
//! list it sat above.

/// One row in the keyboard reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceRow {
    Section(&'static str),
    Binding {
        keys: &'static str,
        action: &'static str,
    },
    Blank,
}

pub(crate) const HELP: &str = "?";
pub(crate) const CLOSE_REFERENCE: &str = "?, Esc, or q";

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
    References,
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
            Self::References => "files in this entry",
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
            Self::Raw | Self::Log | Self::Quota | Self::Transcript | Self::Preview => {
                &[READING, MESSAGE_EDITOR, GLOBAL]
            }
            Self::Driva => &[DRIVA, GLOBAL],
            Self::Files => &[FILES, GLOBAL],
            Self::Answer => &[ANSWER, GLOBAL],
            Self::Interactions => &[INTERACTIONS, GLOBAL],
            Self::Branch => &[BRANCH, GLOBAL],
            Self::Tags => &[TAGS, GLOBAL],
            Self::References => &[REFERENCES, GLOBAL],
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

/// What holds anywhere in a loaded session, whichever of its windows is up.
const GLOBAL: &[ReferenceRow] = &[
    ReferenceRow::Section("Global"),
    ReferenceRow::Binding {
        keys: HELP,
        action: "show/close the reference for this window",
    },
    ReferenceRow::Binding {
        keys: "i / Esc",
        action: "focus message / return to list",
    },
    ReferenceRow::Binding {
        keys: "q",
        action: "quit",
    },
    ReferenceRow::Binding {
        keys: "s / S",
        action: "interrupt active turn / stop interaction",
    },
    ReferenceRow::Binding {
        keys: "B",
        action: "branch from history through, or only, the selected entry",
    },
    ReferenceRow::Binding {
        keys: "n / N",
        action: "new session where this one works / stop and start new",
    },
    ReferenceRow::Binding {
        keys: "L",
        action: "choose model for an idle agent turn",
    },
    ReferenceRow::Binding {
        keys: "!",
        action: "open session shell in a new terminal",
    },
    ReferenceRow::Binding {
        keys: "~",
        action: "open a terminal in the interaction's working directory",
    },
    ReferenceRow::Binding {
        keys: "a / A / V",
        action: "live interactions/sessions/Workspaces",
    },
    ReferenceRow::Binding {
        keys: "W (existing session)",
        action: "create and associate a Git branch and workspace",
    },
    ReferenceRow::Binding {
        keys: "T",
        action: "edit the current interaction's tags",
    },
    ReferenceRow::Binding {
        keys: "ctrl-a",
        action: "go to the next interaction that went idle unseen",
    },
    ReferenceRow::Binding {
        keys: "ctrl-n",
        action: "step to the next interaction that is still running",
    },
    ReferenceRow::Binding {
        keys: "r / l / t / d",
        action: "raw / log / transcript / details; press again for events",
    },
    ReferenceRow::Binding {
        keys: "e",
        action: "toggle the entry log below the event list",
    },
    ReferenceRow::Binding {
        keys: "Tab",
        action: "move between the event list and the open entry log",
    },
    ReferenceRow::Binding {
        keys: "Q",
        action: "plan quota readings, refreshed from the server",
    },
    ReferenceRow::Binding {
        keys: "f",
        action: "files mentioned by the focused entry",
    },
    ReferenceRow::Binding {
        keys: "X",
        action: "the last turn's typed answer",
    },
];

const EVENTS: &[ReferenceRow] = &[
    ReferenceRow::Section("Events and previews"),
    ReferenceRow::Binding {
        keys: "J/K or ↓/↑",
        action: "next/previous entry",
    },
    ReferenceRow::Binding {
        keys: "j/k",
        action: "next/previous line",
    },
    ReferenceRow::Binding {
        keys: "g/G",
        action: "first/last entry",
    },
    ReferenceRow::Binding {
        keys: "Enter (branch marker), b",
        action: "follow the linked Session",
    },
    ReferenceRow::Binding {
        keys: "Space, Enter, o",
        action: "toggle selected entry",
    },
    ReferenceRow::Binding {
        keys: "O",
        action: "expand only selected",
    },
    ReferenceRow::Binding {
        keys: "z R / z M",
        action: "expand all / collapse all",
    },
    ReferenceRow::Binding {
        keys: "m / p",
        action: "toggle minor events / preview panel",
    },
    ReferenceRow::Binding {
        keys: "c",
        action: "toggle conversation-only events",
    },
    ReferenceRow::Binding {
        keys: "P",
        action: "toggle full-screen preview",
    },
    ReferenceRow::Binding {
        keys: "v / C (preview open)",
        action: "pretty/diff preview; preview the newest command",
    },
    ReferenceRow::Binding {
        keys: "C (preview closed)",
        action: "mark this interaction completed and stop it",
    },
    ReferenceRow::Binding {
        keys: "u",
        action: "toggle link destinations",
    },
    ReferenceRow::Binding {
        keys: "PgUp/PgDn",
        action: "scroll preview (full-screen: j/k, entry: J/K)",
    },
    ReferenceRow::Binding {
        keys: "F",
        action: "open a file the entry cites (Enter opens, q cancels)",
    },
    ReferenceRow::Binding {
        keys: "y",
        action: "copy selected entry to clipboard",
    },
    ReferenceRow::Binding {
        keys: "Y",
        action: "copy the whole conversation (any view)",
    },
];

const READING: &[ReferenceRow] = &[
    ReferenceRow::Section("Raw, log, quota, and transcript"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move or scroll",
    },
    ReferenceRow::Binding {
        keys: "R (quota)",
        action: "after a rate limit, ask this session again once the window resets",
    },
    ReferenceRow::Binding {
        keys: "g/G",
        action: "first/top or last/bottom",
    },
    ReferenceRow::Binding {
        keys: "PgUp/PgDn",
        action: "scroll raw-line preview",
    },
    ReferenceRow::Binding {
        keys: "v (raw)",
        action: "switch Styra wire capture / provider-native session JSONL",
    },
    ReferenceRow::Binding {
        keys: "y",
        action: "copy selected line to clipboard (raw view)",
    },
];

const DRIVA: &[ReferenceRow] = &[
    ReferenceRow::Section("Details (Workspace, interaction, and launch policy)"),
    ReferenceRow::Binding {
        keys: "Tab; j/k or ↓/↑",
        action: "edit the Workspace's policy / this interaction's; its mounts",
    },
    ReferenceRow::Binding {
        keys: "w",
        action: "permit/forbid agent networking, in the focused layer",
    },
    ReferenceRow::Binding {
        keys: "R",
        action: "mount the workspace read-write/read-only, in the focused layer",
    },
    ReferenceRow::Binding {
        keys: "T",
        action: "choose Driva templates",
    },
    ReferenceRow::Binding {
        keys: "m / G / x",
        action: "add a mount / set this Workspace's Git checkout / remove selected mount",
    },
    ReferenceRow::Binding {
        keys: "I",
        action: "this interaction adds to / ignores the Workspace policy",
    },
    ReferenceRow::Binding {
        keys: "U / D / W",
        action: "move this interaction's up / save for new clients / store the Workspace's",
    },
    ReferenceRow::Binding {
        keys: "PgDn/PgUp",
        action: "scroll the sandbox account: mounts, floor, environment, private root",
    },
];

const FILES: &[ReferenceRow] = &[
    ReferenceRow::Section("Files"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑; J/K",
        action: "next/previous file; interaction-log entry",
    },
    ReferenceRow::Binding {
        keys: "e; p",
        action: "open selected file in editor; toggle interaction preview",
    },
    ReferenceRow::Binding {
        keys: "a; y",
        action: "focused-entry/all-session files; copy the path",
    },
];

const ANSWER: &[ReferenceRow] = &[
    ReferenceRow::Section("Typed answer"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑; e; y",
        action: "next/previous item; open location in editor; copy",
    },
    ReferenceRow::Binding {
        keys: "T/L/F/J; R",
        action: "re-read as text/lines/files/json; as the turn asked",
    },
];

const MESSAGE_EDITOR: &[ReferenceRow] = &[
    ReferenceRow::Section("Message editor"),
    ReferenceRow::Binding {
        keys: "Ctrl+T",
        action: "ask this message's reply for a shape (text/lines/files/json)",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "send message",
    },
    ReferenceRow::Binding {
        keys: "Ctrl+Enter (first prompt)",
        action: "send in a new Git branch and workspace",
    },
    ReferenceRow::Binding {
        keys: "/cd <directory>",
        action: "change the live Codex interaction directory",
    },
    ReferenceRow::Binding {
        keys: "Alt+Enter",
        action: "insert newline",
    },
    ReferenceRow::Binding {
        keys: "↑/↓",
        action: "older/newer message history",
    },
    ReferenceRow::Binding {
        keys: "Ctrl+W",
        action: "delete previous word",
    },
    ReferenceRow::Binding {
        keys: "Ctrl+L",
        action: "choose model before first message or idle agent turn",
    },
    ReferenceRow::Binding {
        keys: "Ctrl+F",
        action: "insert a file path (Tab completes, Enter inserts)",
    },
    ReferenceRow::Binding {
        keys: "r / w / n",
        action: "when that path is unmounted: mount it readable / writable / neither",
    },
];

const INTERACTIONS: &[ReferenceRow] = &[
    ReferenceRow::Section("Interactions"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move the cursor; the rested-on interaction becomes current",
    },
    ReferenceRow::Binding {
        keys: "ctrl-j / ctrl-k",
        action: "first interaction of the next / previous Workspace, in All",
    },
    ReferenceRow::Binding {
        keys: "ctrl-n",
        action: "next interaction that is still running",
    },
    ReferenceRow::Binding {
        keys: "w",
        action: "current Workspace / all Workspaces",
    },
    ReferenceRow::Binding {
        keys: "c / C",
        action: "show/hide completed / mark selected completed and stop it",
    },
    ReferenceRow::Binding {
        keys: "T",
        action: "edit the selected interaction's tags",
    },
    ReferenceRow::Binding {
        keys: "S / D",
        action: "stop the selected interaction / delete it once stopped",
    },
    ReferenceRow::Binding {
        keys: "Enter or a",
        action: "close the list",
    },
];

const BRANCH: &[ReferenceRow] = &[
    ReferenceRow::Section("Branch from selected entry"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "entire interaction through this entry / only this entry",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "branch, and open the result",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

const TAGS: &[ReferenceRow] = &[
    ReferenceRow::Section("Interaction tags"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move selection",
    },
    ReferenceRow::Binding {
        keys: "Space",
        action: "toggle the selected tag",
    },
    ReferenceRow::Binding {
        keys: "n",
        action: "add a new tag (Enter adds and saves)",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "save",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

const REFERENCES: &[ReferenceRow] = &[
    ReferenceRow::Section("Files in this entry"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move selection",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "open the selected file",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

const SESSION_PICKER: &[ReferenceRow] = &[
    ReferenceRow::Section("Stored sessions"),
    ReferenceRow::Binding {
        keys: HELP,
        action: "show/close this reference",
    },
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move selection",
    },
    ReferenceRow::Binding {
        keys: "J/K",
        action: "next/previous root conversation, skipping its branches",
    },
    ReferenceRow::Binding {
        keys: "g/G",
        action: "first/last session",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "open the selected session",
    },
    ReferenceRow::Binding {
        keys: "n",
        action: "start a new session in this Workspace",
    },
    ReferenceRow::Binding {
        keys: "c / C",
        action: "show/hide completed / mark selected completed or not",
    },
    ReferenceRow::Binding {
        keys: "/",
        action: "filter by name or first prompt (Esc clears)",
    },
    ReferenceRow::Binding {
        keys: "s",
        action: "sort by last activity / by creation",
    },
    ReferenceRow::Binding {
        keys: "a",
        action: "show/hide history older than a week",
    },
    ReferenceRow::Binding {
        keys: "r",
        action: "rename the selected session",
    },
    ReferenceRow::Binding {
        keys: "x",
        action: "convert the selected session to the other provider",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

const WORKSPACE_PICKER: &[ReferenceRow] = &[
    ReferenceRow::Section("Workspaces"),
    ReferenceRow::Binding {
        keys: HELP,
        action: "show/close this reference",
    },
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move selection",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "open the selected Workspace",
    },
    ReferenceRow::Binding {
        keys: "c",
        action: "create a Workspace for the current directory",
    },
    ReferenceRow::Binding {
        keys: "/",
        action: "filter by name (Esc clears)",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

const LAUNCHER: &[ReferenceRow] = &[
    ReferenceRow::Section("Launch"),
    ReferenceRow::Binding {
        keys: HELP,
        action: "show/close this reference",
    },
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move selection",
    },
    ReferenceRow::Binding {
        keys: "Tab, h/l, ←/→",
        action: "move launch column",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "select",
    },
    ReferenceRow::Binding {
        keys: "D",
        action: "select and save launch default",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

const TEMPLATE_PICKER: &[ReferenceRow] = &[
    ReferenceRow::Section("Driva templates"),
    ReferenceRow::Binding {
        keys: HELP,
        action: "show/close this reference",
    },
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move selection",
    },
    ReferenceRow::Binding {
        keys: "Space",
        action: "add/remove the selected template",
    },
    ReferenceRow::Binding {
        keys: "Enter",
        action: "apply; templates layer in the order chosen",
    },
    ReferenceRow::Binding {
        keys: "Esc or q",
        action: "cancel",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every window answers `?` with something, and with its own commands
    /// first: the reference is for what is on screen, not a catalogue the
    /// operator has to search.
    #[test]
    fn every_window_leads_with_its_own_section() {
        for window in [
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
            Window::References,
            Window::SessionPicker,
            Window::WorkspacePicker,
            Window::Launcher,
            Window::TemplatePicker,
        ] {
            let rows = window.reference();
            assert!(
                matches!(rows.first(), Some(ReferenceRow::Section(_))),
                "{} opens with a section heading",
                window.name()
            );
            assert!(
                rows.iter()
                    .any(|row| matches!(row, ReferenceRow::Binding { .. })),
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
            assert!(
                rows.contains(&ReferenceRow::Binding {
                    keys: HELP,
                    action: "show/close this reference",
                }),
                "{} says how to reopen the reference",
                window.name()
            );
            assert!(
                rows.iter().any(
                    |row| matches!(row, ReferenceRow::Binding { action, .. } if *action
                        == "cancel")
                ),
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
}
