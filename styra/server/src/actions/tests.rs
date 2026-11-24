use super::*;

#[test]
fn creation_links_its_intent_and_survives_reopening_without_linking_unrelated_sessions() {
    let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
    let log = ActionLog::new(&root);
    let request = Request::StopInteraction {
        id: "source".into(),
    };
    let started = log
        .start(
            Action::Request(request.clone()),
            ActionOrigin::Client,
            vec![],
        )
        .unwrap();
    log.finish(
        started,
        ActionStatus::Succeeded,
        None,
        vec!["new-session".into()],
    )
    .unwrap();
    let failed = log
        .start(
            Action::Request(request.clone()),
            ActionOrigin::Automatic,
            vec!["new-session".into()],
        )
        .unwrap();
    log.finish(
        failed,
        ActionStatus::Failed,
        Some("provider refused".into()),
        vec![],
    )
    .unwrap();
    let reopened = ActionLog::new(&root);
    let records = reopened.for_session("new-session").unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].status, ActionStatus::Started);
    assert_eq!(records[1].status, ActionStatus::Succeeded);
    assert_eq!(records[2].origin, ActionOrigin::Automatic);
    assert_eq!(records[3].detail.as_deref(), Some("provider refused"));
    assert_eq!(records[0].action, Action::Request(request));
    assert!(reopened.for_session("unrelated").unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_crash_tail_does_not_hide_history_or_corrupt_the_next_append() {
    let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
    let log = ActionLog::new(&root);
    let started = log
        .start(
            Action::Request(Request::Shutdown),
            ActionOrigin::Client,
            vec!["session".into()],
        )
        .unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .open(root.join("actions.jsonl"))
        .unwrap();
    file.write_all(b"{\"id\":\"unfinished").unwrap();
    assert_eq!(log.for_session("session").unwrap(), vec![started.clone()]);
    log.finish(started, ActionStatus::Succeeded, None, vec![])
        .unwrap();
    let records = log.for_session("session").unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].status, ActionStatus::Succeeded);
    file.write_all(b"invalid complete record\n").unwrap();
    assert!(
        log.for_session("session").is_err(),
        "complete corruption must be surfaced"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_actions_keep_complete_correlated_records() {
    let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
    let log = std::sync::Arc::new(ActionLog::new(&root));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let log = log.clone();
            std::thread::spawn(move || {
                let started = log
                    .start(
                        Action::Request(Request::StopInteraction {
                            id: "session".into(),
                        }),
                        ActionOrigin::Client,
                        vec!["session".into()],
                    )
                    .unwrap();
                log.finish(started, ActionStatus::Succeeded, None, vec![])
                    .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let records = log.for_session("session").unwrap();
    assert_eq!(records.len(), 16);
    let ids: HashSet<_> = records.iter().map(|record| &record.id).collect();
    assert_eq!(ids.len(), 8);
    for id in ids {
        let pair: Vec<_> = records.iter().filter(|record| &record.id == id).collect();
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[0].status, ActionStatus::Started);
        assert_eq!(pair[1].status, ActionStatus::Succeeded);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_failed_intent_write_prevents_an_action_from_starting() {
    let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
    std::fs::write(&root, "not a directory").unwrap();
    let log = ActionLog::new(&root);
    assert!(log
        .start(
            Action::Request(Request::Shutdown),
            ActionOrigin::Client,
            vec![]
        )
        .is_err());
    std::fs::remove_file(root).unwrap();
}
