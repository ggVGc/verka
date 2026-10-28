//! Shared filename-to-grammar detection for file previews and diffs.

use std::path::Path;

/// Normalize extensions and their aliases, but preserve extensionless names
/// such as `Makefile` and `.bashrc` for the highlighter's filename lookup.
pub(crate) fn from_path(path: &str) -> Option<String> {
    let path = Path::new(path.trim());
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return path.file_name()?.to_str().map(str::to_owned);
    };
    let extension = extension.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" => Some("cpp"),
        "go" => Some("go"),
        "java" => Some("java"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "sql" => Some("sql"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "md" | "mdx" => Some("markdown"),
        "xml" => Some("xml"),
        _ => Some(extension.as_str()),
    }
    .map(str::to_owned)
}
