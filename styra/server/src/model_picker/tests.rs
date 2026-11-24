use super::*;

#[test]
fn clients_share_history_and_reselecting_moves_a_model_to_the_front() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    let catalog = catalog(&[]);
    let first = &catalog[1].model;
    let second = &catalog[catalog.len() - 1].model;
    record(&path, first).unwrap();
    record(&path, second).unwrap();
    record(&path, first).unwrap();
    let recent = load(&path);
    assert_eq!(recent, vec![first.clone(), second.clone()]);
    let ordered = super::catalog(&recent);
    assert_eq!(&ordered[0].model, first);
    assert_eq!(&ordered[1].model, second);
    let remaining: Vec<_> = ordered.iter().skip(2).collect();
    let expected: Vec<_> = catalog
        .iter()
        .filter(|row| &row.model != first && &row.model != second)
        .collect();
    assert_eq!(remaining, expected);
}

#[test]
fn concurrent_clients_preserve_each_others_selections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::thread::scope(|scope| {
        for model in ["one", "two", "three"] {
            let path = &path;
            scope.spawn(move || record(path, model).unwrap());
        }
    });
    let mut models = load(&path);
    models.sort();
    assert_eq!(models, ["one", "three", "two"]);
}

#[test]
fn missing_or_malformed_history_uses_catalog_order_and_history_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    assert!(load(&path).is_empty());
    fs::write(&path, b"invalid json").unwrap();
    assert!(load(&path).is_empty());
    for model in 0..20 {
        record(&path, &model.to_string()).unwrap();
    }
    let recent = load(&path);
    assert_eq!(recent.len(), RECENT_MODELS);
    assert_eq!(recent[0], "19");
}
