use super::*;
use crate::model::{ArtifactRef, Author, Outcome};

#[test]
fn blob_id_matches_git() {
    // `echo 'hello' | git hash-object --stdin`
    assert_eq!(
        blob_id(b"hello\n"),
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
    // `printf '' | git hash-object --stdin`
    assert_eq!(blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
}

#[test]
fn node_and_result_roundtrip() {
    let dir = std::env::temp_dir().join(format!("linka-store-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::init(dir.join(".linka")).unwrap();
    let node: NodeId = "node-1".parse().unwrap();

    let meta = NodeMeta {
        schema: 1,
        author: Author::Human,
        assignee: None,
        depends_on: vec!["node-a".parse().unwrap()],
        derived_from: vec![],
        verifies: None,
        extensions: Default::default(),
    };
    store
        .write_node(&node, &meta, "hello\n\nthe details")
        .unwrap();
    let (got, description) = store.read_node(&node).unwrap();
    assert_eq!(got.depends_on, vec!["node-a".parse().unwrap()]);
    assert_eq!(description, "hello\n\nthe details");
    assert_eq!(crate::model::title_of(&description), "hello");

    // The version changes exactly when the definition changes.
    let v1 = store.node_version(&node).unwrap();
    store.write_node(&node, &meta, "other description").unwrap();
    assert_ne!(v1, store.node_version(&node).unwrap());

    // No result yet; then one round-trips, without touching the version.
    assert!(store.read_result(&node).unwrap().is_none());
    let v2 = store.node_version(&node).unwrap();
    let result = ResultMeta {
        schema: 1,
        at: 0,
        author: Author::Machine,
        definition: v2.clone(),
        outcome: Outcome::Done.into(),
        project: crate::ProjectSnapshot {
            scheme: "git".into(),
            repository: String::new(),
            revision: String::new(),
            tree: String::new(),
        },
        consumed: vec![],
        context: vec![],
        output: Some(ArtifactRef {
            scheme: "git-commit".into(),
            repository: String::new(),
            id: "abc".into(),
        }),
        producer: None,
    };
    store.write_result(&node, &result, "did the thing").unwrap();
    let (r, notes) = store.read_result(&node).unwrap().unwrap();
    assert_eq!(r.output.as_ref().map(|a| a.id.as_str()), Some("abc"));
    assert_eq!(r.outcome, Outcome::Done.into());
    assert_eq!(notes, "did the thing");
    assert_eq!(store.node_version(&node).unwrap(), v2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loaded_records_hash_exact_bytes_and_reject_partial_results() {
    let dir = std::env::temp_dir().join(format!("linka-loader-test-{}", ulid::Ulid::new()));
    let store = Store::init(dir.join(".linka")).unwrap();
    let node: NodeId = "node-1".parse().unwrap();
    let meta = NodeMeta {
        schema: DEFINITION_SCHEMA,
        author: Author::Human,
        assignee: None,
        depends_on: vec![],
        derived_from: vec![],
        verifies: None,
        extensions: Default::default(),
    };
    store.write_node(&node, &meta, "naïve 🧪\n").unwrap();
    let node_path = store.node_path(&node);
    let mut definition_bytes = b"# formatting is versioned\n".to_vec();
    definition_bytes.extend(std::fs::read(&node_path).unwrap());
    std::fs::write(&node_path, &definition_bytes).unwrap();
    let loaded = store.load_definition(&node).unwrap();
    assert_eq!(loaded.version.metadata, blob_id(&definition_bytes));
    assert_eq!(loaded.version.description, blob_id("naïve 🧪\n".as_bytes()));

    let result = ResultMeta {
        schema: RESULT_SCHEMA,
        at: 0,
        author: Author::Machine,
        definition: loaded.version,
        outcome: Outcome::Failed.into(),
        project: crate::ProjectSnapshot {
            scheme: "git".into(),
            repository: String::new(),
            revision: String::new(),
            tree: String::new(),
        },
        consumed: vec![],
        context: vec![],
        output: None,
        producer: None,
    };
    store.write_result(&node, &result, "").unwrap();
    assert_eq!(
        store.load_result(&node).unwrap().unwrap().version.notes,
        None
    );
    std::fs::write(store.result_path(&node), b"").unwrap();
    assert_eq!(
        store.load_result(&node).unwrap().unwrap().version.notes,
        Some(blob_id(b""))
    );
    std::fs::remove_file(store.result_meta_path(&node)).unwrap();
    assert!(store
        .load_result(&node)
        .unwrap_err()
        .to_string()
        .contains("without"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opaque_node_attachments_round_trip_without_changing_node_versions() {
    let dir = std::env::temp_dir().join(format!("linka-attachment-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::init(dir.join(".linka")).unwrap();
    let node: NodeId = "node-1".parse().unwrap();
    let meta = NodeMeta {
        schema: 1,
        author: Author::Human,
        assignee: None,
        depends_on: vec![],
        derived_from: vec![],
        verifies: None,
        extensions: Default::default(),
    };
    store.write_node(&node, &meta, "attached").unwrap();
    let version = store.node_version(&node).unwrap();
    let data = [0, 1, 2, 255];
    let attachment = NodeAttachment {
        schema: ATTACHMENT_SCHEMA,
        namespace: "test.tool".into(),
        key: "arbitrary report".into(),
        created_at_ms: 42,
        media_type: Some("application/octet-stream".into()),
        content: blob_id(&data),
        size: data.len() as u64,
    };

    store
        .write_node_attachment(&node, &attachment, &data)
        .unwrap();

    assert_eq!(
        store
            .read_node_attachment(&node, "test.tool", "arbitrary report")
            .unwrap(),
        Some((attachment.clone(), data.to_vec()))
    );
    assert_eq!(
        store.list_node_attachments(&node).unwrap(),
        vec![attachment]
    );
    assert_eq!(store.node_version(&node).unwrap(), version);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_node_directory_is_rejected_during_discovery() {
    let dir = std::env::temp_dir().join(format!("linka-invalid-id-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::init(dir.join(".linka")).unwrap();
    std::fs::create_dir_all(store.root().join("nodes/.git")).unwrap();
    assert!(store.list_ids().is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_lays_out_the_workbench() {
    let dir = std::env::temp_dir().join(format!("linka-layout-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::init(dir.join(".linka")).unwrap();

    // Store and project sit side by side under the workbench root.
    assert_eq!(store.workbench_root(), dir);
    assert_eq!(store.project_root(), dir.join(PROJECT_DIR));
    assert!(dir.join(".linka/nodes").is_dir());
    assert!(store.project_root().is_dir());
    assert_eq!(store.store_name(), ".linka");

    let _ = std::fs::remove_dir_all(&dir);
}
