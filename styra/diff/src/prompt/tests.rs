use super::*;

#[test]
fn source_footer_preserves_prompt_and_paths() {
    let prompt =
        Prompt::from_message("first\n\nSource: example\n\nSource: dir/a:b.rs:12", 7).unwrap();
    assert_eq!(prompt.path, "dir/a:b.rs");
    assert_eq!(prompt.line, Some(12));
    assert_eq!(prompt.text, "first\n\nSource: example");
    assert_eq!(prompt.entry, 7);
    assert!(prompt.matches(("dir/a:b.rs", Some(12))));
    assert!(!prompt.matches(("dir/a:b.rs", None)));
    assert_eq!(
        Prompt::from_message("hi\n\nSource: a.rs", 0).unwrap().line,
        None
    );
    assert!(Prompt::from_message("hi\n\nSource: a.rs\nmore", 0).is_none());
    assert!(Prompt::from_message("hi", 0).is_none());
}
