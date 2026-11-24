use super::*;
use linka::{ArtifactRef, ConsumedNode, ContextPin, DefinitionVersion, ResultVersion};

#[test]
fn attempt_input_round_trips_through_toml() {
    let mut input = sample_input("node-1");
    // Populate every embedded Linka type so the round-trip exercises them.
    input.snapshot.dependencies = vec![ConsumedNode {
        id: "node-dep".parse().unwrap(),
        definition: DefinitionVersion {
            metadata: "dm".into(),
            description: "dd".into(),
        },
        result: Some(ResultVersion {
            metadata: "rm".into(),
            notes: Some("rn".into()),
        }),
        outcome: Some(linka::ResultOutcome::Work(linka::Outcome::Done)),
        output: Some(ArtifactRef {
            scheme: "git-commit".into(),
            repository: "r".into(),
            id: "beef".into(),
        }),
    }];
    input.snapshot.lineage = input.snapshot.dependencies.clone();
    input.snapshot.context = vec![ContextPin {
        path: "src/x.rs".parse().unwrap(),
        identity: "blob".into(),
        observed: true,
    }];
    input.snapshot.previous_result = Some(ResultVersion {
        metadata: "pm".into(),
        notes: None,
    });
    input.dependency_context = vec![DependencyContext {
        node: "node-dep".parse().unwrap(),
        title: "the dependency".into(),
        result_notes: "it worked".into(),
    }];

    let text = toml::to_string_pretty(&input).unwrap();
    let back: AttemptInput = toml::from_str(&text).unwrap();
    assert_eq!(back, input);
}
