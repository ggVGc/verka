use super::*;

#[test]
fn title_is_the_first_non_empty_line_of_the_description() {
    assert_eq!(title_of("Parse config\n\nDetails follow."), "Parse config");
    assert_eq!(title_of("\n  \n  Leading blanks\nrest"), "Leading blanks");
    assert_eq!(title_of("one-liner"), "one-liner");
    assert_eq!(title_of(""), "(no description)");
    assert_eq!(title_of("  \n\t\n"), "(no description)");
}
