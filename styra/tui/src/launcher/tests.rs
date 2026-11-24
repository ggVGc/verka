use super::*;
use styra_protocol::agent::{validate_selection, Effort};

#[test]
fn confirming_a_provider_conversion_records_the_choice_and_keeps_other_clients_history() {
    let root = std::env::temp_dir().join(format!("styra-picker-conversion-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let path = root.join("defaults.json");
    preferences::record_recent_model(&path, "claude-haiku-4-5-20251001").unwrap();
    let original = Selection::new(Provider::Claude);
    let chosen = Selection::new(Provider::Codex);
    let mut app = App::pending(original.clone());
    app.session_id = "stored-session".into();
    app.launcher = Some(Launcher::from_selection(&chosen, &[], false));
    confirm(&mut app, &path);
    assert_eq!(app.selection, original, "conversion is pending");
    assert_eq!(
        preferences::load_recent_models(&path),
        vec![chosen.model, "claude-haiku-4-5-20251001".into()]
    );
    std::fs::remove_dir_all(root).ok();
}

fn opened(selection: &str) -> Launcher {
    Launcher::from_selection(&Selection::parse(selection).unwrap(), &[], false)
}

fn typed(launcher: &mut Launcher, query: &str) {
    for character in query.chars() {
        launcher.type_query(Edit::Push(character));
    }
}

/// Why the list is of triples rather than of three columns: no row can name
/// a combination an agent would refuse, so there is no move to get wrong.
#[test]
fn every_row_is_a_launchable_selection() {
    let mut launcher = opened("claude");
    for row in 0..launcher.labels().len() {
        launcher.list.selected = row;
        let selection = launcher.selection();
        validate_selection(&selection)
            .unwrap_or_else(|error| panic!("{} is not launchable: {error:#}", selection.name()));
    }
}

#[test]
fn separate_lists_offer_each_model_and_its_supported_efforts() {
    for provider in PROVIDERS {
        let selection = Selection::new(provider);
        let mut picker = Launcher::for_choice(&selection, Choice::ModelEffort, false);
        assert_eq!(picker.selection(), selection);
        assert_eq!(picker.labels().len(), provider.models().len());
        for (index, model) in provider.models().iter().enumerate() {
            picker.list.selected = index;
            picker.sync_effort(selection.effort);
            assert_eq!(picker.selection().model, model.id);
            assert_eq!(picker.efforts(), model.efforts);
            for effort_index in 0..model.efforts.len().max(1) {
                picker.effort_list.selected = effort_index;
                validate_selection(&picker.selection()).unwrap();
            }
        }
    }
}

#[test]
fn changing_model_preserves_effort_when_supported_and_adapts_shorter_ladders() {
    for provider in PROVIDERS {
        let mut selection = Selection::new(provider);
        selection.effort = *provider.efforts().last().unwrap();
        let mut picker = Launcher::for_choice(&selection, Choice::ModelEffort, false);
        for index in 0..picker.rows.len() {
            picker.list.selected = index;
            picker.sync_effort(selection.effort);
            let chosen = picker.selection();
            let efforts = provider.efforts_for(&chosen.model);
            if efforts.is_empty() || efforts.contains(&selection.effort) {
                assert_eq!(chosen.effort, selection.effort);
            } else {
                assert_eq!(chosen.effort, provider.default_effort_for(&chosen.model));
            }
            validate_selection(&chosen).unwrap();
        }
    }
}

#[test]
fn cycling_targets_separate_lists_and_filters_until_selecting_applies_both() {
    use crate::keybindings::press;
    use crossterm::event::KeyCode;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("defaults.json");
    let original = Selection::new(Provider::Codex);
    let mut app = App::pending(original.clone());
    app.open_model_picker();
    let model_query = original.model.clone();
    for ch in model_query.chars() {
        handle_key(&mut app, KeyCode::Char(ch).into(), &path);
    }
    let model = app.launcher.as_ref().unwrap().selection().model;
    handle_key(&mut app, press(&keys::EDITOR_CYCLE_EFFORT), &path);
    assert_eq!(app.launcher.as_ref().unwrap().selection().model, model);
    for ch in "high".chars() {
        handle_key(&mut app, KeyCode::Char(ch).into(), &path);
    }
    let picker = app.launcher.as_ref().unwrap();
    assert_eq!(picker.active, Field::Effort);
    assert_eq!(picker.list.query, model_query);
    assert_eq!(picker.effort_list.query, "high");
    let effort = picker.selection().effort;
    handle_key(&mut app, press(&keys::EDITOR_CYCLE_MODEL), &path);
    handle_key(&mut app, KeyCode::Backspace.into(), &path);
    let picker = app.launcher.as_ref().unwrap();
    assert_eq!(picker.active, Field::Model);
    assert_eq!(picker.list.query, &model_query[..model_query.len() - 1]);
    assert_eq!(picker.effort_list.query, "high");
    assert_eq!(picker.selection().effort, effort);
    assert_eq!(app.selection, original);
    assert!(app.take_request().is_none());
    handle_key(&mut app, press(&keys::EDITOR_CYCLE_EFFORT), &path);
    let chosen = app.launcher.as_ref().unwrap().selection();
    handle_key(&mut app, press(&keys::LAUNCHER_SELECT), &path);
    assert_eq!(app.selection, chosen);
    assert!(app.launcher.is_none());
}

#[test]
fn focused_pickers_keep_custom_models_and_aliases_on_open() {
    for model in ["custom-model", "opus"] {
        let selection = Selection::parse(&format!("claude:{model}/high")).unwrap();
        for choice in [Choice::Provider, Choice::ModelEffort] {
            let picker = Launcher::for_choice(&selection, choice, false);
            assert_eq!(picker.selection(), selection);
            let labels = picker.labels();
            let mut unique = labels.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), labels.len());
        }
    }
}

