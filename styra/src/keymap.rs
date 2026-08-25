//! The terminal client's visible keyboard map.
//!
//! Keep a command's displayed keys and description here.  Renderers consume
//! this catalogue rather than carrying their own copies of the shortcut text.

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

/// Every shortcut advertised by Styra's UI, in display order.
pub(crate) const REFERENCE: &[ReferenceRow] = &[
    ReferenceRow::Section("Global"),
    ReferenceRow::Binding {
        keys: HELP,
        action: "show/close this reference",
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
        keys: "b",
        action: "branch a new session from the selected entry",
    },
    ReferenceRow::Binding {
        keys: "n / N",
        action: "new session / stop and start new session",
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
        keys: "a / A / V; E",
        action: "current sessions/interactions/Workspaces; notes",
    },
    ReferenceRow::Binding {
        keys: "r / l / t / d",
        action: "raw / log / transcript / driva; press again for events",
    },
    ReferenceRow::Binding {
        keys: "f",
        action: "files mentioned by the focused entry",
    },
    ReferenceRow::Blank,
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
        keys: "v / C",
        action: "pretty/diff preview; preview the newest command",
    },
    ReferenceRow::Binding {
        keys: "PgUp/PgDn",
        action: "scroll preview (full-screen: j/k, entry: J/K)",
    },
    ReferenceRow::Binding {
        keys: "y",
        action: "copy selected entry to clipboard",
    },
    ReferenceRow::Blank,
    ReferenceRow::Section("Raw, log, and transcript"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move or scroll",
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
        keys: "y",
        action: "copy selected line to clipboard (raw view)",
    },
    ReferenceRow::Blank,
    ReferenceRow::Section("Driva (launch policy, before an interaction starts)"),
    ReferenceRow::Binding {
        keys: "w",
        action: "permit/forbid agent networking",
    },
    ReferenceRow::Binding {
        keys: "T",
        action: "choose Driva templates",
    },
    ReferenceRow::Binding {
        keys: "m / g / x",
        action: "add a mount / the git history here (rw) / remove the selected one",
    },
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "move among the mounts you added",
    },
    ReferenceRow::Binding {
        keys: "I",
        action: "add to / ignore the Workspace policy",
    },
    ReferenceRow::Binding {
        keys: "D / W",
        action: "save this policy for new clients / this Workspace",
    },
    ReferenceRow::Blank,
    ReferenceRow::Section("Files"),
    ReferenceRow::Binding {
        keys: "j/k or ↓/↑",
        action: "next/previous file",
    },
    ReferenceRow::Binding {
        keys: "J/K",
        action: "next/previous interaction-log entry",
    },
    ReferenceRow::Binding {
        keys: "e",
        action: "open selected file in editor",
    },
    ReferenceRow::Binding {
        keys: "p",
        action: "toggle interaction preview",
    },
    ReferenceRow::Binding {
        keys: "a",
        action: "toggle focused-entry/all-session files",
    },
    ReferenceRow::Binding {
        keys: "y",
        action: "copy selected file's path to clipboard",
    },
    ReferenceRow::Blank,
    ReferenceRow::Section("Message editor"),
    ReferenceRow::Binding {
        keys: "Enter",
        action: "send message",
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
    ReferenceRow::Blank,
    ReferenceRow::Section("Launch and selection screens"),
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
