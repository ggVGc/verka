//! Messages attached to files and lines, supplied by the conversation host.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub path: String,
    pub line: Option<u32>,
    pub text: String,
    /// The host's interaction-log entry to select.
    pub entry: usize,
}

impl Prompt {
    /// Read the source footer written when a message is sent from the diff.
    pub fn from_message(text: &str, entry: usize) -> Option<Self> {
        let (text, source) = text.rsplit_once("\n\nSource: ")?;
        if source.is_empty() || source.contains(['\n', '\r']) {
            return None;
        }
        let (path, line) = source
            .rsplit_once(':')
            .and_then(|(path, number)| {
                number
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n > 0)
                    .map(|n| (path, Some(n)))
            })
            .unwrap_or((source, None));
        Some(Self {
            path: path.into(),
            line,
            text: text.into(),
            entry,
        })
    }

    pub fn matches(&self, source: (&str, Option<u32>)) -> bool {
        self.path == source.0 && self.line == source.1
    }
}

#[cfg(test)]
mod tests {
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
}

/// Raw patch rows with annotations. Scan once so drawing a large patch does
/// not repeatedly resolve every row's file and hunk position.
#[cfg(any(test, feature = "renderer"))]
pub(crate) fn marked_rows(text: &str, prompts: &[Prompt], per_file: bool) -> Vec<usize> {
    let mut marked = Vec::new();
    if prompts.is_empty() {
        return marked;
    }
    let mut start = text
        .lines()
        .take_while(|line| !line.starts_with("diff --git "))
        .count();
    for file in crate::files::file_diffs(text) {
        let attached: Vec<_> = prompts.iter().filter(|p| p.path == file.path).collect();
        if !attached.is_empty() {
            if !per_file {
                marked.push(start);
            }
            let mut next = None;
            for (row, line) in file.patch.lines().enumerate() {
                if line.starts_with("@@") {
                    next = line
                        .split_whitespace()
                        .find_map(|part| part.strip_prefix('+'))
                        .and_then(|part| part.split(',').next()?.parse::<u32>().ok());
                } else if let Some(number) = next {
                    if line.starts_with(['+', '-', ' ']) {
                        if attached.iter().any(|p| p.line == Some(number.max(1))) {
                            marked.push(start + row);
                        }
                        if !line.starts_with('-') {
                            next = Some(number.saturating_add(1));
                        }
                    }
                }
            }
        }
        start += file.patch.lines().count();
    }
    marked
}
