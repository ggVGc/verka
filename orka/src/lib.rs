//! Graph-driven agent orchestration.
//!
//! Orka selects ready work from a [`ports::WorkGraph`], freezes its inputs in
//! a durable attempt, runs an agent command through a
//! [`ports::IsolatedExecutor`] with an explicitly chosen capability grant,
//! and submits a version-checked result. Production adapters wrap the Linka
//! and Driva libraries; [`fakes`] substitute for both in tests.
//!
//! * [`ports`] — the narrow traits and Orka-owned types crossing them.
//! * [`fakes`] — in-memory port implementations for tests and harnesses.

pub mod attempt;
pub mod fakes;
pub mod linka_graph;
pub mod ports;

#[cfg(test)]
mod tests {
    use crate::fakes::{FakeExecutor, FakeWorkGraph, FakeWorkspaces};
    use crate::ports::*;
    use std::collections::BTreeMap;

    fn frozen(node: &str) -> FrozenInput {
        FrozenInput {
            node: NodeId(node.into()),
            definition: DefinitionFingerprint {
                metadata: "m1".into(),
                description: "d1".into(),
            },
            description: "Do the thing".into(),
            dependencies: vec![],
            input_commit: "c0ffee".into(),
        }
    }

    #[test]
    fn frozen_input_roundtrips_through_toml() {
        let input = FrozenInput {
            dependencies: vec![FrozenDependency {
                id: NodeId("node-dep".into()),
                definition: DefinitionFingerprint {
                    metadata: "dm".into(),
                    description: "dd".into(),
                },
                result: Some(ResultFingerprint {
                    metadata: "rm".into(),
                    notes: None,
                }),
                output: Some(ArtifactPin {
                    scheme: "git-commit".into(),
                    repository: String::new(),
                    id: "beef".into(),
                }),
                title: "the dependency".into(),
                result_notes: "it worked".into(),
            }],
            ..frozen("node-1")
        };
        let text = toml::to_string_pretty(&input).unwrap();
        let back: FrozenInput = toml::from_str(&text).unwrap();
        assert_eq!(back, input);
    }

    #[test]
    fn fake_graph_freezes_submits_and_reports_stale() {
        let mut graph = FakeWorkGraph::default();
        graph.items.push(WorkItem {
            id: NodeId("node-1".into()),
            title: "Do the thing".into(),
        });
        graph.frozen.insert("node-1".into(), frozen("node-1"));
        graph.stale.push("node-2".into());
        graph.frozen.insert("node-2".into(), frozen("node-2"));

        assert_eq!(graph.select_ready().unwrap().len(), 1);
        let input = graph.freeze(&NodeId("node-1".into())).unwrap();
        assert_eq!(input.input_commit, "c0ffee");
        assert!(graph.freeze(&NodeId("node-x".into())).is_err());

        let accepted = graph
            .submit(&Submission {
                frozen: input,
                outcome: WorkOutcome::Succeeded {
                    outputs: vec![],
                    message: None,
                    notes: "done".into(),
                },
                workspace: None,
            })
            .unwrap();
        assert!(matches!(accepted, SubmitOutcome::Accepted { .. }));

        let stale = graph
            .submit(&Submission {
                frozen: frozen("node-2"),
                outcome: WorkOutcome::Failed {
                    notes: "n/a".into(),
                },
                workspace: None,
            })
            .unwrap();
        assert!(matches!(stale, SubmitOutcome::Stale { .. }));
        assert_eq!(graph.submissions.borrow().len(), 1);
    }

    #[test]
    fn fake_executor_streams_transcript_and_records_the_grant() {
        let dir = std::env::temp_dir().join(format!("orka-ports-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let executor = FakeExecutor {
            exit_code: 7,
            transcript: "agent says hi\n".into(),
            ..Default::default()
        };
        let spec = ExecutionSpec {
            command: vec!["agent".into(), "--go".into()],
            working_directory: "/workspace".into(),
            mounts: vec![MountSpec {
                source: dir.clone(),
                destination: "/workspace".into(),
                writable: true,
            }],
            environment: BTreeMap::new(),
            network: false,
        };
        let transcript = dir.join("transcript.log");
        let report = executor.run(&spec, &transcript).unwrap();
        assert_eq!(report.exit_code, 7);
        assert_eq!(
            std::fs::read_to_string(&transcript).unwrap(),
            "agent says hi\n"
        );
        assert_eq!(executor.runs.borrow()[0], spec);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fake_workspaces_prepare_once_and_retain_dirty_trees() {
        let root = std::env::temp_dir().join(format!("orka-ws-test-{}", ulid::Ulid::new()));
        let mut workspaces = FakeWorkspaces::new(&root);
        workspaces.dirty.push("attempt-2".into());

        let ws = workspaces.prepare("attempt-1", "c0ffee").unwrap();
        assert!(ws.path.is_dir());
        assert!(workspaces.prepare("attempt-1", "c0ffee").is_err());
        assert_eq!(workspaces.cleanup(&ws).unwrap(), CleanupOutcome::Removed);
        assert!(!ws.path.exists());
        assert_eq!(
            workspaces.cleanup(&ws).unwrap(),
            CleanupOutcome::AlreadyAbsent
        );

        let dirty = workspaces.prepare("attempt-2", "c0ffee").unwrap();
        assert_eq!(
            workspaces.cleanup(&dirty).unwrap(),
            CleanupOutcome::RetainedDirty
        );
        assert!(dirty.path.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
