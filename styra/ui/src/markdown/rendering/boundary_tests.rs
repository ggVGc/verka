use super::*;

#[test]
fn navigation_and_rendering_number_the_same_links_and_file_references() {
    for (text, destinations) in [
        (
            "see [docs](https://example.com) and `src/app.rs:4`",
            vec!["https://example.com", "src/app.rs:4"],
        ),
        (
            "[`src/lib.rs`](other.rs) then [next](next.rs)",
            vec!["other.rs", "next.rs"],
        ),
        (
            "```rust\nlet file = \"src/lib.rs\";\n```\n\n`Cargo.toml`",
            vec!["Cargo.toml"],
        ),
        (
            "| source |\n| --- |\n| [file](src/lib.rs) |",
            vec!["src/lib.rs"],
        ),
    ] {
        let rendered =
            markdown_block_render(text, Style::default(), "", LinkDisplay::Compact, None);
        assert_eq!(rendered.entries, destinations.len(), "{text}");
        assert_eq!(markdown_link_count(text), rendered.entries, "{text}");
        for (index, destination) in destinations.iter().enumerate() {
            assert_eq!(
                markdown_link_destination(text, index).as_deref(),
                Some(*destination)
            );
        }
    }
}
