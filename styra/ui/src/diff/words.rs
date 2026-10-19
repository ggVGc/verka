//! Intraline changes, calculated from source text before tabs or styles expand it.
use super::*;

/// Overlay word changes on a complete, noncompact diff's rendered lines.
/// Headers and unmatched additions/removals retain their ordinary line bands.
pub fn highlight_words(text: &str, rendered: &mut [Line<'static>]) {
    // Retain source-coordinate masks across layout, tint, and viewport changes.
    thread_local! {
        static LAST: std::cell::RefCell<Option<(String, Vec<Accent>)>> = const { std::cell::RefCell::new(None) };
    }
    let accents = LAST.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((previous, accents)) = cache.as_ref() {
            if previous == text {
                return accents.clone();
            }
        }
        let accents = word_changes(text);
        if text.len() <= 1_000_000 {
            *cache = Some((text.to_owned(), accents.clone()));
        }
        accents
    });
    let source: Vec<_> = text.lines().collect();
    for (index, changed, background) in accents {
        if let Some(line) = rendered.get_mut(index) {
            overlay(line, &source[index][1..], &changed, background);
        }
    }
}

type Accent = (usize, Vec<bool>, Color);

fn word_changes(text: &str) -> Vec<Accent> {
    let mut accents = Vec::new();
    let source: Vec<_> = text.lines().collect();
    let mut old = Vec::new();
    let mut new = Vec::new();
    let mut in_hunk = false;
    let mut budget = 2_000_000usize;
    let mut flush = |old: &mut Vec<usize>, new: &mut Vec<usize>| {
        // Monotone sequence matching lets inserted/deleted lines stay unmatched.
        if old.len() <= 64 && new.len() <= 64 {
            let mut matches = vec![vec![None; new.len()]; old.len()];
            for (i, &a) in old.iter().enumerate() {
                for (j, &b) in new.iter().enumerate() {
                    matches[i][j] = changes(&source[a][1..], &source[b][1..], &mut budget);
                }
            }
            let mut scores = vec![vec![0; new.len() + 1]; old.len() + 1];
            for i in (0..old.len()).rev() {
                for j in (0..new.len()).rev() {
                    let paired = matches[i][j]
                        .as_ref()
                        .map_or(0, |m| m.score + scores[i + 1][j + 1]);
                    scores[i][j] = paired.max(scores[i + 1][j]).max(scores[i][j + 1]);
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < old.len() && j < new.len() {
                if let Some(m) = &matches[i][j] {
                    if scores[i][j] == m.score + scores[i + 1][j + 1] {
                        for (index, changed, background) in [
                            (old[i], &m.old, theme::DIFF_WORD_REMOVED_BACKGROUND),
                            (new[j], &m.new, theme::DIFF_WORD_ADDED_BACKGROUND),
                        ] {
                            accents.push((index, changed.clone(), background));
                        }
                        i += 1;
                        j += 1;
                        continue;
                    }
                }
                if scores[i + 1][j] >= scores[i][j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        old.clear();
        new.clear();
    };
    for (index, line) in source.iter().enumerate() {
        if line.starts_with("@@") {
            flush(&mut old, &mut new);
            in_hunk = true;
        } else if line.starts_with("diff --git ") {
            flush(&mut old, &mut new);
            in_hunk = false;
        } else if in_hunk && line.starts_with('-') {
            old.push(index);
        } else if in_hunk && line.starts_with('+') {
            new.push(index);
        } else if !line.starts_with("\\ No newline") {
            flush(&mut old, &mut new);
        }
    }
    flush(&mut old, &mut new);
    accents
}

#[derive(Clone)]
struct Changes {
    old: Vec<bool>,
    new: Vec<bool>,
    score: usize,
}

// Word tokens establish stable anchors; character refinement within each edit
// keeps identifier suffixes and numeric substitutions precise.
fn tokens(chars: &[char]) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start + 1;
        if chars[start].is_alphanumeric() || chars[start] == '_' {
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
        }
        result.push((start, end));
        start = end;
    }
    result
}

fn lcs<T: PartialEq>(a: &[T], b: &[T], budget: &mut usize) -> Option<Vec<(usize, usize)>> {
    let cost = (a.len() + 1).checked_mul(b.len() + 1)?;
    if cost > 65_536 || cost > *budget {
        return None;
    }
    *budget -= cost;
    let stride = b.len() + 1;
    let mut scores = vec![0u16; cost];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            scores[i * stride + j] = if a[i] == b[j] {
                1 + scores[(i + 1) * stride + j + 1]
            } else {
                scores[(i + 1) * stride + j].max(scores[i * stride + j + 1])
            };
        }
    }
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            result.push((i, j));
            i += 1;
            j += 1;
        } else if scores[(i + 1) * stride + j] >= scores[i * stride + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some(result)
}

fn changes(a: &str, b: &str, budget: &mut usize) -> Option<Changes> {
    // Bound both temporary memory and work for generated/minified files.
    if a.len() > 4096 || b.len() > 4096 {
        return None;
    }
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    let ta = tokens(&a);
    let tb = tokens(&b);
    let aa: Vec<_> = ta.iter().map(|&(s, e)| &a[s..e]).collect();
    let bb: Vec<_> = tb.iter().map(|&(s, e)| &b[s..e]).collect();
    let anchors = lcs(&aa, &bb, budget)?;
    let mut old = vec![true; a.len()];
    let mut new = vec![true; b.len()];
    let (mut pa, mut pb) = (0, 0);
    for (ia, ib) in anchors
        .into_iter()
        .chain(std::iter::once((ta.len(), tb.len())))
    {
        let (sa, ea) = ta.get(ia).copied().unwrap_or((a.len(), a.len()));
        let (sb, eb) = tb.get(ib).copied().unwrap_or((b.len(), b.len()));
        let prefix = a[pa..sa]
            .iter()
            .zip(&b[pb..sb])
            .take_while(|(x, y)| x == y)
            .count();
        let suffix = a[pa + prefix..sa]
            .iter()
            .rev()
            .zip(b[pb + prefix..sb].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        old[pa..pa + prefix].fill(false);
        new[pb..pb + prefix].fill(false);
        old[sa - suffix..sa].fill(false);
        new[sb - suffix..sb].fill(false);
        for (x, y) in lcs(
            &a[pa + prefix..sa - suffix],
            &b[pb + prefix..sb - suffix],
            budget,
        )? {
            old[pa + prefix + x] = false;
            new[pb + prefix + y] = false;
        }
        old[sa..ea].fill(false);
        new[sb..eb].fill(false);
        pa = ea;
        pb = eb;
    }
    let common = a
        .iter()
        .zip(&old)
        .filter(|(c, changed)| !**changed && !c.is_whitespace())
        .count();
    let total = a.iter().filter(|c| !c.is_whitespace()).count()
        + b.iter().filter(|c| !c.is_whitespace()).count();
    if total > 0 && (common == 0 || common * 2 * 100 < total * 60) {
        return None;
    }
    Some(Changes {
        old,
        new,
        score: if total == 0 {
            100
        } else {
            common * 200 / total
        },
    })
}

fn overlay(line: &mut Line<'static>, source: &str, changed: &[bool], background: Color) {
    // A rendered code line has an optional number span, then the sign span.
    let sign = usize::from(has_line_number(line));
    let mut mask = Vec::new();
    for (c, &changed) in source.chars().zip(changed) {
        if c == '\t' {
            for n in 0..4 {
                mask.push((changed, if n == 0 { '→' } else { '·' }));
            }
        } else {
            mask.push((changed, if c == ' ' { '·' } else { c }));
        }
    }
    let emphasis = |style: Style| style.bg(background);
    let mut offset = 0;
    let mut spans = Vec::new();
    for (index, span) in std::mem::take(&mut line.spans).into_iter().enumerate() {
        if index <= sign {
            spans.push(span);
            continue;
        }
        let mut content = String::new();
        let mut active = false;
        for c in span.content.chars() {
            let (changed, visible) = mask.get(offset).copied().unwrap_or((false, c));
            offset += 1;
            if !content.is_empty() && changed != active {
                spans.push(Span::styled(
                    std::mem::take(&mut content),
                    if active {
                        emphasis(span.style)
                    } else {
                        span.style
                    },
                ));
            }
            active = changed;
            content.push(if changed { visible } else { c });
        }
        if !content.is_empty() {
            spans.push(Span::styled(
                content,
                if active {
                    emphasis(span.style)
                } else {
                    span.style
                },
            ));
        }
    }
    line.spans = spans;
}

fn has_line_number(line: &Line<'_>) -> bool {
    line.spans
        .first()
        .is_some_and(|s| s.content.trim() != "+" && s.content.trim() != "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn marked(a: &str, b: &str) -> (String, String) {
        let m = changes(a, b, &mut 2_000_000).unwrap();
        (
            a.chars()
                .zip(m.old)
                .filter_map(|(c, yes)| yes.then_some(c))
                .collect(),
            b.chars()
                .zip(m.new)
                .filter_map(|(c, yes)| yes.then_some(c))
                .collect(),
        )
    }
    #[test]
    fn refines_words_numbers_and_unicode() {
        assert_eq!(
            marked("timeout_ms = 30", "timeout_secs = 60"),
            ("m3".into(), "sec6".into())
        );
        assert_eq!(
            marked("let café = 1;", "let café = 2;"),
            ("1".into(), "2".into())
        );
        assert_eq!(
            marked("call(foo, bar)", "call(foo, baz)"),
            ("r".into(), "z".into())
        );
    }
    #[test]
    fn rejects_unrelated_and_bounds_work() {
        assert!(changes("foo", "bar", &mut 10000).is_none());
        assert!(changes("let a = 1", "let a = 2", &mut 1).is_none());
        assert!(changes(&"a".repeat(5000), "a", &mut 10000).is_none());
    }
    #[test]
    fn pairs_past_insertions_and_keeps_line_bands() {
        let text = "@@ -1 +1,2 @@\n-let timeout = 30;\n+unrelated();\n+let timeout = 60;\n";
        let mut lines = diff_block_lines(text, None, false);
        let original = lines.clone();
        highlight_words(text, &mut lines);
        assert!(lines[2]
            .spans
            .iter()
            .all(|s| s.style.bg != Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
        assert!(lines[3]
            .spans
            .iter()
            .any(|s| s.content == "6" && s.style.bg == Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
        for index in [1, 3] {
            let word = lines[index]
                .spans
                .iter()
                .find(|s| {
                    [
                        Some(theme::DIFF_WORD_ADDED_BACKGROUND),
                        Some(theme::DIFF_WORD_REMOVED_BACKGROUND),
                    ]
                    .contains(&s.style.bg)
                })
                .unwrap();
            let original_style = original[index].spans.last().unwrap().style;
            assert_eq!(word.style.fg, original_style.fg);
            assert_eq!(word.style.add_modifier, original_style.add_modifier);
            assert_eq!(word.style.sub_modifier, original_style.sub_modifier);
        }
        fill_changed_rows(&mut lines, 80);
        assert_eq!(
            lines[3].spans.last().unwrap().style.bg,
            Some(theme::DIFF_ADDED_BACKGROUND)
        );
    }
    #[test]
    fn whitespace_tabs_and_hunk_boundaries() {
        assert_eq!(marked("  ", " "), (" ".into(), "".into()));
        let text =
            "@@ -1 +1 @@\n-\tlet a = 1;\n+ let a = 1;\n@@ -3 +3 @@\n-hello\n@@ -5 +5 @@\n+hello!\n";
        let mut lines = diff_block_lines(text, None, false);
        highlight_words(text, &mut lines);
        assert!(lines[1].spans.iter().any(|s| s.content.contains('→')));
        assert!(lines[4]
            .spans
            .iter()
            .all(|s| s.style.bg != Some(theme::DIFF_WORD_REMOVED_BACKGROUND)));
        assert!(lines[6]
            .spans
            .iter()
            .all(|s| s.style.bg != Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
    }
}
