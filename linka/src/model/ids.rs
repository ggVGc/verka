//! Validated identifiers: node and candidate ids, and project-relative paths.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A validated string identifier: a tuple struct over `String` with the usual
/// `as_str`/`Display`/`AsRef<str>`/`String` conversions and a `FromStr` that
/// runs `$validate` (an `fn(&str) -> Result<String, String>`, returning the
/// string to store — letting it normalize, not just check) on construction.
/// Serde round-trips through `String`, so validation runs on deserialization
/// too. Type-specific extras (`Deref`, `PartialEq<str>`, ...) are added
/// separately alongside each invocation.
macro_rules! validated_string {
    ($name:ident, $validate:path) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                value.parse()
            }
        }
        impl FromStr for $name {
            type Err = String;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self($validate(value)?))
            }
        }
    };
}

validated_string!(NodeId, validate_node_id);

fn validate_node_id(value: &str) -> Result<String, String> {
    if value.is_empty() || value == "." || value == ".." {
        return Err("node id must be a non-empty name".into());
    }
    if value.contains(['/', '\\']) || value.chars().any(char::is_control) {
        return Err("node id must not contain separators or control characters".into());
    }
    if value.eq_ignore_ascii_case(".git") || (value.len() >= 2 && value.as_bytes()[1] == b':') {
        return Err("node id uses a forbidden platform name or prefix".into());
    }
    Ok(value.into())
}

impl std::ops::Deref for NodeId {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

validated_string!(CandidateId, validate_candidate_id);

fn validate_candidate_id(value: &str) -> Result<String, String> {
    if !value.starts_with("candidate-")
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(format!("invalid candidate id `{value}`"));
    }
    Ok(value.into())
}

impl CandidateId {
    pub fn new() -> Self {
        Self(format!("candidate-{}", ulid::Ulid::new()))
    }
}

impl Default for CandidateId {
    fn default() -> Self {
        Self::new()
    }
}

validated_string!(ProjectPath, validate_project_path);

fn validate_project_path(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || (normalized.len() >= 2 && normalized.as_bytes()[1] == b':')
        || normalized.chars().any(char::is_control)
    {
        return Err("project path must be a non-empty relative path".into());
    }
    for component in normalized.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.eq_ignore_ascii_case(".git")
        {
            return Err("project path contains a forbidden component".into());
        }
    }
    Ok(normalized)
}

impl PartialEq<str> for ProjectPath {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for ProjectPath {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl AsRef<std::path::Path> for ProjectPath {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_identifiers_and_project_paths_are_validated_and_normalized() {
        for invalid in [
            "",
            ".",
            "..",
            "../secret",
            "/absolute",
            r"..\secret",
            ".git",
            "C:node",
            "bad\nnode",
        ] {
            assert!(
                invalid.parse::<NodeId>().is_err(),
                "accepted node id {invalid:?}"
            );
        }
        assert_eq!("node-good".parse::<NodeId>().unwrap().as_str(), "node-good");

        for invalid in [
            "",
            "..",
            "../secret",
            "/absolute",
            r"..\secret",
            ".git/config",
            "src/.git/config",
            "C:/windows",
            "bad\npath",
        ] {
            assert!(
                invalid.parse::<ProjectPath>().is_err(),
                "accepted project path {invalid:?}"
            );
        }
        assert_eq!(
            r"src\nested\file.rs"
                .parse::<ProjectPath>()
                .unwrap()
                .as_str(),
            "src/nested/file.rs"
        );
    }
}
