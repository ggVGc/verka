//! File tree and prepared-content presentation.

/// Display-ready path structure. Display strings are owned because converting
/// platform paths may be lossy; file contents remain separate and borrowed.
pub struct FileView {
    pub reported: String,
    pub root: String,
    pub relative: String,
    pub components: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileContent<'a> {
    None,
    Ready(&'a str),
    Empty,
    Failed(&'a str),
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/files.rs"]
mod rendering;
