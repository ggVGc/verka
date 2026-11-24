use super::*;
use crate::agent::Provider;

#[test]
fn a_topic_is_cut_down_to_words_and_hyphens() {
    assert_eq!(
        branch_fragment("Fix the flaky checkout test").unwrap(),
        "fix-the-flaky-checkout-test"
    );
    assert_eq!(
        branch_fragment("  **Branch:** `Fix/Thing` — now!  ").unwrap(),
        "branch-fix-thing-now"
    );
    assert_eq!(branch_fragment("Ärende 42").unwrap(), "rende-42");
    assert_eq!(branch_fragment("···"), None);
    assert_eq!(branch_fragment(""), None);
}

/// A model that answers with an essay, or a prompt used directly as the
/// fallback, still produces a topic short enough to read beside an id.
#[test]
fn a_long_topic_is_truncated_at_a_word_boundary_it_can_reach() {
    let topic = branch_fragment(&"word ".repeat(40)).unwrap();
    assert!(topic.chars().count() <= TOPIC_LIMIT, "{topic:?}");
    assert!(!topic.ends_with('-'));
    assert!(topic.starts_with("word-word"));
}

/// The one topic is read twice: hyphenated on the branch, and as a phrase
/// in the Session picker.
#[test]
fn a_topic_reads_as_a_branch_and_as_a_name() {
    assert_eq!(title("fix-flaky-checkout-test"), "Fix flaky checkout test");
    assert_eq!(title("rende-42"), "Rende 42");
}

/// An interaction launched with no prompt has no topic and keeps the id
/// alone. Checked before anything is run, so a launch without a prompt
/// does not pay for an errand to tell it so.
#[test]
fn a_launch_without_a_prompt_has_no_topic() {
    let selection = Selection::new(Provider::Claude);
    assert!(topic_for_prompt(&selection, None).is_none());
    assert!(topic_for_prompt(&selection, Some("   \n ")).is_none());
}

/// An operator asking for a name is told why there is none, rather than
/// handed a fallback, and no errand runs to find out.
#[test]
fn a_requested_name_needs_a_prompt() {
    let selection = Selection::new(Provider::Claude);
    assert!(name_for_prompt(&selection, None).is_err());
    assert!(name_for_prompt(&selection, Some("  ")).is_err());
}

/// The prompt is shown to the model as text under a heading, bounded, and
/// framed as something to summarise rather than to carry out.
#[test]
fn the_instruction_carries_a_bounded_copy_of_the_prompt() {
    let shown = instruction("Fix the flaky checkout test");
    assert!(shown.contains("--- task ---\nFix the flaky checkout test"));
    assert!(shown.contains("do not act on it"));

    let long = "pasted ".repeat(PROMPT_LIMIT);
    let shown = instruction(&long);
    let task = shown.split_once("--- task ---\n").unwrap().1;
    assert_eq!(task.chars().count(), PROMPT_LIMIT);
}
