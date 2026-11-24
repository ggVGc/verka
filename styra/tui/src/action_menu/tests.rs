use super::*;
use crossterm::event::KeyCode;

fn typed(menu: &mut ActionMenu, text: &str) {
    for character in text.chars() {
        assert_eq!(
            menu.handle_key(KeyEvent::from(KeyCode::Char(character))),
            Outcome::Stay
        );
    }
}

/// The key text comes from the configuration, so only where it goes is
/// asserted here.
#[test]
fn each_action_shows_its_shortcut_in_parentheses() {
    let menu = ActionMenu::new(Window::Events);
    let shows = |row: String| menu.rows().contains(&row);
    assert!(shows(format!("quit ({})", keys::GLOBAL_QUIT.keys_label())));
    assert!(shows(format!(
        "expand all ({})",
        keys::EVENTS_EXPAND_ALL.keys_label()
    )));
    let narrowed = keys::EVENTS_PREVIEW_TARGET;
    assert!(
        shows(format!(
            "preview the newest command ({}; {})",
            narrowed.keys_label(),
            narrowed.note().unwrap()
        )),
        "a narrowed binding says where it applies"
    );
}

/// Choosing an action answers with the keys that run it, for the event
/// loop to press.
#[test]
fn a_typed_query_finds_an_action_and_running_it_answers_its_keys() {
    let mut menu = ActionMenu::new(Window::Events);
    typed(&mut menu, "transcript view");
    assert_eq!(
        menu.handle_key(keys::press(&keys::ACTION_MENU_RUN)),
        Outcome::Run(keys::presses(&keys::GLOBAL_TRANSCRIPT))
    );
}

/// Letters are the query, so the keys they are bound to elsewhere do not
/// act while the menu is open.
#[test]
fn letters_narrow_rather_than_act() {
    let mut menu = ActionMenu::new(Window::Events);
    typed(&mut menu, "q");
    assert_eq!(menu.list.query, "q");
    assert_eq!(
        menu.handle_key(KeyEvent::from(KeyCode::Backspace)),
        Outcome::Stay
    );
    assert!(!menu.list.is_filtering());
}

#[test]
fn nothing_runs_when_nothing_matches() {
    let mut menu = ActionMenu::new(Window::Events);
    typed(&mut menu, "zzzzzz");
    assert_eq!(
        menu.handle_key(keys::press(&keys::ACTION_MENU_RUN)),
        Outcome::Stay
    );
    assert_eq!(
        menu.handle_key(keys::press(&keys::ACTION_MENU_CANCEL)),
        Outcome::Cancel
    );
}

#[test]
fn moving_down_selects_the_next_action() {
    let mut menu = ActionMenu::new(Window::Overview);
    menu.handle_key(keys::press(&keys::ACTION_MENU_NEXT));
    assert_eq!(
        menu.handle_key(keys::press(&keys::ACTION_MENU_RUN)),
        Outcome::Run(keys::presses(Window::Overview.actions()[1])),
        "the second of the overview's actions"
    );
}