/// Both agents' catalogs are on offer at once — that is what a flat list
/// buys — and each model's own ladder is under it.
#[test]
fn the_list_offers_every_agents_models_and_their_own_rungs() {
    let launcher = opened("claude");
    let labels = launcher.labels();
    for provider in PROVIDERS {
        for model in provider.models() {
            assert!(
                labels.iter().any(|label| label.contains(model.id)),
                "{} is not on offer: {labels:?}",
                model.id
            );
        }
    }
    // A model with a ladder gets one row per rung, and one with no ladder
    // gets a single row that does not advertise a placeholder.
    assert_eq!(
        labels
            .iter()
            .filter(|label| label.contains("claude-haiku-4-5-20251001"))
            .collect::<Vec<_>>(),
        vec!["claude:claude-haiku-4-5-20251001"]
    );
}

#[test]
fn the_picker_opens_on_the_selection_it_was_given() {
    for name in [
        "claude:claude-opus-5/max",
        "codex:gpt-5.6-sol/minimal",
        "claude:claude-haiku-4-5-20251001",
    ] {
        let launcher = opened(name);
        assert_eq!(label(&launcher.selection()), name);
    }
}

/// The point of the thing: a few letters of a triple's name reach it,
/// without the separators between them being typed.
#[test]
fn typing_narrows_the_list_to_the_triple_named() {
    let mut launcher = opened("claude");
    typed(&mut launcher, "chk45");
    let selection = launcher.selection();
    assert_eq!(selection.provider, Provider::Claude);
    assert_eq!(selection.model, "claude-haiku-4-5-20251001");

    // Down to the rung: the effort is part of what is being typed at.
    let mut launcher = opened("claude");
    typed(&mut launcher, "opus-5-5/max");
    assert_eq!(label(&launcher.selection()), "claude:claude-opus-5-5/max");
}

