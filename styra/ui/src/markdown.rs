//! Markdown-to-Ratatui styling for agent messages.
//!
//! Multi-line detail blocks are rendered by `tui-markdown`, which parses a
//! whole buffer at once and so can render tables, code fences, and other
//! multi-line constructs correctly. Single-line summaries stay on the
//! lighter-weight `pulldown-cmark`-based inline renderer below, since
//! `tui-markdown` has no single-line-only mode.

use crate::palette;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkDisplay {
    Compact,
    Full,
}
/// Which entry of a rendered block, if any, carries the selection.
///
/// Entries are numbered in reading order across the whole block, starting at
/// zero, so a caller that keeps a `usize` and bounds it by
/// [`BlockRender::entries`] can walk a response's citations without knowing
/// anything about how they were laid out.
pub type EntryIndex = usize;

/// A rendered detail block, plus how many entries it offers for highlighting.
///
/// The count is what a caller needs to move a selection: it is the number of
/// entries the returned lines contain, whether or not any of them is
/// highlighted.
#[derive(Clone, Debug)]
pub struct BlockRender {
    pub lines: Vec<Line<'static>>,
    pub entries: usize,
}

use crate::render_cache::{Memo, Weigh};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::cell::RefCell;
use std::sync::LazyLock;
use tui_markdown::{AlertKind, CodeTheme, StyleSheet};

static MARKDOWN_CODE_THEME: LazyLock<CodeTheme> = LazyLock::new(|| {
    CodeTheme::from_textmate(palette::MARKDOWN_CODE_THEME)
        .expect("Styra's embedded Markdown code theme must be valid")
});

/// Everything that shapes a [`syntax_highlighted_code_lines`] result.
#[derive(PartialEq, Eq, Hash)]
struct CodeKey {
    text: String,
    language: String,
    indent: String,
}

/// Everything that shapes a [`markdown_block_render`] result.
#[derive(PartialEq, Eq, Hash)]
struct BlockKey {
    text: String,
    base_style: Style,
    indent: String,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
}

impl Weigh for Option<Vec<Line<'static>>> {
    fn weight(&self) -> usize {
        // A block that does not highlight still occupies a table slot, and the
        // parse that decided so is what the cache is saving.
        self.as_ref().map_or(1, Vec::len)
    }
}

impl Weigh for BlockRender {
    fn weight(&self) -> usize {
        self.lines.len().max(1)
    }
}

thread_local! {
    /// See [`crate::render_cache`]: the event list asks for every one of these
    /// again on every frame, and a frame is drawn for every keystroke.
    static CODE_CACHE: RefCell<Memo<CodeKey, Option<Vec<Line<'static>>>>> =
        RefCell::new(Memo::default());
    static BLOCK_CACHE: RefCell<Memo<BlockKey, BlockRender>> = RefCell::new(Memo::default());
}

/// Renders a detail block's full markdown buffer as styled lines, each
/// prefixed with `indent`.
#[cfg(test)]
pub fn markdown_block_lines(text: &str, base_style: Style, indent: &str) -> Vec<Line<'static>> {
    markdown_block_lines_with_links(text, base_style, indent, LinkDisplay::Compact)
}

/// As [`markdown_block_lines`], with the operator's link-display choice.
pub fn markdown_block_lines_with_links(
    text: &str,
    base_style: Style,
    indent: &str,
    links: LinkDisplay,
) -> Vec<Line<'static>> {
    markdown_block_render(text, base_style, indent, links, None).lines
}

/// Syntax-highlights a standalone fenced-code block when `language` is known
/// to tui-markdown.  Code reaches Styra as a separate `DetailBlock`, so it
/// cannot otherwise take the Markdown renderer's fenced-code path.
///
/// `None` means the language was absent or unrecognised; callers can then use
/// their ordinary code rendering (including its diagnostics and diff cues).
pub fn syntax_highlighted_code_lines(
    text: &str,
    language: Option<&str>,
    indent: &str,
) -> Option<Vec<Line<'static>>> {
    let language = language?.trim();
    if language.is_empty() {
        return None;
    }
    // Both answers are worth keeping. A language tui-markdown does not know
    // still costs a full Markdown parse to find that out, so a block that
    // falls back is as expensive to re-decide as one that highlights.
    let key = CodeKey {
        text: text.to_owned(),
        language: language.to_owned(),
        indent: indent.to_owned(),
    };
    CODE_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .get_or_insert_with(key, || highlight_code(text, language, indent))
    })
}

