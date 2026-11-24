use super::*;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("linka-pair-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn missing_file_means_unpaired() {
    let dir = temp_dir("missing");
    assert!(Pairing::load(&dir).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pairing_roundtrips() {
    let dir = temp_dir("roundtrip");
    let pairing = Pairing {
        schema: 1,
        root_commit: "8a1f9c2e".into(),
        paired_at: 1719571200000,
        name: Some("splurt".into()),
        remote: Some("git@host:me/splurt.git".into()),
    };
    pairing.save(&dir).unwrap();
    let got = Pairing::load(&dir).unwrap().unwrap();
    assert_eq!(got.root_commit, "8a1f9c2e");
    assert_eq!(got.paired_at, 1719571200000);
    assert_eq!(got.name.as_deref(), Some("splurt"));
    assert_eq!(got.remote.as_deref(), Some("git@host:me/splurt.git"));

    // The informational fields are optional: an identity-only file loads,
    // and an identity-only pairing serialises without empty keys.
    let bare = Pairing {
        schema: 1,
        root_commit: "8a1f9c2e".into(),
        paired_at: 0,
        name: None,
        remote: None,
    };
    bare.save(&dir).unwrap();
    let text = std::fs::read_to_string(dir.join(PAIRING_FILE)).unwrap();
    assert!(!text.contains("name"), "{text}");
    assert!(!text.contains("remote"), "{text}");
    assert!(Pairing::load(&dir).unwrap().unwrap().name.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_file_is_an_error_not_unpaired() {
    let dir = temp_dir("malformed");
    std::fs::write(dir.join(PAIRING_FILE), "not = = toml").unwrap();
    assert!(Pairing::load(&dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
