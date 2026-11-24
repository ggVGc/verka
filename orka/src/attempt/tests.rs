use super::*;
use crate::executor::MountSpec;
use crate::input::sample_input;
use std::collections::BTreeMap;

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store() -> (TempDir, FsAttemptStore) {
    let dir = std::env::temp_dir().join(format!("orka-attempt-test-{}", ulid::Ulid::new()));
    (TempDir(dir.clone()), FsAttemptStore::new(dir.join(".orka")))
}

fn input() -> AttemptInput {
    sample_input("node-1")
}

fn workspace() -> PreparedWorkspace {
    PreparedWorkspace {
        schema: crate::workspace::WORKSPACE_SCHEMA,
        path: "/tmp/ws".into(),
        git_dir: "/tmp/ws.git".into(),
        branch: "orka/attempts/attempt-x".into(),
        input_commit: "c0ffee".into(),
        identity: "workspace-x".into(),
        audit: Default::default(),
    }
}

fn spec() -> ExecutionSpec {
    ExecutionSpec {
        command: vec!["agent".into()],
        protocol: OutputFormat::Plain,
        working_directory: "/tmp/orka/workspace".into(),
        mounts: vec![MountSpec {
            source: "/tmp/ws".into(),
            destination: "/tmp/orka/workspace".into(),
            writable: true,
        }],
        environment: BTreeMap::new(),
        network: false,
    }
}

fn report() -> ExecutionReport {
    ExecutionReport {
        backend: "fake".into(),
        exit_code: 0,
        started_at_ms: 1,
        finished_at_ms: 2,
    }
}

/// The lifecycle stops cleanly at every step: whatever was durably written
/// before a crash classifies the attempt, and loading never guesses.
#[test]
fn phase_is_derived_from_the_files_present_at_each_step() {
    let (_temp, store) = store();
    let id = AttemptId::new();

    assert!(store.load(&id).is_err());
    store.create(&id, &input()).unwrap();
    assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Created);

    store.plan_workspace(&id, &workspace()).unwrap();
    assert_eq!(
        store.load(&id).unwrap().phase(),
        AttemptPhase::WorkspacePlanned
    );

    store.mark_prepared(&id).unwrap();
    assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Prepared);

    store.record_request(&id, &spec()).unwrap();
    assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Requested);

    store.record_evidence(&id, &report()).unwrap();
    assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Executed);

    store
        .seal(
            &id,
            SealedState::Submitted {
                output_commit: Some("beef".into()),
            },
        )
        .unwrap();
    let snapshot = store.load(&id).unwrap();
    assert_eq!(snapshot.phase(), AttemptPhase::Sealed);

    // The snapshot carries every frozen fact back out.
    assert_eq!(snapshot.record.input, input());
    assert_eq!(snapshot.workspace.unwrap(), workspace());
    assert_eq!(snapshot.request.unwrap(), spec());
    assert_eq!(snapshot.evidence.unwrap(), report());
    assert_eq!(
        snapshot.seal.unwrap().state,
        SealedState::Submitted {
            output_commit: Some("beef".into())
        }
    );
}

#[test]
fn attempts_are_never_reused_and_ids_list_in_order() {
    let (_temp, store) = store();
    let id = AttemptId::new();
    store.create(&id, &input()).unwrap();
    assert!(store.create(&id, &input()).is_err());

    let second = AttemptId::new();
    store.create(&second, &input()).unwrap();
    // Ids minted in the same millisecond have no defined ULID order;
    // listing promises lexicographic order, so compare against that.
    let mut expected = vec![id, second];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(store.list().unwrap(), expected);
}

#[test]
fn listing_an_uninitialised_store_is_empty_not_an_error() {
    let (_temp, store) = store();
    assert_eq!(store.list().unwrap(), vec![]);
}

#[test]
fn audit_reports_orphaned_and_contradictory_attempt_records() {
    let (_temp, store) = store();
    let orphan = AttemptId::new();
    std::fs::create_dir_all(store.attempt_dir(&orphan)).unwrap();

    let mismatched = AttemptId::new();
    store.create(&mismatched, &input()).unwrap();
    let mut record = store.load(&mismatched).unwrap().record;
    record.id = AttemptId::new();
    write_toml(&store.attempt_record_path(&mismatched), &record).unwrap();
    write_toml(
        &store.attempt_dir(&mismatched).join("request.toml"),
        &spec(),
    )
    .unwrap();

    let problems = store.audit().unwrap();
    let all = problems.join("\n");
    assert!(
        all.contains(&format!("{orphan}: unreadable attempt record")),
        "{all}"
    );
    assert!(
        all.contains(&format!(
            "{mismatched}: attempt record identifies itself as {}",
            record.id
        )),
        "{all}"
    );
    assert!(
        all.contains(&format!(
            "{mismatched}: execution request exists without a prepared marker"
        )),
        "{all}"
    );
}