/// Typing narrows the list without reshuffling it: a query several models
/// match leaves the most recently selected of them on top, even where
/// another would rank as the closer match.
#[test]
fn recency_order_holds_while_filtering() {
    let recent = vec!["claude-sonnet-5".to_owned(), "claude-opus-5".to_owned()];
    let mut launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &recent, false);
    typed(&mut launcher, "co5");
    assert_eq!(launcher.selection().model, "claude-sonnet-5");

    let matched: Vec<String> = launcher
        .list
        .matches(&launcher.labels())
        .into_iter()
        .map(|found| launcher.labels()[found.index].clone())
        .collect();
    let opus = matched
        .iter()
        .position(|label| label.contains(":claude-opus-5/"))
        .expect("the older recent model still matches");
    assert!(
        matched[..opus]
            .iter()
            .all(|label| label.contains(":claude-sonnet-5/")),
        "only the most recent model's rows come before the next: {matched:?}"
    );
}

/// A query can cross the agent boundary, which is the whole point of one
/// list: reaching another agent's model no longer means selecting the agent
/// first.
#[test]
fn a_query_reaches_another_agents_model_directly() {
    let mut launcher = opened("claude:claude-opus-5/max");
    assert_eq!(launcher.selection().provider, Provider::Claude);
    typed(&mut launcher, "codex:");
    assert_eq!(launcher.selection().provider, Provider::Codex);
}

/// The models the operator actually uses lead the list; the rest keep the
/// catalogs' order, and a model's rungs stay together beneath it.
#[test]
fn recently_selected_models_lead_the_list() {
    let catalog: Vec<&str> = Provider::Claude
        .models()
        .iter()
        .map(|model| model.id)
        .collect();
    let recent = vec![catalog[catalog.len() - 1].to_owned(), catalog[1].to_owned()];
    let launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &recent, false);

    let models: Vec<String> = launcher
        .labels()
        .iter()
        .map(|label| {
            label
                .split(':')
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .expect("a labelled model")
                .to_owned()
        })
        .collect();
    assert_eq!(
        models.first().map(String::as_str),
        Some(catalog[catalog.len() - 1])
    );
    // Every rung of the most recent model comes before the next model's.
    let first_run = models
        .iter()
        .take_while(|model| *model == catalog[catalog.len() - 1])
        .count();
    assert_eq!(
        models[first_run], catalog[1],
        "the second most recent model follows the first's rungs"
    );
    // Ordering the rows does not change which one the picker opened on.
    assert_eq!(launcher.selection().model, Provider::Claude.default_model());
}

/// A live session's agent is the process itself, so while one is up the
/// list offers that agent's triples and no others — no query can reach a
/// row the session could not adopt.
#[test]
fn a_locked_agent_leaves_only_its_own_rows() {
    let mut launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &[], true);
    for label in launcher.labels() {
        assert!(label.starts_with("claude:"), "{label}");
    }
    // Even typing another agent's name reaches nothing.
    typed(&mut launcher, "codex");
    assert!(launcher.list.matches(&launcher.labels()).is_empty());
    assert_eq!(launcher.selection().provider, Provider::Claude);
}

/// A selection the catalogs no longer offer is still what the session was
/// launched with, so the picker carries it as a row instead of quietly
/// relaunching on something else.
#[test]
fn a_selection_outside_the_catalogs_is_carried_as_its_own_row() {
    let carried = Selection::parse("claude:claude-opus-4-1-20250805").unwrap();
    let launcher = Launcher::from_selection(&carried, &[], false);
    assert_eq!(launcher.selection(), carried);
    assert_eq!(
        launcher
            .labels()
            .iter()
            .filter(|label| label.contains("claude-opus-4-1-20250805"))
            .count(),
        1
    );
}

/// A query matching nothing leaves no row under the cursor. Confirming then
/// cannot invent one, so what the picker opened on is what stands.
#[test]
fn an_over_narrow_query_leaves_the_opening_selection_standing() {
    let mut launcher = opened("claude:claude-opus-5/max");
    typed(&mut launcher, "zzzz");
    assert!(launcher.list.matches(&launcher.labels()).is_empty());
    // And moving within nothing is a no-op rather than an arithmetic fault.
    launcher.next();
    launcher.prev();
    assert_eq!(label(&launcher.selection()), "claude:claude-opus-5/max");
}

