//! Orka-owned coding-agent profiles.
//!
//! Profiles describe how a coding agent consumes Orka's prompt and which
//! narrowly scoped capabilities it needs. Driva remains only the isolation
//! executor; its user-facing template registry is deliberately not involved.

use crate::executor::MountSpec;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const AGENT_PROMPT: &str =
    "Read and follow the instructions in the file named by the ORKA_PROMPT environment variable.";

/// Machine-readable output protocol produced by an agent command.
///
/// This belongs to Orka because Driva transports process streams without
/// interpreting which program produced them.
///
/// The protocol is also the identity of the decoder that reads an attempt's raw
/// agent-output fact back into a work log. It is a versioned registry: a new
/// agent wire format (or a new version of an existing one) is added as a new
/// variant here, its media type below, and a match arm in
/// [`crate::events::work_log_from_raw`]. Because dispatch is an exhaustive
/// match, the compiler then requires every reader to handle it, so a decoder is
/// never silently missing — and old attempts keep decoding through their own
/// recorded variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentProtocol {
    #[default]
    Plain,
    CodexJsonl,
}

/// Media type stamped on the durable agent-output fact for [`AgentProtocol::Plain`].
pub const PLAIN_OUTPUT_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// Media type stamped on the durable agent-output fact for
/// [`AgentProtocol::CodexJsonl`]. Vendor-specific and version-bearing so the
/// stored fact names exactly the decoder that reads it; a future revision of the
/// Codex stream gets a new type (e.g. `…codex.v2+ndjson`) and a new variant.
pub const CODEX_JSONL_OUTPUT_MEDIA_TYPE: &str = "application/vnd.orka.codex.v1+ndjson";

impl AgentProtocol {
    /// The media type stamped on this protocol's durable agent-output fact, so
    /// the stored blob is self-describing: a reader selects the decoder from
    /// this alone, without the attempt's request record.
    pub fn output_media_type(self) -> &'static str {
        match self {
            AgentProtocol::Plain => PLAIN_OUTPUT_MEDIA_TYPE,
            AgentProtocol::CodexJsonl => CODEX_JSONL_OUTPUT_MEDIA_TYPE,
        }
    }

    /// The decoder named by a stored agent-output media type, when it is one
    /// this build understands. `None` means the fact was written by a newer or
    /// unknown decoder — surfaced as an error rather than mis-decoded.
    pub fn from_output_media_type(media_type: &str) -> Option<Self> {
        match media_type {
            PLAIN_OUTPUT_MEDIA_TYPE => Some(AgentProtocol::Plain),
            CODEX_JSONL_OUTPUT_MEDIA_TYPE => Some(AgentProtocol::CodexJsonl),
            _ => None,
        }
    }
}

/// Stable paths inside one isolated Orka execution.
///
/// Bubblewrap always provides a private writable `/tmp`; container backends
/// create bind destinations as needed. Keeping the whole protocol beneath one
/// Orka-owned root avoids assumptions about directories in an agent rootfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLayout {
    pub workspace: PathBuf,
    pub exchange: PathBuf,
}

impl Default for SandboxLayout {
    fn default() -> Self {
        let root = PathBuf::from("/tmp/orka");
        Self {
            workspace: root.join("workspace"),
            exchange: root.join("exchange"),
        }
    }
}

/// Agent-specific parts of an execution request. The engine adds the concrete
/// attempt worktree and prompt/outcome exchange mounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInvocation {
    pub command: Vec<String>,
    pub protocol: AgentProtocol,
    pub mounts: Vec<MountSpec>,
    pub environment: BTreeMap<String, String>,
    pub network: bool,
}

