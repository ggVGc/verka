use super::*;
use styra_protocol::agent::Effort;

fn defaults_path() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings").join(FILE_NAME);
    (directory, path)
}

fn claude_selection() -> Selection {
    Selection {
        provider: Provider::Claude,
        model: "claude-sonnet-5".into(),
        effort: Effort::Max,
    }
}

#[test]
fn xdg_config_home_takes_precedence_with_a_home_fallback() {
    assert_eq!(
        config_home(Some("/config".into()), Some("/home/user".into())),
        Some(PathBuf::from("/config"))
    );
    assert_eq!(
        config_home(None, Some("/home/user".into())),
        Some(PathBuf::from("/home/user/.config"))
    );
    assert_eq!(
        config_home(Some(OsString::new()), Some("/home/user".into())),
        Some(PathBuf::from("/home/user/.config"))
    );
}

#[test]
fn a_missing_file_uses_the_declared_codex_defaults() {
    let (_directory, path) = defaults_path();
    let defaults = load_or_default(&path).unwrap();
    assert_eq!(defaults.selection, Selection::new(Provider::Codex));
    assert_eq!(defaults.launch, LaunchPolicy::default());
}

#[test]
fn a_saved_selection_and_launch_policy_round_trip() {
    let (_directory, path) = defaults_path();
    let defaults = Defaults {
        selection: claude_selection(),
        launch: LaunchPolicy {
            network: Some(true),
            writable_workspace: None,
            templates: vec!["rust".into()],
            mounts: vec![styra_protocol::LaunchMount {
                source: PathBuf::from("/srv/data"),
                destination: None,
                writable: false,
            }],
            ignore_workspace: false,
            ..LaunchPolicy::default()
        },
    };

    save(&path, &defaults).unwrap();
    assert_eq!(load_or_default(&path).unwrap(), defaults);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// Each of the two halves is saved without disturbing the other, so
/// pressing `D` in the launch picker cannot silently drop a saved sandbox
/// policy (or the other way round).
#[test]
fn saving_one_half_of_the_defaults_keeps_the_other() {
    let (_directory, path) = defaults_path();
    let launch = LaunchPolicy {
        network: Some(true),
        templates: vec!["browser".into()],
        ..LaunchPolicy::default()
    };
    let selection = claude_selection();

    save_launch(&path, &launch).unwrap();
    save_selection(&path, &selection).unwrap();

    let defaults = load_or_default(&path).unwrap();
    assert_eq!(defaults.launch, launch);
    assert_eq!(defaults.selection, selection);
}

/// A defaults file written before the launch policy existed holds a bare
/// selection. It must keep working rather than erroring its owner out of
/// their next launch.
#[test]
fn a_defaults_file_holding_only_a_selection_is_still_read() {
    let (_directory, path) = defaults_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let selection = claude_selection();
    fs::write(&path, serde_json::to_vec(&selection).unwrap()).unwrap();

    let defaults = load_or_default(&path).unwrap();
    assert_eq!(defaults.selection, selection);
    assert_eq!(defaults.launch, LaunchPolicy::default());
}

#[test]
fn malformed_saved_defaults_are_not_silently_ignored() {
    let (_directory, path) = defaults_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{ definitely not json").unwrap();

    let error = load_or_default(&path).unwrap_err();
    assert!(error.to_string().contains("parsing Styra defaults"));
}
