//! Styling for unified diffs: each added or removed line's sign in green or
//! red, and the code after it highlighted in the language of the file it
//! belongs to.
//!
//! The language comes from the file's path, which a diff names in its own
//! headers (`diff --git`, `+++`) — or does not, as with the snippets
//! reconstructed from Claude's Edit calls, whose path the caller passes in.
//! Each hunk's code is highlighted as one block rather than line by line, so
//! a string or comment spanning lines keeps its colors; removed and added
//! lines are interleaved in that block, which the grammars tolerate well.
//!
//! Code in a language the highlighter does not know keeps the plain
//! rendering: the whole line green or red.

use crate::code::with_gutter;
use crate::markdown::syntax_highlighted_code_lines;
use crate::palette;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render `text` as a diff, each row prefixed with `indent` and the code
/// gutter. `path` names the file for a diff that carries no headers saying
/// so; a header in the diff takes over from it.
///
/// `compact` leaves out what a reader skimming a change does not need:
/// unchanged context and the per-file metadata, keeping the path each file
/// header names and the hunk boundaries.
pub fn diff_block_lines(
    text: &str,
    path: Option<&str>,
    compact: bool,
    indent: &str,
) -> Vec<Line<'static>> {
    diff_body_lines(text, path, compact)
        .into_iter()
        .map(|line| with_gutter(line, indent))
        .collect()
}

/// The diff's rows, without the gutter.
pub(crate) fn diff_body_lines(text: &str, path: Option<&str>, compact: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut language = path.and_then(language_of);
    let mut hunk: Vec<&str> = Vec::new();
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(&mut lines, &mut hunk, language.as_deref());
            in_hunk = false;
            let named = rest
                .rsplit_once(" b/")
                .map(|(_, path)| path)
                .unwrap_or(rest);
            language = language_of(named).or(language);
            lines.push(header(if compact { named } else { line }, palette::ACCENT));
        } else if line.starts_with("@@") {
            flush(&mut lines, &mut hunk, language.as_deref());
            in_hunk = true;
            lines.push(header(line, palette::ACCENT));
        } else if !in_hunk || is_file_marker(line) {
            // Everything between a file header and its first hunk describes
            // the file rather than changing it. A diff with neither header —
            // a bare run of `+`/`-` lines — is all code.
            if let Some(named) = line.strip_prefix("+++ ") {
                let named = named.strip_prefix("b/").unwrap_or(named);
                language = language_of(named).or(language);
            }
            if is_code(line) && !is_file_marker(line) {
                hunk.push(line);
            } else if !compact {
                flush(&mut lines, &mut hunk, language.as_deref());
                lines.push(header(line, palette::TEXT));
            }
        } else if compact && !is_change(line) {
            // Unchanged context, and `\ No newline at end of file`.
        } else {
            hunk.push(line);
        }
    }
    flush(&mut lines, &mut hunk, language.as_deref());
    lines
}

/// The `---`/`+++` lines naming a file's two sides. A removed line whose code
/// begins `-- ` looks the same, but only inside a hunk, and there a file
/// marker always names a side the way git writes it.
fn is_file_marker(line: &str) -> bool {
    ["--- a/", "--- /dev/null", "+++ b/", "+++ /dev/null"]
        .iter()
        .any(|marker| line.starts_with(marker))
}

fn is_change(line: &str) -> bool {
    line.starts_with('+') || line.starts_with('-')
}

/// A line of code in the diff: a change, or unchanged context — which some
/// producers trim to an empty line when the code itself was blank.
fn is_code(line: &str) -> bool {
    is_change(line) || line.starts_with(' ') || line.is_empty()
}

fn header(line: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        line.replace('\t', "    "),
        Style::default().fg(color),
    ))
}

/// The highlighter's name for the language of `path`: its extension, or for
/// an extensionless file its name, which is how grammars such as Makefile's
/// are registered.
fn language_of(path: &str) -> Option<String> {
    let name = path.trim().rsplit('/').next()?;
    let language = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => name,
    };
    (!language.is_empty()).then(|| language.to_owned())
}

