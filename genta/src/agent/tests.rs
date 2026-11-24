use super::testing::{agent_bin, builtin};
use super::*;

/// The whole point of resolving on the host: the profile carries a path the
/// sandbox can exec, not a name that only the operator's own `PATH` finds.
#[test]
fn a_profile_launches_the_resolved_executable_path_not_a_bare_name() {
    for name in ["codex", "claude", "claude:opus"] {
        let profile = builtin(name, &SandboxLayout::default()).unwrap();
        let command = Path::new(&profile.command[0]);
        assert!(
            command.is_absolute(),
            "{name} must launch an absolute path, got {}",
            profile.command[0]
        );
        assert_eq!(command.parent(), Some(agent_bin().as_path()));
    }
}

/// A missing agent is a clear error at profile construction, not an opaque
/// `execvp: No such file or directory` from inside the sandbox.
#[test]
fn a_missing_agent_is_reported_before_any_isolation_is_built() {
    let error = Profile::builtin_on_path("claude", &SandboxLayout::default(), OsStr::new(""))
        .expect_err("an agent that is not installed cannot be launched");
    let message = format!("{error:#}");
    assert!(message.contains("claude"), "{message}");
    assert!(message.contains("not found on PATH"), "{message}");
}

#[test]
fn unknown_profile_is_rejected() {
    assert!(builtin("gpt5", &SandboxLayout::default()).is_err());
}

#[test]
fn empty_model_suffix_is_rejected() {
    assert!(builtin("claude:", &SandboxLayout::default()).is_err());
}

#[test]
fn session_meta_captures_the_selection_and_survives_json_round_trip() {
    let selection = Selection {
        provider: Provider::Claude,
        model: "opus".into(),
        effort: Effort::High,
    };
    let meta = SessionMeta::new(selection.clone(), Protocol::ClaudeJsonl);
    assert_eq!(meta.selection, selection);
    assert_eq!(meta.protocol, Protocol::ClaudeJsonl);

    let json = serde_json::to_string(&meta).unwrap();
    let restored: SessionMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, meta);
}

/// A selection is the profile name: what a picker composes, what a journal
/// records, and what re-parses into the same launch. A fully-named profile
/// round-trips unchanged.
#[test]
fn a_selection_round_trips_through_its_profile_name() {
    for (name, expected) in [
        (
            "codex:gpt-5.6-sol/xhigh",
            Selection {
                provider: Provider::Codex,
                model: "gpt-5.6-sol".into(),
                effort: Effort::XHigh,
            },
        ),
        (
            "claude:claude-opus-5/max",
            Selection {
                provider: Provider::Claude,
                model: "claude-opus-5".into(),
                effort: Effort::Max,
            },
        ),
        (
            "codex:gpt-5.6-luna/minimal",
            Selection {
                provider: Provider::Codex,
                model: "gpt-5.6-luna".into(),
                effort: Effort::Minimal,
            },
        ),
    ] {
        let parsed = Selection::parse(name).unwrap();
        assert_eq!(parsed, expected, "parsing {name:?}");
        assert_eq!(parsed.name(), name, "round-tripping {name:?}");
    }
}

/// A model id with a `/` in it: the effort is what follows the last one,
/// and a catalog model named alone takes the default effort rather than
/// having its own tail read as one. An unlisted id with a `/` is
/// launchable too, so long as its effort is named.
#[test]
fn a_model_id_may_contain_the_effort_separator() {
    let deepseek = "deepseek/deepseek-v4.1-flash";
    let pinned = Selection::parse(&format!("codex:{deepseek}/high")).unwrap();
    assert_eq!(
        (pinned.model.as_str(), pinned.effort),
        (deepseek, Effort::High)
    );
    assert_eq!(pinned.name(), format!("codex:{deepseek}/high"));

    let bare = Selection::parse(&format!("codex:{deepseek}")).unwrap();
    assert_eq!(bare.model, deepseek);
    assert_eq!(bare.effort, Provider::Codex.default_effort());

    let unlisted = Selection::parse("codex:vendor/new-model/low").unwrap();
    assert_eq!(
        (unlisted.model.as_str(), unlisted.effort),
        ("vendor/new-model", Effort::Low)
    );
    // Without the catalog to vouch for it, an unlisted id's tail is an
    // effort, and a misspelt one is still refused by name.
    let error = Selection::parse("codex:vendor/new-model").unwrap_err();
    assert!(
        error.to_string().contains("unknown reasoning effort"),
        "{error}"
    );
}

