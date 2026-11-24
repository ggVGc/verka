use super::*;

#[test]
fn only_a_file_link_offers_to_be_mounted() {
    assert_eq!(
        LinkMenu::new("src/lib.rs".into(), false).labels(),
        [
            "open in editor",
            "mount read-only for this interaction",
            "mount read-write for this interaction",
        ]
    );
    assert_eq!(
        LinkMenu::new("https://example.com".into(), true).labels(),
        ["open in browser"]
    );
}

#[test]
fn selection_wraps_within_the_actions() {
    let mut menu = LinkMenu::new("src/lib.rs".into(), false);
    menu.select_previous();
    assert_eq!(menu.selected(), LinkAction::Mount { writable: true });
    menu.select_next();
    assert_eq!(menu.selected(), LinkAction::Open);
}
