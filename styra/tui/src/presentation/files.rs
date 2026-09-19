//! Preparation of file-tree records before presentation.

use crate::app::App;
use crate::files::{self, FileItem};
use std::path::PathBuf;

pub(crate) fn items(app: &App) -> Vec<FileItem> {
    let root = app
        .workspace
        .root_or_current_directory()
        .unwrap_or_else(|| PathBuf::from("."));
    files::items(&root, app.file_paths())
}