/// A selection may not leave the model or effort unpinned: a shorter profile
/// name takes the provider's declared defaults, and then names itself in
/// full, so what a journal records is never "whatever the agent was set to".
#[test]
fn a_shorter_profile_name_takes_the_declared_defaults() {
    for provider in Provider::ALL {
        let name = provider.as_str();
        let (model, effort) = (provider.default_model(), provider.default_effort());

        // The bare name fills out to both declared defaults, and filling in
        // a default is idempotent — the full name re-parses to it.
        let parsed = Selection::parse(name).unwrap();
        let full = format!("{name}:{model}/{}", effort.as_str());
        assert_eq!(parsed.name(), full, "{name:?} should fill out to {full:?}");
        assert_eq!(Selection::parse(&full).unwrap(), parsed);

        // Whichever half is given is kept; only the missing half defaults.
        let cheapest = provider.cheapest_model();
        assert_eq!(
            Selection::parse(&format!("{name}:{cheapest}"))
                .unwrap()
                .name(),
            format!("{name}:{cheapest}/{}", effort.as_str())
        );
        let cheapest_effort = provider.cheapest_effort().as_str();
        assert_eq!(
            Selection::parse(&format!("{name}/{cheapest_effort}"))
                .unwrap()
                .name(),
            format!("{name}:{model}/{cheapest_effort}")
        );

        // `Selection::new` is the same defaults by another route.
        let selection = Selection::new(provider);
        assert_eq!(selection.model, model);
        assert_eq!(selection.effort, effort);
        assert_eq!(parsed, selection);
    }
}

/// Each declared default must be launchable as a model id and effort.
#[test]
fn the_declared_defaults_are_offered_by_their_provider() {
    for provider in Provider::ALL {
        let model = provider.default_model();
        assert!(
            provider.model(model).is_some(),
            "{provider:?} default model is outside its own catalog"
        );
        assert!(
            provider
                .efforts_for(model)
                .contains(&provider.default_effort()),
            "{provider:?} cannot run its own default effort on its default model"
        );
    }
}

/// The errand tier is launchable too, and is never the tier a session
/// itself would get — an errand that cost what the session does would not
/// be worth routing away from it.
#[test]
fn the_cheapest_model_is_offered_by_its_provider_and_is_not_the_default() {
    for provider in Provider::ALL {
        let model = provider.cheapest_model();
        assert!(
            provider.model(model).is_some(),
            "{provider:?} cheapest model is outside its own catalog"
        );
        // Every launch pins an effort, so a model that rejects one cannot
        // run an errand.
        assert!(
            provider.supports_effort(model),
            "{provider:?} routes errands to a model that takes no effort"
        );
        assert!(
            provider
                .efforts_for(model)
                .contains(&provider.cheapest_effort()),
            "{provider:?} cannot run its own cheapest effort"
        );
        assert_ne!(
            provider.cheapest_model(),
            provider.default_model(),
            "{provider:?} routes errands to the model its sessions run on"
        );
    }
}

#[test]
fn an_unknown_provider_or_effort_is_rejected_by_name() {
    for name in [
        "gpt5",
        "codex-exec",
        "codex/turbo",
        "claude:opus/turbo",
        "codex:/high",
    ] {
        let error = Selection::parse(name)
            .expect_err("an unlaunchable selection must not parse")
            .to_string();
        assert!(
            error.contains("unknown") || error.contains("empty model"),
            "{name:?}: {error}"
        );
    }
}

