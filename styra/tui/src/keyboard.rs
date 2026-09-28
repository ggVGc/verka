//! Keyboard input primitives shared by the binding configuration and UI.
//!
//! This module contains representation and matching machinery. Edit
//! [`crate::keybindings`] to customize keys.

use crate::actions::Action;
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
    action: Action,
}

impl Binding {
    pub(crate) const fn new(
        keys: &'static [Key],
        label: Option<&'static str>,
        note: Option<&'static str>,
        action: Action,
    ) -> Self {
        Self {
            keys,
            label,
            note,
            action,
        }
    }

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

    pub(crate) const fn action(&self) -> Action {
        self.action
    }
}

/// Declare a section of the reference, and with it the bindings it is made of.
///
/// ```ignore
/// bindings! { SECTION = "Heading";
///     NAME: [Key::ch('j'), Key::code(KeyCode::Down)] => Action::Example;
///     CHORD: [Key::ch('R')] as "z R" => Action::Example;
///     NARROW: [Key::ch('v')] ("preview open") => Action::Example;
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
            $name:ident: [$($key:expr),* $(,)?] $(as $label:literal)? $(($note:literal))? => $action:expr;
        )*
    ) => {
        $(
            $(#[$meta])*
            pub(crate) const $name: Binding = Binding::new(
                &[$($key),*],
                bindings!(@label $($label)?),
                bindings!(@note $($note)?),
                $action,
            );
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

pub(crate) use bindings;

/// One row in the keyboard reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceRow {
    Section(&'static str),
    Binding(&'static Binding),
    Blank,
}
