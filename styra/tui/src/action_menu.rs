//! The action menu: every command of the window it was opened over, in a list
//! that narrows as it is typed at, for running one without knowing its key.
//!
//! A chosen action is not carried out here. The menu answers with the
//! keypresses its binding is reached by, and the event loop feeds them back
//! through itself — so an action run from the menu is the same code path, with
//! the same guards, as the key it names, and the two cannot drift apart.

use crate::keybindings::{self as keys, Window};
use crate::keyboard::Binding;
use crossterm::event::KeyEvent;
use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::query::Edit;

/// What a key did to the open menu.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The menu stays open.
    Stay,
    /// Closed without running anything.
    Cancel,
    /// Closed on a choice: these keys run it.
    Run(Vec<KeyEvent>),
}

pub struct ActionMenu {
    /// The window the actions belong to, which the menu now covers.
    window: Window,
    bindings: Vec<&'static Binding>,
    /// Each action as the list shows it, with its keys in parentheses so the
    /// menu also teaches the shortcut it stands in for.
    rows: Vec<String>,
    pub list: FuzzyList,
}

impl ActionMenu {
    pub fn new(window: Window) -> Self {
        let bindings = window.actions();
        let rows = bindings.iter().map(|binding| row(binding)).collect();
        Self {
            window,
            bindings,
            rows,
            list: FuzzyList::default(),
        }
    }

    pub fn window(&self) -> Window {
        self.window
    }

    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key {
            k if keys::ACTION_MENU_CANCEL.matches(k) => return Outcome::Cancel,
            k if keys::ACTION_MENU_RUN.matches(k) => {
                return match self.list.selected_row(&self.rows) {
                    Some(index) => Outcome::Run(keys::presses(self.bindings[index])),
                    // An over-narrow query is not a reason to lose it.
                    None => Outcome::Stay,
                };
            }
            k if keys::ACTION_MENU_NEXT.matches(k) => self.list.next(&self.rows),
            k if keys::ACTION_MENU_PREV.matches(k) => self.list.prev(&self.rows),
            k if keys::ACTION_MENU_PAGE_DOWN.matches(k) => self.list.page_down(&self.rows),
            k if keys::ACTION_MENU_PAGE_UP.matches(k) => self.list.page_up(&self.rows),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    self.list.edit(edit);
                }
            }
        }
        Outcome::Stay
    }
}

/// `description (keys)`, with a narrowing note after the keys when the binding
/// has one.
fn row(binding: &Binding) -> String {
    let description = binding.action().description();
    let keys = binding.keys_label();
    match binding.note() {
        Some(note) => format!("{description} ({keys}; {note})"),
        None => format!("{description} ({keys})"),
    }
}

#[cfg(test)]
mod tests;
