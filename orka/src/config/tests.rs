use super::*;

#[test]
fn literal_command_maps_to_the_orka_layout() {
    let config: Config = toml::from_str(
        r#"
            [agent]
            command = ["agent", "--go"]

            [isolation]
            backend = "bwrap"
            rootfs = "/"

            [[mount]]
            source = "/somewhere/context"
            destination = "/context"
            "#,
    )
    .unwrap();
    let policy = config.policy().unwrap();
    assert_eq!(policy.command, vec!["agent", "--go"]);
    assert_eq!(policy.protocol, OutputFormat::Plain);
    assert_eq!(
        policy.workspace_destination,
        PathBuf::from("/tmp/orka/workspace")
    );
    assert_eq!(policy.io_destination, PathBuf::from("/tmp/orka/exchange"));
    assert!(!policy.network, "network denied unless enabled");
    assert_eq!(policy.extra_mounts.len(), 1);
    assert!(!policy.extra_mounts[0].writable, "read-only by default");
}

#[test]
fn codex_profile_is_resolved_from_genta() {
    let config: Config = toml::from_str(
        r#"
            [agent]
            kind = "codex"

            [isolation]
            backend = "bwrap"
            rootfs = "/"
            "#,
    )
    .unwrap();
    let policy = config.policy().unwrap();
    assert_eq!(policy.command.last().unwrap(), agent::AGENT_PROMPT);
    assert_eq!(
        policy.protocol,
        OutputFormat::Agent(genta::event::Protocol::CodexJsonl)
    );
    assert!(policy.command.iter().any(|argument| argument == "--json"));
    assert_eq!(
        policy.workspace_destination,
        PathBuf::from("/tmp/orka/workspace")
    );
    assert_eq!(policy.io_destination, PathBuf::from("/tmp/orka/exchange"));
    assert!(policy.network);
    assert!(policy
        .extra_mounts
        .iter()
        .any(|mount| mount.destination == Path::new("/tmp/agent-home/.codex") && mount.writable));
    assert_eq!(policy.environment["HOME"], "/tmp/agent-home");
    assert!(matches!(
        config.resolve().unwrap().backend,
        ResolvedBackend::Bwrap { .. }
    ));
}

/// An attempt is durable evidence, so the model and effort it ran on are
/// always pinned in its argv — from the configuration when it names them, and
/// from the provider's declared defaults when it does not.
#[test]
fn the_codex_profile_always_pins_a_model_and_an_effort() {
    let base = "[isolation]\nbackend = \"bwrap\"\nrootfs = \"/\"\n";

    let defaulted: Config = toml::from_str(&format!("[agent]\nkind = \"codex\"\n{base}")).unwrap();
    let command = defaulted.policy().unwrap().command;
    assert!(command
        .iter()
        .any(|argument| argument == &format!("model={:?}", Provider::Codex.default_model())));
    assert!(command.iter().any(|argument| argument
        == &format!(
            "model_reasoning_effort={:?}",
            Provider::Codex.default_effort().as_str()
        )));

    let pinned: Config = toml::from_str(&format!(
        "[agent]\nkind = \"codex\"\nmodel = \"gpt-5.6-luna\"\neffort = \"low\"\n{base}"
    ))
    .unwrap();
    let command = pinned.policy().unwrap().command;
    assert!(command
        .iter()
        .any(|argument| argument == r#"model="gpt-5.6-luna""#));
    assert!(command
        .iter()
        .any(|argument| argument == r#"model_reasoning_effort="low""#));
}

#[test]
fn an_unusable_effort_or_a_profileless_model_is_rejected() {
    let base = "[isolation]\nbackend = \"bwrap\"\nrootfs = \"/\"\n";

    // Not a level at all.
    let unknown: Config = toml::from_str(&format!(
        "[agent]\nkind = \"codex\"\neffort = \"turbo\"\n{base}"
    ))
    .unwrap();
    assert!(unknown
        .policy()
        .unwrap_err()
        .to_string()
        .contains("unknown reasoning effort"));

    // A level Genta names but no codex model accepts, and one this model
    // lacks though others have it.
    for agent in [
        "effort = \"minimal\"",
        "model = \"gpt-5.5\"\neffort = \"max\"",
    ] {
        let unaccepted: Config =
            toml::from_str(&format!("[agent]\nkind = \"codex\"\n{agent}\n{base}")).unwrap();
        assert!(unaccepted
            .policy()
            .unwrap_err()
            .to_string()
            .contains("not accepted by codex"));
    }

    // A literal command has no profile to pin a model on.
    let literal: Config = toml::from_str(&format!(
        "[agent]\ncommand = [\"agent\"]\nmodel = \"gpt-5.6-sol\"\n{base}"
    ))
    .unwrap();
    assert!(literal
        .policy()
        .unwrap_err()
        .to_string()
        .contains("require agent.kind"));
}

#[test]
fn rejects_driva_templates_and_ambiguous_agent_configuration() {
    let unsupported = toml::from_str::<Config>("[agent]\ntemplate = \"codex-exec\"\n");
    assert!(unsupported
        .unwrap_err()
        .to_string()
        .contains("unknown field `template`"));

    let both: Config = toml::from_str(
        "[agent]\nkind = \"codex\"\ncommand = [\"agent\"]\n[isolation]\nbackend = \"bwrap\"\n",
    )
    .unwrap();
    assert!(both
        .policy()
        .unwrap_err()
        .to_string()
        .contains("mutually exclusive"));
}

#[test]
fn bwrap_requires_an_explicit_rootfs() {
    let config: Config = toml::from_str("[agent]\nkind = \"codex\"\n").unwrap();
    assert!(config
        .policy()
        .unwrap_err()
        .to_string()
        .contains("isolation.rootfs is required"));
}

#[test]
fn init_never_overwrites_an_existing_configuration() {
    let dir = std::env::temp_dir().join(format!("orka-config-test-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(CONFIG_FILE);
    Config::init(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), default_config());
    assert!(Config::init(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), default_config());
    std::fs::remove_dir_all(dir).unwrap();
}