/// Widening the list back out keeps the row the query had found, so
/// clearing a query is not also losing the choice it reached.
#[test]
fn clearing_the_query_keeps_the_row_it_reached() {
    let mut launcher = opened("claude");
    typed(&mut launcher, "haiku");
    let found = launcher.selection();

    launcher.clear_query();
    assert!(!launcher.is_filtering());
    assert_eq!(
        launcher.list.matches(&launcher.labels()).len(),
        launcher.labels().len()
    );
    assert_eq!(launcher.selection(), found);
}

/// A mistyped rung costs the rung, not the agent and model in front of
/// it.
#[test]
fn deleting_a_word_takes_back_one_part_of_the_triple() {
    let mut launcher = opened("claude");
    typed(&mut launcher, "claude:claude-opus-5-5/mox");
    assert!(launcher.list.matches(&launcher.labels()).is_empty());

    launcher.type_query(Edit::DeleteWord);
    assert_eq!(launcher.list.query, "claude:claude-opus-5-5/");
    assert_eq!(launcher.selection().model, "claude-opus-5-5");
}

/// The list is long enough to page through, and a page lands on a row
/// rather than running off the end of it.
#[test]
fn paging_crosses_the_list_and_stops_at_its_ends() {
    let mut launcher = opened("claude");
    assert!(
        launcher.labels().len() > styra_ui::fuzzy_list::PAGE,
        "the list is worth paging"
    );

    let opened_at = launcher.list.selected;
    launcher.page_down();
    assert_eq!(
        launcher.list.selected,
        opened_at + styra_ui::fuzzy_list::PAGE
    );
    for _ in 0..launcher.labels().len() {
        launcher.page_down();
    }
    assert_eq!(launcher.list.selected, launcher.labels().len() - 1);
    validate_selection(&launcher.selection()).expect("still a launchable row");

    for _ in 0..launcher.labels().len() {
        launcher.page_up();
    }
    assert_eq!(launcher.list.selected, 0);
}

/// The cursor holds at the ends of the list rather than wrapping round
/// them: the top is where the best match of a query sits, and stepping off
/// the bottom must not land on it.
#[test]
fn row_navigation_holds_at_both_ends() {
    let mut launcher = opened("codex");
    let rows = launcher.labels().len();

    launcher.list.selected = rows - 1;
    launcher.next();
    assert_eq!(launcher.list.selected, rows - 1);

    launcher.list.selected = 0;
    launcher.prev();
    assert_eq!(launcher.list.selected, 0);
}

/// The rung is part of the row, so a model without `xhigh` simply has no
/// `xhigh` row — there is no cursor left that could point at one.
#[test]
fn a_model_only_offers_the_rungs_it_has() {
    let launcher = opened("claude");
    let labels = launcher.labels();
    assert!(labels.contains(&"claude:claude-opus-5/xhigh".to_owned()));
    assert!(
        !labels.contains(&"claude:claude-opus-4-6/xhigh".to_owned()),
        "that rung arrived after 4.6"
    );
    assert!(labels.contains(&"claude:claude-opus-4-6/high".to_owned()));
}

#[test]
fn every_models_efforts_are_offered_in_ladder_order() {
    let launcher = opened("claude");
    for provider in PROVIDERS {
        for model in provider.models() {
            let rungs: Vec<Effort> = launcher
                .rows
                .iter()
                .filter(|row| row.provider == provider && row.model == model.id)
                .map(|row| row.effort)
                .collect();
            let expected = if model.efforts.is_empty() {
                vec![provider.default_effort_for(model.id)]
            } else {
                model.efforts.to_vec()
            };
            assert_eq!(rungs, expected, "{}:{}", provider.as_str(), model.id);
        }
    }
}
