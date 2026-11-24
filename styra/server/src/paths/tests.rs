use super::*;

#[test]
fn state_home_prefers_xdg_and_otherwise_uses_the_home_default() {
    for (xdg, expected) in [
        (Some(OsString::from("/state")), "/state"),
        (None, "/home/user/.local/state"),
        (Some(OsString::new()), "/home/user/.local/state"),
    ] {
        assert_eq!(
            state_home(xdg.clone(), Some("/home/user".into())),
            Some(PathBuf::from(expected)),
            "{xdg:?}"
        );
    }
}

#[test]
fn daemon_and_standalone_stores_are_distinct_siblings() {
    let daemon = store_path(Some("/state".into()), Some("/home/user".into()), "styra");
    let standalone = store_path(
        Some("/state".into()),
        Some("/home/user".into()),
        "styra-standalone",
    );

    assert_eq!(daemon, Some(PathBuf::from("/state/styra")));
    assert_eq!(standalone, Some(PathBuf::from("/state/styra-standalone")));
    assert_ne!(daemon, standalone);
}

#[test]
fn runtime_directory_has_no_persistent_fallback() {
    assert_eq!(
        runtime_home(Some("/run/user/1000".into())),
        Some(PathBuf::from("/run/user/1000"))
    );
    assert_eq!(runtime_home(None), None);
    assert_eq!(runtime_home(Some(OsString::new())), None);
}
