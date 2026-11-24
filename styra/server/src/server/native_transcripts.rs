//! Storage for provider-native transcripts, shared by switching and branching.
use super::{find_native_session_file, native_session_destination};
use crate::agent::Provider;
use anyhow::{Context, Result};
use std::path::Path;

/// Native ids belong to a provider. The working directory is needed when
/// writing because Claude scopes its transcripts by project; it is not needed
/// when reading, since providers may move transcripts within their own trees.
pub(super) trait NativeTranscripts: Send + Sync {
    fn read(&self, provider: Provider, native_id: &str) -> Result<String>;
    fn write(&self, provider: Provider, native_id: &str, cwd: &Path, text: &str) -> Result<()>;
}

pub(super) struct FilesystemTranscripts;

impl NativeTranscripts for FilesystemTranscripts {
    fn read(&self, provider: Provider, native_id: &str) -> Result<String> {
        let path = find_native_session_file(provider, native_id)?;
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    fn write(&self, provider: Provider, native_id: &str, cwd: &Path, text: &str) -> Result<()> {
        let path = native_session_destination(provider, native_id, cwd)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}

/// Convert and store a branch of native history. Switching passes the entire
/// conversation; explicit branches may choose a cutoff or a single message.
/// Session/journal updates belong to the caller and happen after this succeeds.
pub(super) fn write_converted(
    transcripts: &dyn NativeTranscripts,
    source: &str,
    from: Provider,
    to: Provider,
    native_id: &str,
    cwd: &Path,
    point: super::BranchPoint<'_>,
) -> Result<()> {
    let converted = super::branch_native_session(
        source,
        super::native_session_format(from),
        super::native_session_format(to),
        native_id,
        cwd,
        point,
    )?;
    transcripts.write(to, native_id, cwd, &converted)
}

/// Configurable storage for workflow tests. It models provider/id lookup and
/// write failure without knowing which workflow or conversation is under test.
#[cfg(test)]
#[derive(Default)]
pub(super) struct MemoryTranscripts {
    entries: std::sync::Mutex<std::collections::BTreeMap<(String, String), String>>,
    fail_writes: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl MemoryTranscripts {
    pub(super) fn fail_writes(&self, fail: bool) {
        self.fail_writes
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    pub(super) fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

#[cfg(test)]
impl NativeTranscripts for MemoryTranscripts {
    fn read(&self, provider: Provider, native_id: &str) -> Result<String> {
        self.entries
            .lock()
            .unwrap()
            .get(&(provider.as_str().into(), native_id.into()))
            .cloned()
            .with_context(|| format!("{} session {native_id:?} does not exist", provider.as_str()))
    }

    fn write(&self, provider: Provider, native_id: &str, _cwd: &Path, text: &str) -> Result<()> {
        anyhow::ensure!(
            !self.fail_writes.load(std::sync::atomic::Ordering::Relaxed),
            "native storage unavailable"
        );
        self.entries
            .lock()
            .unwrap()
            .insert((provider.as_str().into(), native_id.into()), text.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
