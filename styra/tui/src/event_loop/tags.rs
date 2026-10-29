//! The tag picker: editing an interaction's tags, or choosing the
//! navigator's tag filter.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::keybindings as keys;
use crate::tag_picker::TagPurpose;
use styra_server::Client;
use styra_ui::query::Edit;

/// Answer a key while the tag picker is open. Tags are edited as one modal
/// operation, so typing a new tag cannot trigger the navigator or any global
/// shortcut underneath it.
pub(super) fn handle_tag_picker_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    if picker.new_tag.is_some() {
        match key.code {
            KeyCode::Esc => picker.new_tag = None,
            KeyCode::Enter => {
                picker.add_new();
                let tags = picker.selected.clone();
                let id = app.session_id.clone();
                save_tags(app, client, id, tags);
            }
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    edit.apply(picker.new_tag.as_mut().expect("new tag is present"));
                }
            }
        }
        return;
    }
    match key {
        k if keys::TAGS_NEXT.matches(k) => picker.next(),
        k if keys::TAGS_PREV.matches(k) => picker.previous(),
        k if keys::TAGS_PAGE_DOWN.matches(k) => picker.page_down(),
        k if keys::TAGS_PAGE_UP.matches(k) => picker.page_up(),
        k if keys::TAGS_TOGGLE.matches(k) => toggle_tag_picker_selection(app),
        k if keys::TAGS_EXCLUDE.matches(k) && picker.purpose == TagPurpose::Filter => {
            toggle_tag_picker_exclusion(app)
        }
        k if keys::TAGS_CLEAR.matches(k) => clear_tag_picker_selection(app),
        k if keys::TAGS_NEW.matches(k) => picker.start_new(),
        k if keys::TAGS_CANCEL.matches(k) => {
            restore_tag_picker_selection(app);
            app.tag_picker = None;
        }
        k if keys::TAGS_SAVE.matches(k) => {
            let tags = picker.selected.clone();
            let id = app.session_id.clone();
            match picker.purpose {
                TagPurpose::Edit => save_tags(app, client, id, tags),
                TagPurpose::Filter => {
                    app.tag_picker = None;
                }
            }
        }
        _ => {
            if let Some(edit) = Edit::from_key(key) {
                picker.type_query(edit);
            }
        }
    }
}

/// Store a completed tag edit and immediately refresh the row the navigator
/// renders. A new tag is a completed edit in its own right: requiring a
/// second Enter after the text field made it look as though adding failed.
fn save_tags(app: &mut App, client: &Client, id: String, tags: Vec<String>) {
    app.tag_picker = None;
    match client.set_session_tags(&id, tags) {
        Ok(summary) => {
            if let Some(item) = app
                .interactions
                .items
                .iter_mut()
                .find(|item| item.id == summary.id)
            {
                item.tags = summary.tags;
            }
            app.show_action_message("interaction tags saved");
        }
        Err(error) => app.show_error(format!("could not save interaction tags: {error:#}")),
    }
}

fn apply_tag_picker_filter(app: &mut App) {
    let tags = {
        let Some(picker) = app.tag_picker.as_ref() else {
            return;
        };
        (picker.purpose == TagPurpose::Filter).then(|| picker.selected.clone())
    };
    let Some(tags) = tags else {
        return;
    };
    let id = app.session_id.clone();
    let workspace_id = app.workspace.id.clone();
    app.interactions
        .set_tag_filter(tags, &id, workspace_id.as_deref());
}

/// Toggle the highlighted tag and, when the picker is choosing a navigator
/// filter, apply the new selection immediately. The filter picker is drawn on
/// top of the navigator, so updating its state here lets the rows behind it
/// react to every Space press instead of waiting for Enter.
fn toggle_tag_picker_selection(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.toggle();
    apply_tag_picker_filter(app);
}

fn toggle_tag_picker_exclusion(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.toggle_excluded();
    apply_tag_picker_filter(app);
}

fn clear_tag_picker_selection(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.selected.clear();
    apply_tag_picker_filter(app);
}

fn restore_tag_picker_selection(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.restore_selection();
    apply_tag_picker_filter(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::navigation::cursored_interaction_is_loadable;
    use crate::tag_picker::TagPicker;
    use std::time::Duration;
    use styra_protocol::InteractionActivity;

    #[test]
    fn toggling_a_tag_filter_applies_it_immediately() {
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
        app.session_id = "one".into();
        let current = crate::interactions::tests::interaction("one", InteractionActivity::Pending);
        let mut matching =
            crate::interactions::tests::interaction("two", InteractionActivity::Pending);
        matching.tags = vec!["bug".into()];
        app.interactions.open(vec![current, matching], Vec::new());
        app.tag_picker = Some(TagPicker::filter(vec!["bug".into()], Vec::new()));

        toggle_tag_picker_selection(&mut app);
        assert_eq!(app.interactions.tag_filter(), ["bug"]);
        assert!(app.tag_picker.is_some());
        assert!(app.interactions.pending("one").is_some());
        std::thread::sleep(Duration::from_millis(140));
        assert!(!cursored_interaction_is_loadable(&app));

        toggle_tag_picker_selection(&mut app);
        assert!(app.interactions.tag_filter().is_empty());
        assert!(app.tag_picker.is_some());

        toggle_tag_picker_selection(&mut app);
        clear_tag_picker_selection(&mut app);
        assert!(app.interactions.tag_filter().is_empty());
        assert!(app
            .tag_picker
            .as_ref()
            .is_some_and(|picker| picker.selected.is_empty()));

        app.tag_picker = Some(TagPicker::filter(vec!["bug".into()], vec!["bug".into()]));
        clear_tag_picker_selection(&mut app);
        restore_tag_picker_selection(&mut app);
        assert_eq!(app.interactions.tag_filter(), ["bug"]);
        assert_eq!(app.tag_picker.as_ref().unwrap().selected, ["bug"]);

        app.tag_picker = None;
        assert!(cursored_interaction_is_loadable(&app));
    }

    #[test]
    fn excluding_a_tag_filter_applies_it_immediately() {
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
        app.session_id = "one".into();
        let current = crate::interactions::tests::interaction("one", InteractionActivity::Pending);
        let mut excluded =
            crate::interactions::tests::interaction("two", InteractionActivity::Pending);
        excluded.tags = vec!["bug".into()];
        app.interactions.open(vec![current, excluded], Vec::new());
        app.tag_picker = Some(TagPicker::filter(vec!["bug".into()], Vec::new()));

        toggle_tag_picker_exclusion(&mut app);
        assert_eq!(app.interactions.tag_filter(), ["-bug"]);
        assert_eq!(app.interactions.visible_indices("", None), vec![0]);

        toggle_tag_picker_exclusion(&mut app);
        assert!(app.interactions.tag_filter().is_empty());
        assert_eq!(app.interactions.visible_indices("", None), vec![0, 1]);
    }
}
