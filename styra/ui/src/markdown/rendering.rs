use super::*;
use crate::theme;
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
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::cell::RefCell;
use std::sync::LazyLock;
use tui_markdown::{AlertKind, CodeTheme, StyleSheet};
use two_face::re_exports::syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
};

static MARKDOWN_CODE_THEME: LazyLock<CodeTheme> = LazyLock::new(|| {
    CodeTheme::from_textmate(theme::MARKDOWN_CODE_THEME)
        .expect("Styra's embedded Markdown code theme must be valid")
});

// tui-markdown's built-in syntax set does not include Elixir and cannot be
// extended. Use the bat grammar bundle for it, with the same TextMate theme.
static ELIXIR_SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_no_newlines);
static ELIXIR_CODE_THEME: LazyLock<Theme> = LazyLock::new(|| {
    ThemeSet::load_from_reader(&mut std::io::Cursor::new(theme::MARKDOWN_CODE_THEME))
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

/// Syntax-highlights a standalone code block when `language` is known to
/// tui-markdown or the supplemental Elixir grammar. Code reaches Styra as a
/// separate `DetailBlock`, so it cannot otherwise take the Markdown renderer's
/// fenced-code path.
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
    // Match plain code rendering: each tab occupies four spaces. Expand before
    // either highlighter so tabs cannot collapse when Ratatui draws the spans.
    let text = text.replace('\t', "    ");
    if matches!(
        language.to_ascii_lowercase().as_str(),
        "elixir" | "ex" | "exs"
    ) {
        return highlight_elixir(&text, indent);
    }
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

fn highlight_elixir(text: &str, indent: &str) -> Option<Vec<Line<'static>>> {
    let syntax = ELIXIR_SYNTAXES.find_syntax_by_name("Elixir")?;
    let mut highlighter = HighlightLines::new(syntax, &ELIXIR_CODE_THEME);
    text.split('\n')
        .map(|line| {
            let tokens = highlighter.highlight_line(line, &ELIXIR_SYNTAXES).ok()?;
            let mut spans = vec![Span::styled(
                indent.to_owned(),
                Style::default().fg(theme::TEXT),
            )];
            spans.extend(tokens.into_iter().map(|(style, token)| {
                let mut modifiers = Modifier::empty();
                if style.font_style.contains(FontStyle::BOLD) {
                    modifiers |= Modifier::BOLD;
                }
                if style.font_style.contains(FontStyle::ITALIC) {
                    modifiers |= Modifier::ITALIC;
                }
                if style.font_style.contains(FontStyle::UNDERLINE) {
                    modifiers |= Modifier::UNDERLINED;
                }
                Span::styled(
                    token.to_owned(),
                    Style::default()
                        .fg(ratatui::style::Color::Rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        ))
                        .add_modifier(modifiers),
                )
            }));
            Some(Line::from(spans))
        })
        .collect()
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
    let leading = crate::text::width(&text[..text.len() - trimmed.len()]);
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
mod tests;

#[cfg(test)]
mod boundary_tests;