/// The profile name a launch records must be the selection that produced
/// it, effort included, or a stored session cannot say what actually ran.
#[test]
fn a_resolved_profile_is_named_after_its_full_selection() {
    let layout = SandboxLayout::default();
    for name in [
        "codex:gpt-5.6-sol/high",
        "claude:claude-opus-5/max",
        "codex:gpt-5.6-terra/low",
    ] {
        let profile = builtin(name, &layout).unwrap();
        assert_eq!(profile.name, name);
    }
    // A shorter name resolves to a profile named after the defaults it took,
    // so `--profile claude` still records the model that ran.
    for short in ["codex", "claude"] {
        let selection = Selection::parse(short).unwrap();
        let profile = builtin(short, &layout).unwrap();
        assert_eq!(profile.name, selection.name());
        assert_ne!(profile.name, short);
    }
}

/// A conversation profile uses the provider's declared conversation protocol.
#[test]
fn a_built_profile_speaks_the_protocol_its_provider_declares() {
    for provider in Provider::ALL {
        let profile = builtin(provider.as_str(), &SandboxLayout::default()).unwrap();
        assert_eq!(
            profile.protocol,
            provider.protocol(),
            "{provider:?} builds a profile that speaks a protocol it does not declare"
        );
    }
}

#[test]
fn execution_mode_preserves_selection_and_isolation_but_selects_the_transport() {
    let layout = SandboxLayout::default();
    for provider in Provider::ALL {
        let selection = Selection::new(provider);
        let conversation = selection
            .resolve_on_path(&layout, agent_bin().as_os_str())
            .unwrap();
        let batch = selection
            .resolve_for_on_path(ExecutionMode::Batch, &layout, agent_bin().as_os_str())
            .unwrap();
        assert_eq!(batch.name, selection.name());
        assert_eq!(conversation.name, batch.name);
        assert_eq!(conversation.mounts, batch.mounts);
        assert_eq!(conversation.environment, batch.environment);
        assert_eq!(conversation.network, batch.network);
        assert!(!conversation.single_turn);
        assert!(batch.single_turn);
        match provider {
            Provider::Codex => {
                assert_eq!(conversation.protocol, Protocol::CodexAppServer);
                assert_eq!(batch.protocol, Protocol::CodexJsonl);
                assert_eq!(batch.encode_message("hello"), b"hello\n");
                assert!(batch.command.iter().any(|arg| arg == "exec"));
            }
            Provider::Claude => {
                assert_eq!(conversation.protocol, Protocol::ClaudeJsonl);
                assert_eq!(conversation.command, batch.command);
                let input: serde_json::Value =
                    serde_json::from_slice(&batch.encode_message("hello")).unwrap();
                assert_eq!(input["message"]["content"], "hello");
            }
        }
    }
}

/// Resuming is per-provider, but an unusable id is refused the same way for
/// all of them, before any provider-specific work.
#[test]
fn an_empty_provider_session_id_is_refused_by_every_provider() {
    for provider in Provider::ALL {
        let mut profile = builtin(provider.as_str(), &SandboxLayout::default()).unwrap();
        let error = profile
            .resume(provider, "  ")
            .expect_err("an empty id cannot name a conversation");
        assert!(format!("{error:#}").contains("empty provider session id"));
    }
}

/// Every catalog entry is launchable as written and stays within its
/// provider's widest ladder, so that ladder is a true fallback for an
/// unlisted id rather than a second, disagreeing statement.
#[test]
fn every_catalog_entry_is_launchable_and_within_the_widest_ladder() {
    for provider in Provider::ALL {
        assert!(!provider.models().is_empty());
        assert!(provider.efforts().is_sorted(), "{provider:?} ladder order");
        for entry in provider.models() {
            // A `Selection` round-trips through one string, so every id
            // must survive it — at every rung, and with the effort left
            // off — even one that carries the grammar's own `/`.
            for id in std::iter::once(&entry.id).chain(entry.aliases) {
                for effort in provider.efforts() {
                    let selection = Selection {
                        provider,
                        model: (*id).to_owned(),
                        effort: *effort,
                    };
                    assert_eq!(
                        Selection::parse(&selection.name()).unwrap(),
                        selection,
                        "{id} would not survive Selection::name"
                    );
                }
                let bare = format!("{}:{id}", provider.as_str());
                assert_eq!(Selection::parse(&bare).unwrap().model, *id, "{bare}");
            }
            assert!(entry.efforts.is_sorted(), "{} ladder order", entry.id);
            for effort in entry.efforts {
                assert!(
                    provider.efforts().contains(effort),
                    "{} offers {effort:?}, outside {provider:?}'s widest ladder",
                    entry.id
                );
            }
            // The default a picker opens on is one of the model's rungs.
            let default = provider.default_effort_for(entry.id);
            assert!(
                entry.efforts.is_empty() || entry.efforts.contains(&default),
                "{provider:?} {} opens on a rung it does not have",
                entry.id
            );
        }
        for effort in provider.efforts() {
            assert_eq!(Effort::parse(effort.as_str()).unwrap(), *effort);
        }
    }
}

