use super::*;
use crate::input::{sample_input, DependencyContext};

#[test]
fn the_prompt_carries_the_task_related_work_and_the_contract() {
    let mut input = sample_input("node-1");
    input.description = "Implement the parser".into();
    input.dependency_context = vec![DependencyContext {
        node: "node-dep".parse().unwrap(),
        title: "Define the grammar".into(),
        result_notes: "grammar lives in grammar.md".into(),
    }];
    input.lineage_context = vec![DependencyContext {
        node: "node-src".parse().unwrap(),
        title: "Original spec".into(),
        result_notes: String::new(),
    }];

    let prompt = build_prompt(&input, &ExecutionPolicy::new(vec!["agent".into()]));
    assert!(prompt.contains("# Task (node-1)"));
    assert!(prompt.contains("Implement the parser"));
    assert!(prompt.contains("Completed dependencies"));
    assert!(prompt.contains("Define the grammar (node-dep)"));
    assert!(prompt.contains("grammar lives in grammar.md"));
    assert!(prompt.contains("Derived from"));
    assert!(prompt.contains("Original spec (node-src)"));
    assert!(prompt.contains("outcome = \"succeeded\""));
}
