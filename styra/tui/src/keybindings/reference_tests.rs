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
        let has = |description: &str| {
            rows.iter().any(|row| {
                matches!(
                    row,
                    ReferenceRow::Binding(binding)
                        if binding.press().is_some()
                            && binding.action().description() == description
                )
            })
        };
        assert!(
            has("show/close this reference"),
            "{} says how to reopen the reference",
            window.name()
        );
        assert!(has("cancel"), "{} says how to leave", window.name());
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
    const ONE: Binding = Binding::new(
        &[Key::ch('J')],
        None,
        None,
        Windows::All,
        Action::EventsScrollDown,
    );
    const SEVERAL: Binding = Binding::new(
        &[Key::ch('o'), Key::code(KeyCode::Esc)],
        None,
        None,
        Windows::All,
        Action::OverviewClose,
    );
    const MODIFIED: Binding = Binding::new(
        &[Key::alt_code(KeyCode::Enter), Key::ctrl('w')],
        None,
        None,
        Windows::All,
        Action::EditorNewline,
    );
    const NARROWED: Binding = Binding::new(
        &[Key::ctrl('w')],
        None,
        Some("existing session"),
        Windows::All,
        Action::GlobalSessionWorktree,
    );
    const CHORD: Binding = Binding::new(
        &[Key::ch('R')],
        Some("z R"),
        None,
        Windows::All,
        Action::EventsExpandAll,
    );
    assert_eq!(ONE.label(), "J");
    assert_eq!(SEVERAL.label(), "o/Esc");
    assert_eq!(MODIFIED.label(), "alt-Enter/ctrl-w");
    assert_eq!(
        NARROWED.label(),
        "ctrl-w (existing session)",
        "a narrower binding says where it applies"
    );
    assert_eq!(NARROWED.keys_label(), "ctrl-w");
    assert_eq!(CHORD.label(), "z R", "a chord names its prefix");
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
    assert!(
        EVENTS.iter().any(
            |row| matches!(row, ReferenceRow::Binding(binding) if binding.action() == actions[0])
        ),
        "the window's own commands come first"
    );
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
/// menu would press a key that does something else. Where neither holds
/// for one of its keys, it is offered on that key. Which keys collide is
/// the configuration's business, so this holds for whatever it says.
#[test]
fn a_window_offers_only_the_commands_its_keys_reach() {
    for window in WINDOWS {
        let sections = window.sections();
        let has_global = sections
            .iter()
            .any(|section| section.first() == GLOBAL.first());
        let own = sections.first().copied().unwrap_or_default();
        if !has_global || own.first() == GLOBAL.first() {
            continue;
        }
        // Only a plain binding of the window's own holds its key all the
        // time: one narrowed by a note, or the second key of a chord, does
        // not.
        let taken = |key: KeyEvent| {
            own.iter().any(|row| {
                matches!(row, ReferenceRow::Binding(binding)
                        if binding.note().is_none()
                            && !is_chord(binding)
                            && binding.matches_in(key, window))
            })
        };
        let listed = window.reference();
        let offered = window.actions();
        for binding in offered.iter() {
            assert!(
                listed.contains(&ReferenceRow::Binding(binding)),
                "{} lists what it offers",
                window.name()
            );
        }
        for row in GLOBAL {
            let ReferenceRow::Binding(binding) = row else {
                continue;
            };
            let Some(key) = binding.press() else {
                continue;
            };
            let reaches = binding.applies_in(window) && !taken(key);
            assert_eq!(
                listed.contains(row),
                reaches,
                "{} lists {:?} on {} only where its key reaches it",
                window.name(),
                binding.action(),
                binding.keys_label()
            );
            if !reaches {
                assert!(
                    !offered.iter().any(|seen| *seen == *binding),
                    "{} does not offer {:?} on {}",
                    window.name(),
                    binding.action(),
                    binding.keys_label()
                );
            }
        }
    }
}

/// A chord is run by pressing its prefix first, as the operator would; any
/// other command by its own key alone.
#[test]
fn a_chord_is_run_with_its_prefix() {
    assert_eq!(
        presses(&EVENTS_EXPAND_ALL),
        [press(&EVENTS_FOLD_PREFIX), press(&EVENTS_EXPAND_ALL)]
    );
    assert_eq!(presses(&GLOBAL_LOG), [press(&GLOBAL_LOG)]);
}

const PLAIN: Binding = Binding::new(
    &[Key::ch('l')],
    None,
    None,
    Windows::All,
    Action::GlobalLauncher,
);
const CONTROL: Binding = Binding::new(
    &[Key::ctrl('l')],
    None,
    None,
    Windows::All,
    Action::GlobalLog,
);

/// Control is part of the binding, so ctrl-l is not plain `l`.
#[test]
fn modifiers_are_part_of_the_match() {
    let plain = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
    let control = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);

    assert!(PLAIN.matches(plain));
    assert!(!PLAIN.matches(control));
    assert!(CONTROL.matches(control));
    assert!(!CONTROL.matches(plain));
}

/// Shift is not: a capital already arrives as its own character, and
/// terminals disagree about reporting the modifier for punctuation.
#[test]
fn shift_is_ignored_so_capitals_and_punctuation_still_match() {
    const CAPITAL: Binding = Binding::new(
        &[Key::ch('N')],
        None,
        None,
        Windows::All,
        Action::GlobalNextWorking,
    );
    const PUNCTUATION: Binding = Binding::new(
        &[Key::ch('~')],
        None,
        None,
        Windows::All,
        Action::GlobalDirectory,
    );
    assert!(CAPITAL.matches(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)));
    assert!(PUNCTUATION.matches(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::SHIFT)));
}
