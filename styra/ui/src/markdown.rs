//! Markdown-to-Ratatui styling for agent messages.
//!
//! Everything is rendered by `tui-markdown`, which parses a whole buffer at
//! once and so can render tables, code fences, and other multi-line
//! constructs correctly. A single-line summary is the same rendering with its
//! lines laid end to end, so a row and its expanded body never disagree.

use crate::theme;
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
    CodeTheme::from_textmate(theme::MARKDOWN_CODE_THEME)
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

/// How many Markdown links a block contains, in reading order.
///
/// This is the same numbering used by [`markdown_block_render`], allowing a
/// caller to keep one selection while the links live in separate messages.
pub fn markdown_link_count(text: &str) -> usize {
    markdown_block_render(text, Style::default(), "", LinkDisplay::Compact, None).entries
}

/// Destination of the `index`th entry, in the same reading order as
/// [`markdown_link_count`] and [`markdown_block_render`]: a link's
/// destination, or the text of a code span that names a file.
pub fn markdown_link_destination(text: &str, index: EntryIndex) -> Option<String> {
    let mut depth = 0usize;
    Parser::new_ext(text, Options::all())
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                depth += 1;
                Some(dest_url.into_string())
            }
            Event::End(TagEnd::Link) => {
                depth = depth.saturating_sub(1);
                None
            }
            // A code span inside a link is its label; the link is the entry.
            Event::Code(code) if depth == 0 && is_file_reference(&code) => Some(code.into_string()),
            _ => None,
        })
        .nth(index)
}

/// Whether an inline code span reads as a file reference — `src/app.rs`,
/// `app.rs:120`, `lib/mod.rs:7:3` — rather than code.
///
/// Agents cite files in backticks as often as they write links, and a
/// citation should be selectable however it was written. What rules out code
/// is its punctuation: a call, a path in `::`, a URL, or anything with a space
/// is not a filename. A dotted name with no slash must end in a short,
/// letter-led extension, so `Cargo.toml` is a file while `1.2.3` and
/// `config.default_model` are not. A short field access such as `self.spec`
/// still passes; selecting it only opens a file that does not exist.
pub fn is_file_reference(code: &str) -> bool {
    let mut path = code;
    // At most `:line:column`, the same suffix link destinations may carry.
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((before, digits))
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                path = before;
            }
            _ => break,
        }
    }
    if path.is_empty()
        || !path.chars().any(|ch| ch.is_ascii_alphabetic())
        || !path.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '~' | '+' | '@')
        })
    {
        return false;
    }
    if path.contains('/') {
        return true;
    }
    // A dotfile (`.gitignore`) is all extension, so its length says nothing.
    path.rsplit_once('.').is_some_and(|(stem, extension)| {
        (stem.is_empty() || (1..=5).contains(&extension.len()))
            && !extension.is_empty()
            && extension.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    })
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
                    Style::default().fg(theme::TEXT),
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
                // Plain text comes out of tui-markdown with no colour of its
                // own, which would leave it in the terminal's default
                // foreground rather than the entry's text colour.
                Span::styled(content, base_style.patch(line_style).patch(span.style))
            }));
            Line::from(spans).style(line_style)
        })
        .collect();
    BlockRender { lines, entries }
}