pub fn codex(executable: &Path, layout: &SandboxLayout) -> Result<AgentInvocation> {
    let workspace = layout
        .workspace
        .to_str()
        .context("Orka's isolated workspace path is not valid UTF-8")?;
    let trust = format!("projects.{workspace:?}.trust_level=\"trusted\"");

    Ok(AgentInvocation {
        command: vec![
            executable.to_string_lossy().into_owned(),
            "-c".into(),
            trust,
            "--sandbox".into(),
            "danger-full-access".into(),
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--json".into(),
            AGENT_PROMPT.into(),
        ],
        protocol: AgentProtocol::CodexJsonl,
        mounts: vec![MountSpec {
            source: "~/.codex/auth.json".into(),
            destination: "/root/.codex/auth.json".into(),
            writable: true,
        }],
        environment: BTreeMap::from([
            ("HOME".into(), "/root".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]),
        network: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use driva::{BwrapIsolation, ExecutionRequest, Mount, MountAccess};
    use std::ffi::OsString;

    /// A stored agent-output fact is self-describing: each protocol stamps a
    /// distinct media type, and that media type maps back to exactly the decoder
    /// that produced it. A new decoder adds a variant and a distinct type; this
    /// guards the round-trip the durable render path depends on.
    #[test]
    fn output_media_types_round_trip_to_their_decoder() {
        for protocol in [AgentProtocol::Plain, AgentProtocol::CodexJsonl] {
            assert_eq!(
                AgentProtocol::from_output_media_type(protocol.output_media_type()),
                Some(protocol),
                "{protocol:?} media type must select its own decoder"
            );
        }
        assert_ne!(
            AgentProtocol::Plain.output_media_type(),
            AgentProtocol::CodexJsonl.output_media_type(),
            "each decoder must be distinguishable by its stored media type"
        );
        // An output written by a newer or unknown decoder is refused, never
        // mis-decoded as a format this build happens to recognise.
        assert_eq!(
            AgentProtocol::from_output_media_type("application/vnd.orka.future.v9+ndjson"),
            None
        );
    }

    #[test]
    fn codex_profile_uses_the_orka_layout_and_trusts_only_its_workspace() {
        let layout = SandboxLayout::default();
        let invocation = codex(Path::new("codex"), &layout).unwrap();

        assert_eq!(layout.workspace, Path::new("/tmp/orka/workspace"));
        assert_eq!(layout.exchange, Path::new("/tmp/orka/exchange"));
        assert_eq!(invocation.command[0], "codex");
        assert_eq!(invocation.protocol, AgentProtocol::CodexJsonl);
        assert!(invocation
            .command
            .iter()
            .any(|argument| argument == "--json"));
        assert!(invocation.command.iter().any(|argument| {
            argument == "projects.\"/tmp/orka/workspace\".trust_level=\"trusted\""
        }));
        assert_eq!(invocation.command.last().unwrap(), AGENT_PROMPT);
        assert!(invocation.network);
        assert!(invocation
            .mounts
            .iter()
            .any(|mount| mount.destination == Path::new("/root/.codex/auth.json")));
    }

    #[test]
    fn codex_layout_needs_no_workspace_directory_in_a_bubblewrap_rootfs() {
        let rootfs = std::env::temp_dir().join(format!("orka-agent-rootfs-{}", ulid::Ulid::new()));
        for directory in ["proc", "dev", "tmp", "root"] {
            std::fs::create_dir_all(rootfs.join(directory)).unwrap();
        }

        let layout = SandboxLayout::default();
        let invocation = codex(Path::new("codex"), &layout).unwrap();
        let mut mounts = vec![
            Mount::Temporary {
                destination: "/root".into(),
            },
            Mount::Bind {
                source: "/host/attempt".into(),
                destination: layout.workspace.clone(),
                access: MountAccess::ReadWrite,
            },
            Mount::Bind {
                source: "/host/exchange".into(),
                destination: layout.exchange.clone(),
                access: MountAccess::ReadWrite,
            },
        ];
        mounts.extend(invocation.mounts.into_iter().map(|mount| Mount::Bind {
            source: mount.source,
            destination: mount.destination,
            access: if mount.writable {
                MountAccess::ReadWrite
            } else {
                MountAccess::ReadOnly
            },
        }));
        let request = ExecutionRequest {
            command: invocation.command.into_iter().map(OsString::from).collect(),
            working_directory: layout.workspace,
            mounts,
            environment: BTreeMap::new(),
            network: invocation.network,
            interactive: false,
            new_session: true,
        };
        let backend = BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: Some(rootfs.clone()),
        };

        backend.command(&request).unwrap();
        assert!(!rootfs.join("workspace").exists());
        std::fs::remove_dir_all(rootfs).unwrap();
    }
}