/// [`syntax_highlighted_code_lines`] proper, behind its cache.
fn highlight_code(text: &str, language: &str, indent: &str) -> Option<Vec<Line<'static>>> {
    // A four-backtick wrapper also permits source which itself contains a
    // normal three-backtick fence.
    let source = format!("````{language}\n{text}\n````");
    let options = tui_markdown::Options::new(StyraStyleSheet)
        .code_theme(CodeTheme::clone(&MARKDOWN_CODE_THEME));
    let rendered = tui_markdown::from_str_with_options(&source, &options);

    // tui-markdown deliberately falls back to one plain span for an unknown
    // token.  Do not replace Styra's established fallback rendering in that
    // case.
    let highlighted = rendered.lines.iter().any(|line| line.spans.len() > 1);
    highlighted.then(|| {
        rendered
            .lines
            .into_iter()
            .map(|line| {
                let line_style = line.style;
                let mut spans = vec![Span::styled(
                    indent.to_owned(),
                    Style::default().fg(palette::TEXT),
                )];
                spans.extend(
                    line.spans
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style)),
                );
                Line::from(spans).style(line_style)
            })
            .collect()
    })
}

/// As [`markdown_block_lines_with_links`], drawing one entry as selected.
///
/// An entry is a link — which is also how agents write a file reference, as
/// `[app.rs:120](/home/me/src/app.rs:120)` — so highlighting entry `n` puts a
/// selection over the nth citation of the block. Entries are counted in
/// reading order regardless of [`LinkDisplay`], so a selection does not jump
/// when the operator toggles destinations on; what the selection covers does,
/// since in [`LinkDisplay::Full`] the destination is part of what is on
/// screen.
///
/// A `highlight` past the last entry simply highlights nothing, which is what
/// a caller rendering a block that has since lost its citations wants.
pub fn markdown_block_render(
    text: &str,
    base_style: Style,
    indent: &str,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
) -> BlockRender {
    let key = BlockKey {
        text: text.to_owned(),
        base_style,
        indent: indent.to_owned(),
        links,
        highlight,
    };
    BLOCK_CACHE.with(|cache| {
        cache.borrow_mut().get_or_insert_with(key, || {
            render_markdown_block(text, base_style, indent, links, highlight)
        })
    })
}

/// [`markdown_block_render`] proper, behind its cache.
fn render_markdown_block(
    text: &str,
    base_style: Style,
    indent: &str,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
) -> BlockRender {
    let normalized = force_hard_line_breaks(text);
    let options = tui_markdown::Options::new(StyraStyleSheet)
        .code_theme(CodeTheme::clone(&MARKDOWN_CODE_THEME));
    let rendered = tui_markdown::from_str_with_options(&normalized, &options);
    let mut entries = 0;
    let lines = rendered
        .lines
        .into_iter()
        .map(|line| {
            // tui-markdown puts some styling (heading color, blockquote color,
            // table borders) on the Line itself rather than on every Span, so
            // that base style has to be carried over explicitly.
            let line_style = line.style;
            let mut spans = vec![Span::styled(indent.to_owned(), base_style)];
            let rendered_spans = render_links(line.spans, links, &mut entries, highlight);
            spans.extend(rendered_spans.into_iter().enumerate().map(|(i, span)| {
                let mut content = span.content.into_owned();
                // tui-markdown has no hook to customize the unordered-list
                // marker, so the "- " it hardcodes is swapped for a bullet
                // glyph here to match Styra's established look.
                if i == 0 {
                    if let Some(bulleted) = bulletize(&content) {
                        content = bulleted;
                    }
                }
                Span::styled(content, span.style)
            }));
            Line::from(spans).style(line_style)
        })
        .collect();
    BlockRender { lines, entries }
}

/// The selection drawn over a highlighted entry.
///
/// Yellow is what Styra already marks a selection with (see
/// [`palette::SELECTION_MARKER`]), and filling the entry rather than tinting
/// its text keeps it visible on the selected row too, whose own background is
/// already [`palette::SELECTION_BACKGROUND`].
fn entry_highlight_style() -> Style {
    Style::new()
        .fg(palette::SELECTION_BACKGROUND)
        .bg(palette::SELECTION_MARKER)
}