#[test]
fn only_unsealed_attempts_without_evidence_can_be_discarded() {
    let (_temp, store) = store();
    let unexecuted = AttemptId::new();
    store.create(&unexecuted, &input()).unwrap();
    store.record_request(&unexecuted, &spec()).unwrap();
    store.discard_without_evidence(&unexecuted).unwrap();
    assert!(store.list().unwrap().is_empty());

    let interrupted = AttemptId::new();
    store.create(&interrupted, &input()).unwrap();
    store
        .seal(
            &interrupted,
            SealedState::Interrupted {
                reason: "backend failure".into(),
            },
        )
        .unwrap();
    assert!(store.discard_without_evidence(&interrupted).is_err());
    assert!(store.load(&interrupted).is_ok());

    let executed = AttemptId::new();
    store.create(&executed, &input()).unwrap();
    store.record_evidence(&executed, &report()).unwrap();
    assert!(store.discard_without_evidence(&executed).is_err());
    assert!(store.load(&executed).is_ok());
}

#[test]
fn only_linka_registered_attempts_can_be_discarded_after_completion() {
    let (_temp, store) = store();
    let accepted = AttemptId::new();
    store.create(&accepted, &input()).unwrap();
    store
        .seal(
            &accepted,
            SealedState::Submitted {
                output_commit: None,
            },
        )
        .unwrap();
    store.discard_registered(&accepted).unwrap();
    assert!(!store.contains(&accepted));

    let stale = AttemptId::new();
    store.create(&stale, &input()).unwrap();
    store
        .seal(
            &stale,
            SealedState::StaleAtSubmit {
                conflicts: Vec::new(),
            },
        )
        .unwrap();
    assert!(store.discard_registered(&stale).is_err());
    assert!(store.contains(&stale));
}

#[test]
fn sealing_is_idempotent_but_never_rewrites_history() {
    let (_temp, store) = store();
    let id = AttemptId::new();
    store.create(&id, &input()).unwrap();

    let first = store
        .seal(
            &id,
            SealedState::Interrupted {
                reason: "backend died".into(),
            },
        )
        .unwrap();
    let again = store
        .seal(
            &id,
            SealedState::Interrupted {
                reason: "backend died".into(),
            },
        )
        .unwrap();
    assert_eq!(first, again);

    let conflict = store.seal(&id, SealedState::FailureRecorded);
    assert!(conflict.is_err());
    assert_eq!(
        store.load(&id).unwrap().seal.unwrap().state,
        SealedState::Interrupted {
            reason: "backend died".into()
        }
    );
}

#[test]
fn corrupt_records_are_errors_not_degraded_phases() {
    let (_temp, store) = store();
    let id = AttemptId::new();
    store.create(&id, &input()).unwrap();
    let dir = store.root().join("attempts").join(&id.0);
    std::fs::write(dir.join("evidence.toml"), "not toml [").unwrap();
    assert!(store.load(&id).is_err());
}

#[test]
fn prior_attempt_schemas_are_rejected_without_migration() {
    let (_temp, store) = store();
    let id = AttemptId::new();
    store.create(&id, &input()).unwrap();
    let path = store.attempt_record_path(&id);
    let current = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        current.replacen(&format!("schema = {ATTEMPT_SCHEMA}"), "schema = 1", 1),
    )
    .unwrap();

    let error = store.load(&id).unwrap_err();
    assert!(
        error.to_string().contains("unsupported schema 1"),
        "{error:#}"
    );
}

#[test]
fn steps_require_a_created_attempt() {
    let (_temp, store) = store();
    let id = AttemptId::new();
    assert!(store.plan_workspace(&id, &workspace()).is_err());
    assert!(store.mark_prepared(&id).is_err());
    assert!(store.record_request(&id, &spec()).is_err());
    assert!(store.record_evidence(&id, &report()).is_err());
    assert!(store.seal(&id, SealedState::FailureRecorded).is_err());
}
