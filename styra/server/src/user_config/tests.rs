use super::*;

#[test]
fn xdg_config_home_takes_precedence_with_a_home_fallback() {
    assert_eq!(
        config_home(Some("/config".into()), Some("/home/user".into())),
        Some(PathBuf::from("/config"))
    );
    assert_eq!(
        config_home(Some(OsString::new()), Some("/home/user".into())),
        Some(PathBuf::from("/home/user/.config"))
    );
}

#[test]
fn a_missing_file_is_an_empty_environment() {
    let dir = std::env::temp_dir().join(format!("styra-user-config-{}", uuid::Uuid::new_v4()));
    assert!(environment(Some(&dir.join(FILE_NAME))).unwrap().is_empty());
    assert!(environment(None).unwrap().is_empty());
}

#[test]
fn reads_the_environment_table_and_rejects_unknown_keys() {
    let dir = std::env::temp_dir().join(format!("styra-user-config-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(FILE_NAME);

    std::fs::write(&path, "[environment]\nFOO = \"bar\"\n").unwrap();
    let environment = environment(Some(&path)).unwrap();
    assert_eq!(
        environment.get(&OsString::from("FOO")),
        Some(&OsString::from("bar"))
    );

    std::fs::write(&path, "[enviroment]\nFOO = \"bar\"\n").unwrap();
    assert!(load(&path).is_err());

    std::fs::remove_dir_all(&dir).unwrap();
}