/// Drops the destination `tui-markdown` appends to every link, leaving the
/// title — unless the operator asked to see them ([`LinkDisplay::Full`]) —
/// and puts the selection over the entry `highlight` names.
///
/// `entries` counts the entries seen so far, and is advanced past the ones on
/// this line: a block is numbered continuously, but `tui-markdown` hands its
/// lines over one at a time.
///
/// `tui-markdown` renders a link as `label (destination)`, and agents cite
/// their work as links: a reply reads `app.rs:120 (/home/me/src/app.rs:120)`,
/// saying the same thing twice and at twice the width. What a citation points
/// at is not lost with the destination — the references modal (`F`) still opens
/// it; see [`crate::references`].
///
/// A link that wrote no label keeps its destination, since collapsing it would
/// leave nothing on screen at all.
fn render_links<'a>(
    mut spans: Vec<Span<'a>>,
    links: LinkDisplay,
    entries: &mut usize,
    highlight: Option<EntryIndex>,
) -> Vec<Span<'a>> {
    // `tui-markdown` emits a link's destination as the three spans " (", the
    // destination under the link style, and ")", directly after the label —
    // which carries that same style, plus whatever emphasis it was written
    // with. Nothing else in a rendered line is styled as a link, so the shape
    // identifies a destination rather than parenthesised prose.
    let mut appended = vec![false; spans.len()];
    let mut selected = vec![false; spans.len()];
    for index in 1..spans.len().saturating_sub(2) {
        if !(spans[index].content == " ("
            && spans[index + 2].content == ")"
            && spans[index + 1].style == StyraStyleSheet.link()
            && is_link_label(&spans[index - 1]))
        {
            continue;
        }
        appended[index] = true;
        appended[index + 1] = true;
        appended[index + 2] = true;
        // One entry per destination, so two links written back to back stay
        // two entries even once their destinations are dropped.
        let entry = *entries;
        *entries += 1;
        if highlight != Some(entry) {
            continue;
        }
        // The label is however many spans the emphasis inside it was split
        // into; in `Full` the destination is on screen too, and the selection
        // covers what is on screen.
        let mut first = index;
        while first > 0 && is_link_span(&spans[first - 1]) {
            first -= 1;
        }
        let last = if links == LinkDisplay::Full {
            index + 2
        } else {
            index.saturating_sub(1)
        };
        for span in selected.iter_mut().take(last + 1).skip(first) {
            *span = true;
        }
    }
    for (span, selected) in spans.iter_mut().zip(&selected) {
        if *selected {
            span.style = span.style.patch(entry_highlight_style());
        }
    }
    if links == LinkDisplay::Full {
        return spans;
    }
    spans
        .into_iter()
        .zip(appended)
        .filter_map(|(span, appended)| (!appended).then_some(span))
        .collect()
}

/// Whether `span` is part of a link's label, rather than text that merely
/// happens to be underlined — a level-one heading is too.
fn is_link_span(span: &Span<'_>) -> bool {
    is_link_label(span) && span.style.fg == StyraStyleSheet.link().fg
}

/// Whether `span` could be the label of the link whose destination follows it.
///
/// Underlining is what the link style contributes that survives any emphasis,
/// heading, or inline code the label was written inside — so a `(destination)`
/// preceded by ordinary text belongs to a link that wrote no label.
fn is_link_label(span: &Span<'_>) -> bool {
    span.style.add_modifier.contains(Modifier::UNDERLINED)
}

/// The column a rendered Markdown line's continuation rows should be indented
/// to when it has to wrap.
///
/// Everything wraps at the pane edge — nothing is worth hiding off-screen —
/// but a line whose structure carries meaning should not lose it in the wrap.
/// A list item's continuation is aligned under its own text, so the marker
/// column stays free and the next bullet still reads as a bullet; a table row
/// is aligned under its left border. Flowing prose has no such column and
/// falls back to the caller's own indent.
pub fn structural_indent(line: &Line<'_>) -> Option<usize> {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let trimmed = text.trim_start();
    let leading = text.chars().count() - trimmed.chars().count();
    if is_table_row(trimmed) {
        return Some(leading);
    }
    list_marker_width(trimmed).map(|marker| leading + marker)
}

/// Table rows and borders as `tui-markdown` draws them: box-drawing glyphs.
fn is_table_row(trimmed: &str) -> bool {
    trimmed.starts_with([
        '\u{2502}', '\u{250c}', '\u{251c}', '\u{2514}', '\u{252c}', '\u{253c}', '\u{2534}',
        '\u{2500}', '|',
    ])
}

