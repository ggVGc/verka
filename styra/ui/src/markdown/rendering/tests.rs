use super::*;

fn rendered(spans: &[Span<'_>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

fn rendered_line(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn supports_common_inline_markdown_and_literal_unmatched_markers() {
    let base = Style::default();
    let spans = parse_inline_spans("*italic* and ~~gone~~ and `open", base);

    assert_eq!(rendered(&spans), "italic and gone and `open");
    assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
    assert!(spans[2].style.add_modifier.contains(Modifier::CROSSED_OUT));
}

#[test]
fn empty_inline_input_still_produces_a_span() {
    assert_eq!(parse_inline_spans("", Style::default()).len(), 1);
}

#[test]
fn block_lines_strip_the_heading_marker_and_style_the_heading() {
    let base = Style::default().fg(theme::TEXT);
    let lines = markdown_block_lines("# Title", base, "  ");

    assert_eq!(rendered_line(&lines[0]), "  Title");
    assert_eq!(lines[0].style.fg, Some(theme::MARKDOWN_HEADING));
    assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
    // The heading's colour reaches its text past the base style.
    assert_eq!(lines[0].spans[1].style.fg, Some(theme::MARKDOWN_HEADING));
}

/// Plain text is drawn in the base style's colour, not left to the
/// terminal's default foreground.
#[test]
fn block_lines_draw_plain_text_in_the_base_color() {
    let base = Style::default().fg(theme::USER_TEXT);
    let lines = markdown_block_lines("first\n\nsecond *em*", base, "  ");

    for line in &lines {
        for span in line
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
        {
            assert_eq!(span.style.fg, Some(theme::USER_TEXT), "{lines:?}");
        }
    }
}

#[test]
fn block_lines_show_only_the_title_of_a_file_citation() {
    let base = Style::default();
    let lines = markdown_block_lines(
        "see [app.rs:120](/home/me/src/app.rs:120) and \
             [src/app.rs:7](file:///home/me/src/app.rs) too",
        base,
        "",
    );

    assert_eq!(
        rendered_line(&lines[0]),
        "see app.rs:120 and src/app.rs:7 too"
    );
}

#[test]
fn block_lines_compact_every_link_and_can_show_destinations() {
    let base = Style::default();
    let compact = markdown_block_lines("[the docs](https://example.com)", base, "");
    assert_eq!(rendered_line(&compact[0]), "the docs");

    let full = markdown_block_lines_with_links(
        "[the docs](https://example.com)",
        base,
        "",
        LinkDisplay::Full,
    );
    assert_eq!(rendered_line(&full[0]), "the docs (https://example.com)");
}

#[test]
fn a_highlighted_entry_is_the_nth_citation_of_the_block() {
    let base = Style::default();
    let render = markdown_block_render(
        "see [app.rs:120](/src/app.rs:120) and [lib.rs:7](/src/lib.rs)",
        base,
        "",
        LinkDisplay::Compact,
        Some(1),
    );

    assert_eq!(render.entries, 2);
    let highlighted: Vec<&str> = render.lines[0]
        .spans
        .iter()
        .filter(|span| span.style.bg == Some(theme::LINK_HIGHLIGHT_BACKGROUND))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(highlighted, vec!["lib.rs:7"]);
}

#[test]
fn a_highlight_covers_the_destination_only_when_it_is_on_screen() {
    let base = Style::default();
    let source = "[app.rs:120](/src/app.rs:120)";
    let selected = |links| {
        markdown_block_render(source, base, "", links, Some(0)).lines[0]
            .spans
            .iter()
            .filter(|span| span.style.bg == Some(theme::LINK_HIGHLIGHT_BACKGROUND))
            .map(|span| span.content.to_string())
            .collect::<Vec<String>>()
            .concat()
    };

    assert_eq!(selected(LinkDisplay::Compact), "app.rs:120");
    assert_eq!(selected(LinkDisplay::Full), "app.rs:120 (/src/app.rs:120)");
}

#[test]
fn entries_are_counted_across_lines_and_an_absent_one_highlights_nothing() {
    let base = Style::default();
    let render = markdown_block_render(
        "- [one](/a)\n- [two](/b)",
        base,
        "",
        LinkDisplay::Compact,
        Some(7),
    );

    assert_eq!(render.entries, 2);
    assert!(render
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .all(|span| span.style.bg != Some(theme::LINK_HIGHLIGHT_BACKGROUND)));
}

#[test]
fn a_code_span_naming_a_file_is_an_entry_in_reading_order() {
    let base = Style::default();
    let text = "- **Field:** `genta/src/spec.rs:44` calls `Provider::x()`, \
                    see [lib.rs](/src/lib.rs) and `Cargo.toml`";
    let highlighted = |entry| {
        markdown_block_render(text, base, "", LinkDisplay::Compact, Some(entry)).lines[0]
            .spans
            .iter()
            .filter(|span| span.style.bg == Some(theme::LINK_HIGHLIGHT_BACKGROUND))
            .map(|span| span.content.to_string())
            .collect::<String>()
    };

    assert_eq!(markdown_link_count(text), 3);
    assert_eq!(highlighted(0), "genta/src/spec.rs:44");
    assert_eq!(highlighted(1), "lib.rs");
    assert_eq!(highlighted(2), "Cargo.toml");
    assert_eq!(
        markdown_link_destination(text, 0).as_deref(),
        Some("genta/src/spec.rs:44")
    );
    assert_eq!(
        markdown_link_destination(text, 1).as_deref(),
        Some("/src/lib.rs")
    );
    assert_eq!(
        markdown_link_destination(text, 2).as_deref(),
        Some("Cargo.toml")
    );

    let summary = parse_inline_spans_with_highlight(text, base, Some(2));
    let summary: String = summary
        .iter()
        .filter(|span| span.style.bg == Some(theme::LINK_HIGHLIGHT_BACKGROUND))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(summary, "Cargo.toml");
}

#[test]
fn a_link_labelled_in_code_is_one_entry_and_drops_its_destination() {
    let base = Style::default();
    let text = "see [`app.rs:120`](/src/app.rs:120)";
    let render = markdown_block_render(text, base, "", LinkDisplay::Compact, Some(0));

    assert_eq!(render.entries, 1);
    assert_eq!(rendered_line(&render.lines[0]), "see app.rs:120");
    assert_eq!(
        markdown_link_destination(text, 0).as_deref(),
        Some("/src/app.rs:120")
    );
}

#[test]
fn code_that_is_an_entry_is_backed_apart_from_plain_code() {
    let text = "`Provider::x()`, `Cargo.toml` and [`app.rs`](/src/app.rs)";
    let render = markdown_block_render(text, Style::default(), "", LinkDisplay::Compact, None);
    let background = |content: &str| {
        render.lines[0]
            .spans
            .iter()
            .find(|span| span.content == content)
            .and_then(|span| span.style.bg)
    };

    assert_eq!(
        background("Provider::x()"),
        Some(theme::INLINE_CODE_BACKGROUND)
    );
    assert_eq!(background("Cargo.toml"), Some(theme::ENTRY_CODE_BACKGROUND));
    assert_eq!(background("app.rs"), Some(theme::ENTRY_CODE_BACKGROUND));
}

#[test]
fn a_summary_styles_code_as_a_rendered_block_does() {
    let text = "`Provider::x()`, `Cargo.toml` and [`app.rs`](/src/app.rs)";
    let spans = parse_inline_spans(text, Style::default());
    let style = |content: &str| {
        spans
            .iter()
            .find(|span| span.content == content)
            .map(|span| (span.style.fg, span.style.bg))
    };
    let code = Some(theme::INLINE_CODE);
    let entry = Some(theme::ENTRY_CODE);

    assert_eq!(
        style("Provider::x()"),
        Some((code, Some(theme::INLINE_CODE_BACKGROUND)))
    );
    assert_eq!(
        style("Cargo.toml"),
        Some((entry, Some(theme::ENTRY_CODE_BACKGROUND)))
    );
    assert_eq!(
        style("app.rs"),
        Some((entry, Some(theme::ENTRY_CODE_BACKGROUND)))
    );
}

#[test]
fn only_code_that_reads_as_a_filename_is_a_file_reference() {
    for file in [
        "src/app.rs",
        "app.rs:120",
        "lib/mod.rs:7:3",
        "Cargo.toml",
        ".gitignore",
        "~/notes",
    ] {
        assert!(is_file_reference(file), "{file}");
    }
    for code in [
        "Provider::default_model",
        "self.spec().model",
        "config.default_model",
        "1.2.3",
        "https://example.com/a",
        "a b.rs",
        "\"claude-opus-5\"",
    ] {
        assert!(!is_file_reference(code), "{code}");
    }
}

#[test]
fn a_filename_in_a_fenced_code_block_is_not_an_entry() {
    assert_eq!(markdown_link_count("```\nsrc/main.rs\n```"), 0);
}

#[test]
fn a_heading_is_not_mistaken_for_a_link_label() {
    let base = Style::default();
    let render = markdown_block_render(
        "# Title [app.rs](/src/app.rs)",
        base,
        "",
        LinkDisplay::Compact,
        Some(0),
    );

    let highlighted: Vec<&str> = render.lines[0]
        .spans
        .iter()
        .filter(|span| span.style.bg == Some(theme::LINK_HIGHLIGHT_BACKGROUND))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(highlighted, vec!["app.rs"]);
}

#[test]
fn block_lines_render_a_table_with_borders() {
    let base = Style::default();
    let lines = markdown_block_lines("| A | B |\n|---|---|\n| 1 | 2 |", base, "");

    let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
    assert!(rendered.iter().any(|line| line.contains('┌')));
    assert!(rendered.iter().any(|line| line.contains('│')));
    assert!(rendered
        .iter()
        .any(|line| line.contains('A') && line.contains('B')));
}

#[test]
fn block_lines_preserve_source_structure_and_code() {
    for (case, source, expected) in [
        (
            "ordered list",
            "1. first\n2. second",
            vec!["1. first", "2. second"],
        ),
        ("bullet glyphs", "- one\n- two", vec!["• one", "• two"]),
        // Agent messages use single newlines: CommonMark soft breaks must
        // not silently join their lines or introduce blank separators.
        ("source newlines", "hello\nworld", vec!["hello", "world"]),
        ("fenced code", "```\nfn f() {}\n```", vec!["fn f() {}"]),
    ] {
        let lines = markdown_block_lines(source, Style::default(), "");
        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert_eq!(rendered, expected, "{case}");
    }
}

#[test]
fn tables_and_list_items_get_a_structural_indent_but_prose_does_not() {
    let base = Style::default();
    let table = markdown_block_lines("| A | B |\n|---|---|\n| 1 | 2 |", base, "  ");
    // Under the row's left border, which sits just past the indent.
    assert!(table.iter().all(|line| structural_indent(line) == Some(2)));

    let list = markdown_block_lines("- one\n2. two", base, "  ");
    let indents: Vec<Option<usize>> = list
        .iter()
        .filter(|line| !rendered_line(line).trim().is_empty())
        .map(structural_indent)
        .collect();
    // Two indent columns plus the marker and its space: "• " and "2. ".
    assert_eq!(indents, vec![Some(4), Some(5)]);

    let prose = markdown_block_lines("a sentence - with a dash", base, "  ");
    assert!(prose.iter().all(|line| structural_indent(line).is_none()));
}

/// Rendering is memoized (see [`crate::render_cache`]), so what is asked
/// for twice has to come back the same both times — and, more to the
/// point, what differs only in a display choice must not come back as the
/// rendering made under the other one.
#[test]
fn a_cached_block_is_not_reused_for_a_different_display_choice() {
    let text = "see [app.rs:120](/home/me/src/app.rs:120)";
    let base = Style::default().fg(theme::TEXT);

    let compact = markdown_block_lines_with_links(text, base, "  ", LinkDisplay::Compact);
    let full = markdown_block_lines_with_links(text, base, "  ", LinkDisplay::Full);
    assert_ne!(text_of(&compact), text_of(&full));
    assert_eq!(
        text_of(&markdown_block_lines_with_links(
            text,
            base,
            "  ",
            LinkDisplay::Compact
        )),
        text_of(&compact),
        "asking again has to give the same rendering back"
    );

    // The selection is part of what shapes a block, so it is part of the key.
    let plain = markdown_block_render(text, base, "  ", LinkDisplay::Compact, None);
    let selected = markdown_block_render(text, base, "  ", LinkDisplay::Compact, Some(0));
    assert_ne!(
        plain.lines[0].spans.last().map(|span| span.style),
        selected.lines[0].spans.last().map(|span| span.style),
    );

    // As are the base style and the indent.
    let indented = markdown_block_lines_with_links(text, base, "        ", LinkDisplay::Compact);
    assert_ne!(text_of(&indented), text_of(&compact));
}

/// The same, for the standalone code path: two blocks differing only in
/// language must not answer for each other.
#[test]
fn a_cached_code_block_is_keyed_by_its_language() {
    let source = "fn main() {}";
    let rust = syntax_highlighted_code_lines(source, Some("rust"), "  ")
        .expect("Rust is a known language");
    let unknown = syntax_highlighted_code_lines(source, Some("not-a-language"), "  ");
    assert!(unknown.is_none());
    assert_eq!(
        text_of(
            &syntax_highlighted_code_lines(source, Some("rust"), "  ")
                .expect("Rust is a known language")
        ),
        text_of(&rust),
        "asking again has to give the same rendering back"
    );
}

#[test]
fn highlighted_code_expands_tabs_like_plain_code() {
    for (language, source, expected) in [
        ("rust", "\tlet\tvalue = 1;", "      let    value = 1;"),
        ("elixir", "\tvalue\t= :ok", "      value    = :ok"),
    ] {
        let lines =
            syntax_highlighted_code_lines(source, Some(language), "  ").expect("known language");
        let expanded = source.replace('\t', "    ");
        let spaces =
            syntax_highlighted_code_lines(&expanded, Some(language), "  ").expect("known language");
        assert_eq!(text_of(&lines), text_of(&spaces), "{language}");
        assert_eq!(text_of(&lines)[0].0, expected, "{language}");
    }
}

#[test]
fn elixir_language_aliases_preserve_source_and_indent() {
    let source = "def hello, do: :world\n\n";
    for language in ["elixir", "ex", "exs", "Elixir"] {
        let lines = syntax_highlighted_code_lines(source, Some(language), "  ")
            .expect("Elixir is a known language");
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(texts, ["  def hello, do: :world", "  ", "  "]);
        assert!(lines[0].spans.len() > 3);
    }
}

fn text_of(lines: &[Line<'static>]) -> Vec<(String, Vec<Style>)> {
    lines
        .iter()
        .map(|line| {
            (
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
                line.spans.iter().map(|span| span.style).collect(),
            )
        })
        .collect()
}