/// The point of a per-model ladder: a rung one model has and another does
/// not is offered on the first and not the second, under the same
/// provider — and an alias is judged as the model it names.
#[test]
fn an_effort_ladder_belongs_to_the_model_not_the_agent() {
    let claude = Provider::Claude;
    assert!(claude.efforts_for("claude-opus-5").contains(&Effort::XHigh));
    assert!(!claude
        .efforts_for("claude-opus-4-6")
        .contains(&Effort::XHigh));
    assert_eq!(
        claude.efforts_for("claude-opus-4-5"),
        claude.efforts_for("claude-opus-4-5-20251101")
    );

    let codex = Provider::Codex;
    assert!(codex.efforts_for("gpt-5.6-sol").contains(&Effort::Max));
    assert!(!codex.efforts_for("gpt-5.5").contains(&Effort::Max));
    assert!(!codex.efforts_for("gpt-5.4").contains(&Effort::Max));
}

/// Genta's `minimal` rung is on no current codex model, so nothing may
/// launch on it — including the errand path, which takes the lowest rung.
#[test]
fn codex_does_not_offer_a_minimal_effort() {
    assert!(!Provider::Codex.efforts().contains(&Effort::Minimal));
    assert_eq!(Provider::Codex.cheapest_effort(), Effort::Low);
}

/// The two Claude models that predate the effort parameter are offered, but
/// take no effort — and so are never the errand model.
#[test]
fn the_models_without_an_effort_parameter_say_so() {
    for model in [
        "claude-sonnet-4-5-20250929",
        "claude-haiku-4-5-20251001",
        "claude-haiku-4-5",
    ] {
        assert!(!Provider::Claude.supports_effort(model), "{model}");
        assert_eq!(
            Provider::Claude.default_effort_for(model),
            Provider::Claude.default_effort(),
            "{model} still has a placeholder to put in a selection"
        );
    }
}

/// A shorter ladder that still has the declared default keeps it: the
/// default is only stepped down when the model lacks that rung.
#[test]
fn a_declared_default_stands_on_a_shorter_ladder_that_has_it() {
    assert_eq!(
        Provider::Codex.default_effort_for("gpt-5.5"),
        Provider::Codex.default_effort()
    );
    assert_eq!(
        Provider::Claude.default_effort_for("claude-opus-4-5-20251101"),
        Provider::Claude.default_effort()
    );
    assert_eq!(
        Provider::Claude.default_effort_for("claude-opus-5"),
        Provider::Claude.default_effort()
    );
}

/// An id newer than the catalog is under-constrained rather than refused:
/// the agent itself is the authority on its own catalog, and the id is
/// nobody's in particular.
#[test]
fn an_unknown_model_falls_back_to_the_widest_ladder() {
    assert_eq!(
        Provider::Claude.efforts_for("claude-opus-9"),
        Provider::Claude.efforts()
    );
    assert_eq!(
        Provider::Codex.efforts_for("gpt-7-nova"),
        Provider::Codex.efforts()
    );
    assert!(Provider::Claude.could_run("claude-opus-9"));
    assert!(!Provider::Claude.could_run("gpt-6.1-sol"));
    assert!(!Provider::Codex.could_run("claude-haiku-4-5"));
}