/// Render the collected run of code lines and empty it.
fn flush(lines: &mut Vec<Line<'static>>, hunk: &mut Vec<&str>, language: Option<&str>) {
    if hunk.is_empty() {
        return;
    }
    let code: Vec<String> = hunk
        .iter()
        .map(|line| line.get(1..).unwrap_or_default().replace('\t', "    "))
        .collect();
    let highlighted = language
        .and_then(|language| syntax_highlighted_code_lines(&code.join("\n"), Some(language), ""))
        // The highlighter renders one line per line of code; anything else
        // cannot be matched back up with the signs.
        .filter(|highlighted| highlighted.len() == hunk.len());
    match highlighted {
        Some(highlighted) => {
            for (line, code) in hunk.iter().zip(highlighted) {
                let mut spans = vec![sign(line)];
                spans.extend(
                    code.spans
                        .into_iter()
                        .filter(|span| !span.content.is_empty()),
                );
                lines.push(Line::from(spans));
            }
        }
        None => {
            for (line, code) in hunk.iter().zip(code) {
                let color = match line.chars().next() {
                    Some('+') => palette::SUCCESS,
                    Some('-') => palette::ERROR,
                    _ => palette::TEXT,
                };
                lines.push(Line::from(vec![
                    sign(line),
                    Span::styled(code, Style::default().fg(color)),
                ]));
            }
        }
    }
    hunk.clear();
}

/// A code line's first column: green for an addition, red for a removal, and
/// a blank for context.
fn sign(line: &str) -> Span<'static> {
    match line.chars().next() {
        Some('+') => Span::styled(
            "+",
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ),
        Some('-') => Span::styled(
            "-",
            Style::default()
                .fg(palette::ERROR)
                .add_modifier(Modifier::BOLD),
        ),
        _ => Span::raw(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn signs_are_green_and_red_and_the_code_is_highlighted() {
        let lines = diff_body_lines(
            "@@ edit @@\n-fn old() -> u32 { 1 }\n+fn new() -> u32 { 2 }",
            Some("src/lib.rs"),
            false,
        );

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].spans[0].content.as_ref(), "-");
        assert_eq!(lines[1].spans[0].style.fg, Some(palette::ERROR));
        assert_eq!(lines[2].spans[0].content.as_ref(), "+");
        assert_eq!(lines[2].spans[0].style.fg, Some(palette::SUCCESS));
        assert!(lines[2].spans.len() > 3, "tokenized: {:?}", lines[2].spans);
        assert_eq!(text(&lines[2]), "+fn new() -> u32 { 2 }");
    }

    /// Each file in a multi-file diff is highlighted as what it is, which only
    /// its own header says.
    #[test]
    fn the_language_follows_the_file_headers() {
        let lines = diff_body_lines(
            "diff --git a/notes.txt b/notes.txt\n@@\n+plain words here\n\
             diff --git a/src/a.rs b/src/a.rs\n@@\n+let x = \"s\";",
            None,
            true,
        );

        let texts: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            texts,
            [
                "notes.txt",
                "@@",
                "+plain words here",
                "src/a.rs",
                "@@",
                "+let x = \"s\";"
            ]
        );
        // Text has no grammar, so it keeps the plain green line.
        assert_eq!(lines[2].spans[1].style.fg, Some(palette::SUCCESS));
        assert!(lines[5].spans.len() > 3, "{:?}", lines[5].spans);
    }

    #[test]
    fn compact_drops_context_and_file_metadata() {
        let diff = "diff --git a/f.rs b/f.rs\nindex 1..2 100644\n--- a/f.rs\n+++ b/f.rs\n\
                    @@ -1,3 +1,3 @@\n fn keep() {}\n-fn gone() {}\n+fn added() {}";
        let full: Vec<String> = diff_body_lines(diff, None, false)
            .iter()
            .map(text)
            .collect();
        assert_eq!(full.len(), 8);

        let compact: Vec<String> = diff_body_lines(diff, None, true).iter().map(text).collect();
        assert_eq!(
            compact,
            ["f.rs", "@@ -1,3 +1,3 @@", "-fn gone() {}", "+fn added() {}"]
        );
    }

    #[test]
    fn a_removed_line_that_looks_like_a_file_marker_stays_code() {
        let lines = diff_body_lines("@@\n--- a comment\n+x", Some("f.lua"), false);
        assert_eq!(lines[1].spans[0].content.as_ref(), "-");
    }

    #[test]
    fn an_extensionless_file_is_looked_up_by_name() {
        assert_eq!(language_of("build/Makefile").as_deref(), Some("Makefile"));
        assert_eq!(language_of("a/b.test.ts").as_deref(), Some("ts"));
        assert_eq!(language_of(".bashrc").as_deref(), Some(".bashrc"));
    }
}