/// The selection drawn over a highlighted entry.
///
/// A dusty rose fill stays visible on the selected row too, whose own
/// background is already [`theme::SELECTION_BACKGROUND`], without competing
/// with the bright-yellow row cursor.
fn entry_highlight_style() -> Style {
    Style::new()
        .fg(theme::TEXT)
        .bg(theme::LINK_HIGHLIGHT_BACKGROUND)
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
/// at is not lost with the destination — conversation link navigation (`F`)
/// selects it by this same reading-order index.
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
    // Every entry is underlined, as a link already is, which is what
    // [`is_entry_style`] recognises it by once it is on screen.
    let mut underlined = vec![false; spans.len()];
    let is_destination = |index: usize| {
        index >= 1
            && index + 2 < spans.len()
            && spans[index].content == " ("
            && spans[index + 2].content == ")"
            && spans[index + 1].style == StyraStyleSheet.link()
            && is_link_label(&spans[index - 1])
    };
    for index in 0..spans.len() {
        // A code span that names a file is an entry of its own, unless it is
        // the label of the link whose destination follows it.
        if is_code_span(&spans[index])
            && is_file_reference(&spans[index].content)
            && !is_destination(index + 1)
        {
            underlined[index] = true;
            let entry = *entries;
            *entries += 1;
            if highlight == Some(entry) {
                selected[index] = true;
            }
            continue;
        }
        if !is_destination(index) {
            continue;
        }
        underlined[index - 1] = true;
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
        // into — or the one code span it was written as, which the code style
        // replaces the link style on; in `Full` the destination is on screen
        // too, and the selection covers what is on screen.
        let mut first = index;
        while first > 0 && is_link_span(&spans[first - 1]) {
            first -= 1;
        }
        if first == index && is_code_span(&spans[index - 1]) {
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
    for ((span, selected), underlined) in spans.iter_mut().zip(&selected).zip(&underlined) {
        if *underlined {
            if is_code_span(span) {
                span.style = span
                    .style
                    .fg(theme::ENTRY_CODE)
                    .bg(theme::ENTRY_CODE_BACKGROUND);
            }
            span.style = span.style.add_modifier(Modifier::UNDERLINED);
        }
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
/// preceded by ordinary text belongs to a link that wrote no label. The
/// exception is a label written as one code span, `` [`app.rs`](/src/app.rs) ``,
/// which `tui-markdown` draws in the code style alone.
fn is_link_label(span: &Span<'_>) -> bool {
    span.style.add_modifier.contains(Modifier::UNDERLINED) || is_code_span(span)
}

/// Whether `span` is inline code as `tui-markdown` draws it. Fenced code is
/// never drawn this way, so a filename on a line of its own in a code block
/// is not mistaken for a citation.
fn is_code_span(span: &Span<'_>) -> bool {
    let code = StyraStyleSheet.code();
    span.style.fg == code.fg && span.style.bg == code.bg
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

/// Styra's theme overrides for `tui-markdown`'s default style sheet.
///
/// Every colored Markdown construct is mapped onto Styra's theme. The
/// leading heading marker is omitted because pretty mode strips Markdown
/// syntax rather than showing it styled.
#[derive(Clone, Copy, Debug, Default)]
struct StyraStyleSheet;

impl StyleSheet for StyraStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::new().fg(theme::MARKDOWN_HEADING).bold().underlined(),
            2 => Style::new().fg(theme::MARKDOWN_HEADING).bold(),
            3 => Style::new().fg(theme::ACCENT).bold().italic(),
            _ => Style::new().fg(theme::LIGHT_ACCENT).italic(),
        }
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code(&self) -> Style {
        Style::new()
            .fg(theme::INLINE_CODE)
            .bg(theme::INLINE_CODE_BACKGROUND)
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn link(&self) -> Style {
        Style::new().fg(theme::MARKDOWN_LINK).underlined()
    }

    fn blockquote(&self) -> Style {
        Style::new().fg(theme::MARKDOWN_QUOTE).italic()
    }

    fn heading_meta(&self) -> Style {
        Style::new().fg(theme::ADDITIONAL_INFO).dim()
    }

    fn metadata_block(&self) -> Style {
        Style::new().fg(theme::MUTED_WARNING)
    }

    fn html(&self) -> Style {
        Style::new().fg(theme::ADDITIONAL_INFO).dim()
    }

    fn math_inline(&self) -> Style {
        Style::new().fg(theme::SPECIAL).italic()
    }

    fn math_display(&self) -> Style {
        Style::new().fg(theme::SPECIAL)
    }

    fn footnote_ref(&self) -> Style {
        Style::new().fg(theme::ADDITIONAL_INFO).dim().italic()
    }

    fn footnote_def(&self) -> Style {
        Style::new().fg(theme::ADDITIONAL_INFO).dim()
    }

    fn alert(&self, kind: AlertKind) -> Style {
        let color = match kind {
            AlertKind::Note => theme::INFO,
            AlertKind::Tip => theme::SUCCESS,
            AlertKind::Important => theme::SPECIAL,
            AlertKind::Warning => theme::WARNING,
            AlertKind::Caution => theme::ERROR,
        };
        Style::new().fg(color)
    }

    fn table_header(&self) -> Style {
        Style::new().fg(theme::ACCENT).bold()
    }

    fn table_border(&self) -> Style {
        Style::new().fg(theme::INACTIVE)
    }

    fn image_alt(&self) -> Style {
        Style::new().fg(theme::ADDITIONAL_INFO).dim().italic()
    }
}

/// Renders inline Markdown used in compact, single-line event summaries.
pub fn parse_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    parse_inline_spans_with_highlight(text, base_style, None)
}

/// Renders inline Markdown and marks one link, when a compact event summary
/// owns the visible copy of that link.
pub fn parse_inline_spans_with_highlight(
    text: &str,
    base_style: Style,
    highlight: Option<EntryIndex>,
) -> Vec<Span<'static>> {
    // The summary is the block rendering laid end to end, so a row and its
    // expanded body style and number their entries identically. The block is
    // rendered on the default style, which keeps it the same cache entry as
    // [`markdown_link_count`]'s, and `base_style` is laid under it here.
    let render = markdown_block_render(text, Style::default(), "", LinkDisplay::Compact, highlight);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for line in render.lines {
        if line.spans.iter().all(|span| span.content.trim().is_empty()) {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(" ", base_style));
        }
        let line_style = base_style.patch(line.style);
        spans.extend(
            line.spans
                .into_iter()
                .filter(|span| !span.content.is_empty())
                .map(|span| Span::styled(span.content, line_style.patch(span.style))),
        );
    }
    if spans.is_empty() {
        vec![Span::styled(String::new(), base_style)]
    } else {
        spans
    }
}

/// Whether a drawn cell belongs to an entry — a link or a file reference —
/// so that link navigation can wash out everything else on screen.
///
/// Entries are the only underlined text Styra draws, bar a level-one heading,
/// which is told apart by the accent fill it is drawn on.
pub fn is_entry_style(style: Style) -> bool {
    style.bg == Some(theme::LINK_HIGHLIGHT_BACKGROUND)
        || (style.add_modifier.contains(Modifier::UNDERLINED)
            && style.bg != StyraStyleSheet.heading(1).bg)
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
            for span in line.spans.iter().filter(|span| !span.content.trim().is_empty()) {
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