/// The width of a leading list marker — a bullet (as [`bulletize`] rewrites
/// it, or a raw `-`/`*`/`+`) or an ordered marker such as `1.` / `2)` —
/// including the space that follows it.
fn list_marker_width(trimmed: &str) -> Option<usize> {
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if matches!(first, '\u{2022}' | '-' | '*' | '+') {
        // Columns, not bytes: the bullet glyph is one column wide.
        return (chars.next() == Some(' ')).then_some(2);
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &trimmed[digits..];
    match rest.strip_prefix(['.', ')']) {
        Some(after) if after.starts_with(' ') => Some(digits + 2),
        _ => None,
    }
}

fn bulletize(content: &str) -> Option<String> {
    let indent = content.strip_suffix("- ")?;
    indent
        .chars()
        .all(|c| c == ' ')
        .then(|| format!("{indent}\u{2022} "))
}

/// Agent messages commonly separate paragraphs with a single `\n` rather
/// than a blank line, but CommonMark treats a single newline inside a
/// paragraph as a soft break that collapses to a space. Force it into a hard
/// break instead, so plain prose keeps one rendered line per source line.
/// Code fences and table rows are left untouched since their line structure
/// is already meaningful.
fn force_hard_line_breaks(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut fenced = vec![false; lines.len()];
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced[i] = true;
            in_fence = !in_fence;
        } else {
            fenced[i] = in_fence;
        }
    }

    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 < lines.len() {
            let trimmed = line.trim();
            let next_is_blank = lines[i + 1].trim().is_empty();
            let is_table_row = trimmed.starts_with('|');
            if !fenced[i] && !is_table_row && !trimmed.is_empty() && !next_is_blank {
                out.push_str("  ");
            }
            out.push('\n');
        }
    }
    out
}

/// Styra's palette overrides for `tui-markdown`'s default style sheet.
///
/// Every colored Markdown construct is mapped onto Styra's palette. The
/// leading heading marker is omitted because pretty mode strips Markdown
/// syntax rather than showing it styled.
#[derive(Clone, Copy, Debug, Default)]
struct StyraStyleSheet;

impl StyleSheet for StyraStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::new()
                .fg(palette::TEXT)
                .bg(palette::ACCENT)
                .bold()
                .underlined(),
            2 => Style::new().fg(palette::ACCENT).bold(),
            3 => Style::new().fg(palette::ACCENT).bold().italic(),
            _ => Style::new().fg(palette::LIGHT_ACCENT).italic(),
        }
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code(&self) -> Style {
        Style::new()
            .fg(palette::WARNING)
            .bg(palette::CODE_BACKGROUND)
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn link(&self) -> Style {
        Style::new().fg(palette::INFO).underlined()
    }

    fn blockquote(&self) -> Style {
        Style::new().fg(palette::SUCCESS)
    }

    fn heading_meta(&self) -> Style {
        Style::new().fg(palette::ADDITIONAL_INFO).dim()
    }

    fn metadata_block(&self) -> Style {
        Style::new().fg(palette::MUTED_WARNING)
    }

    fn html(&self) -> Style {
        Style::new().fg(palette::ADDITIONAL_INFO).dim()
    }

    fn math_inline(&self) -> Style {
        Style::new().fg(palette::SPECIAL).italic()
    }

    fn math_display(&self) -> Style {
        Style::new().fg(palette::SPECIAL)
    }

    fn footnote_ref(&self) -> Style {
        Style::new().fg(palette::ADDITIONAL_INFO).dim().italic()
    }

    fn footnote_def(&self) -> Style {
        Style::new().fg(palette::ADDITIONAL_INFO).dim()
    }

    fn alert(&self, kind: AlertKind) -> Style {
        let color = match kind {
            AlertKind::Note => palette::INFO,
            AlertKind::Tip => palette::SUCCESS,
            AlertKind::Important => palette::SPECIAL,
            AlertKind::Warning => palette::WARNING,
            AlertKind::Caution => palette::ERROR,
        };
        Style::new().fg(color)
    }

    fn table_header(&self) -> Style {
        Style::new().fg(palette::ACCENT).bold()
    }

    fn table_border(&self) -> Style {
        Style::new().fg(palette::INACTIVE)
    }

    fn image_alt(&self) -> Style {
        Style::new().fg(palette::ADDITIONAL_INFO).dim().italic()
    }
}

/// Renders inline Markdown used in compact, single-line event summaries.
pub fn parse_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let spans = render_spans(text, base_style);
    if spans.is_empty() {
        vec![Span::styled(String::new(), base_style)]
    } else {
        spans
    }
}

fn render_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    let mut spans = Vec::new();
    let mut styles = vec![base_style];

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Strong => styles.push(current_style(&styles).add_modifier(Modifier::BOLD)),
                Tag::Emphasis => styles.push(current_style(&styles).add_modifier(Modifier::ITALIC)),
                Tag::Strikethrough => {
                    styles.push(current_style(&styles).add_modifier(Modifier::CROSSED_OUT))
                }
                _ => {}
            },
            Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough) => {
                if styles.len() > 1 {
                    styles.pop();
                }
            }
            Event::End(_) => {}
            Event::Text(text) => {
                spans.push(Span::styled(text.into_string(), current_style(&styles)))
            }
            Event::Code(code) => spans.push(Span::styled(
                code.into_string(),
                current_style(&styles).fg(palette::WARNING),
            )),
            Event::SoftBreak | Event::HardBreak => {
                spans.push(Span::styled(" ", current_style(&styles)))
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                spans.push(Span::styled(html.into_string(), current_style(&styles)))
            }
            _ => {}
        }
    }
    spans
}

fn current_style(styles: &[Style]) -> Style {
    styles.last().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
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
        let base = Style::default().fg(palette::TEXT);
        let lines = markdown_block_lines("# Title", base, "  ");

        assert_eq!(rendered_line(&lines[0]), "  Title");
        assert_eq!(lines[0].style.bg, Some(palette::ACCENT));
        assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
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
            .filter(|span| span.style.bg == Some(palette::SELECTION_MARKER))
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
                .filter(|span| span.style.bg == Some(palette::SELECTION_MARKER))
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
            .all(|span| span.style.bg != Some(palette::SELECTION_MARKER)));
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
            .filter(|span| span.style.bg == Some(palette::SELECTION_MARKER))
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
    fn block_lines_keep_ordered_list_numbering() {
        let base = Style::default();
        let lines = markdown_block_lines("1. first\n2. second", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line.starts_with("1. first")));
        assert!(rendered.iter().any(|line| line.starts_with("2. second")));
    }

    #[test]
    fn block_lines_use_a_bullet_glyph_for_unordered_items() {
        let base = Style::default();
        let lines = markdown_block_lines("- one\n- two", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line.starts_with("\u{2022} one")));
        assert!(rendered.iter().any(|line| line.starts_with("\u{2022} two")));
        assert!(!rendered.iter().any(|line| line.contains("- ")));
    }

    #[test]
    fn block_lines_keep_one_line_per_source_line_without_blank_separators() {
        // Agent messages routinely separate lines with a single `\n`, not a
        // blank line. CommonMark's soft-break-to-space rule would otherwise
        // silently merge them into one rendered line.
        let base = Style::default();
        let lines = markdown_block_lines("hello\nworld", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert_eq!(rendered, vec!["hello".to_owned(), "world".to_owned()]);
    }

    #[test]
    fn block_lines_leave_fenced_code_content_untouched() {
        let base = Style::default();
        let lines = markdown_block_lines("```\nfn f() {}\n```", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line == "fn f() {}"));
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

    #[test]
    fn fenced_code_highlighting_uses_the_embedded_palette_theme() {
        let lines = markdown_block_lines("```rust\nfn main() {}\n```", Style::default(), "");
        let keyword = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == "fn")
            .expect("highlighted Rust keyword");

        assert_eq!(keyword.style.fg, Some(palette::MARKDOWN_CODE_KEYWORD));
    }

    /// Rendering is memoized (see [`crate::render_cache`]), so what is asked
    /// for twice has to come back the same both times — and, more to the
    /// point, what differs only in a display choice must not come back as the
    /// rendering made under the other one.
    #[test]
    fn a_cached_block_is_not_reused_for_a_different_display_choice() {
        let text = "see [app.rs:120](/home/me/src/app.rs:120)";
        let base = Style::default().fg(palette::TEXT);

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
        let indented =
            markdown_block_lines_with_links(text, base, "        ", LinkDisplay::Compact);
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
}
