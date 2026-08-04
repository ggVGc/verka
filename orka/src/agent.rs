//! Orka-owned coding-agent profiles.
//!
//! Profiles describe how a coding agent consumes Orka's prompt and which
//! narrowly scoped capabilities it needs. Driva remains only the isolation
//! executor; its user-facing template registry is deliberately not involved.

use anyhow::Result;
use genta::agent::Effort;
use genta::event::Protocol;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

pub const AGENT_PROMPT: &str =
    "Read and follow the instructions in the file named by the ORKA_PROMPT environment variable.";

/// The shape of output retained for an agent command.
///
/// Plain commands produce a transcript. Structured coding agents name the
/// Genta-owned wire protocol that decodes their verbatim event stream.
///
/// The protocol is also the identity of the decoder that reads an attempt's raw
/// agent-output fact back into a work log. Genta owns the versioned structured
/// protocol registry; Orka adds only its plain-output case and durable media
/// types. Each attempt therefore decodes through its recorded protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Plain,
    Agent(Protocol),
}

/// Media type stamped on the durable agent-output fact for [`OutputFormat::Plain`].
pub const PLAIN_OUTPUT_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// Media type stamped on the durable agent-output fact for
/// [`OutputFormat::Agent`] with [`Protocol::CodexJsonl`]. Vendor-specific and version-bearing so the
/// stored fact names exactly the decoder that reads it; a future revision of the
/// Codex stream gets a new type (e.g. `…codex.v2+ndjson`) and a new variant.
pub const CODEX_JSONL_OUTPUT_MEDIA_TYPE: &str = "application/vnd.orka.codex.v1+ndjson";

impl OutputFormat {
    pub fn is_agent(self) -> bool {
        matches!(self, OutputFormat::Agent(_))
    }

    pub fn records_file_changes(self) -> bool {
        matches!(self, OutputFormat::Agent(Protocol::CodexJsonl))
    }

    /// The media type stamped on this protocol's durable agent-output fact, so
    /// the stored blob is self-describing: a reader selects the decoder from
    /// this alone, without the attempt's request record.
    pub fn output_media_type(self) -> &'static str {
        match self {
            OutputFormat::Plain => PLAIN_OUTPUT_MEDIA_TYPE,
            OutputFormat::Agent(Protocol::CodexJsonl) => CODEX_JSONL_OUTPUT_MEDIA_TYPE,
            OutputFormat::Agent(Protocol::CodexAppServer) => {
                "application/vnd.orka.codex-app-server.v1+ndjson"
            }
            OutputFormat::Agent(Protocol::ClaudeJsonl) => "application/vnd.orka.claude.v1+ndjson",
        }
    }

    /// The decoder named by a stored agent-output media type, when it is one
    /// this build understands. `None` means the fact was written by a newer or
    /// unknown decoder — surfaced as an error rather than mis-decoded.
    pub fn from_output_media_type(media_type: &str) -> Option<Self> {
        match media_type {
            PLAIN_OUTPUT_MEDIA_TYPE => Some(OutputFormat::Plain),
            CODEX_JSONL_OUTPUT_MEDIA_TYPE => Some(OutputFormat::Agent(Protocol::CodexJsonl)),
            "application/vnd.orka.codex-app-server.v1+ndjson" => {
                Some(OutputFormat::Agent(Protocol::CodexAppServer))
            }
            "application/vnd.orka.claude.v1+ndjson" => {
                Some(OutputFormat::Agent(Protocol::ClaudeJsonl))
            }
            _ => None,
        }
    }
}

// Preserve the original string representation in durable attempt records:
// "plain" and "codex-jsonl". Genta's protocol names extend that representation
// directly instead of introducing a nested Orka-specific enum shape.
impl Serialize for OutputFormat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OutputFormat::Plain => serializer.serialize_str("plain"),
            OutputFormat::Agent(protocol) => protocol.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for OutputFormat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "plain" {
            return Ok(OutputFormat::Plain);
        }
        let protocol =
            serde_json::from_value(serde_json::Value::String(value)).map_err(D::Error::custom)?;
        Ok(OutputFormat::Agent(protocol))
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

/// Resolve Orka's batch Codex session through Genta's complete agent profile.
/// Orka contributes its workspace, staged-prompt instruction, and the model and
/// reasoning effort the attempt runs on; Genta owns all Codex-specific command,
/// protocol, credential, environment, and network data.
///
/// `model` and `effort` are pinned rather than left to codex's own
/// configuration. An attempt is durable evidence: the argv it records has to say
/// which model produced the work, or a later reader cannot tell whether two
/// attempts are comparable, and re-running one cannot reproduce it. There is no
/// operator present to pick per run, so the configuration decides — see
/// `config::AgentConfig`.
///
/// A bare `codex` (the configuration default) is located on Orka's own `PATH`
/// rather than left for the sandbox to resolve: the isolation backend supplies a
/// fixed system `PATH`, so an agent installed under the operator's home would
/// otherwise fail inside the sandbox with an opaque `execvp` error.
pub fn codex(
    executable: &Path,
    layout: &SandboxLayout,
    model: &str,
    effort: Effort,
) -> Result<genta::agent::Profile> {
    let agent_layout = genta::agent::SandboxLayout {
        workspace: layout.workspace.clone(),
    };
    let executable = genta::agent::resolve_executable(executable)?;
    Ok(genta::agent::codex_exec(
        &agent_layout,
        &executable,
        AGENT_PROMPT,
        model,
        effort,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use driva::{BwrapIsolation, ExecutionRequest, Mount, MountAccess};
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    /// A stored agent-output fact is self-describing: each protocol stamps a
    /// distinct media type, and that media type maps back to exactly the decoder
    /// that produced it. A new decoder adds a variant and a distinct type; this
    /// guards the round-trip the durable render path depends on.
    #[test]
    fn output_media_types_round_trip_to_their_decoder() {
        for protocol in [
            OutputFormat::Plain,
            OutputFormat::Agent(Protocol::CodexJsonl),
            OutputFormat::Agent(Protocol::CodexAppServer),
            OutputFormat::Agent(Protocol::ClaudeJsonl),
        ] {
            assert_eq!(
                OutputFormat::from_output_media_type(protocol.output_media_type()),
                Some(protocol),
                "{protocol:?} media type must select its own decoder"
            );
        }
        assert_ne!(
            OutputFormat::Plain.output_media_type(),
            OutputFormat::Agent(Protocol::CodexJsonl).output_media_type(),
            "each decoder must be distinguishable by its stored media type"
        );
        // An output written by a newer or unknown decoder is refused, never
        // mis-decoded as a format this build happens to recognise.
        assert_eq!(
            OutputFormat::from_output_media_type("application/vnd.orka.future.v9+ndjson"),
            None
        );
    }

    #[test]
    fn output_formats_keep_their_flat_durable_names() {
        let codex = OutputFormat::Agent(Protocol::CodexJsonl);
        assert_eq!(serde_json::to_string(&codex).unwrap(), r#""codex-jsonl""#);
        assert_eq!(
            serde_json::from_str::<OutputFormat>(r#""codex-jsonl""#).unwrap(),
            codex
        );
        assert_eq!(
            serde_json::from_str::<OutputFormat>(r#""plain""#).unwrap(),
            OutputFormat::Plain
        );
    }

    /// A stub codex install, so profile construction resolves against a known
    /// path instead of whatever the machine running the tests has on its `PATH`.
    fn stub_codex() -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("orka-agent-bin-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("codex");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn codex_profile_uses_the_orka_layout_and_trusts_only_its_workspace() {
        let layout = SandboxLayout::default();
        let executable = stub_codex();
        let invocation = codex(&executable, &layout, "gpt-5.6-sol", Effort::High).unwrap();

        assert_eq!(layout.workspace, Path::new("/tmp/orka/workspace"));
        assert_eq!(layout.exchange, Path::new("/tmp/orka/exchange"));
        assert_eq!(Path::new(&invocation.command[0]), executable);
        assert_eq!(invocation.protocol, Protocol::CodexJsonl);
        assert!(invocation
            .command
            .iter()
            .any(|argument| argument == "--json"));
        assert!(invocation.command.iter().any(|argument| {
            argument == "projects.\"/tmp/orka/workspace\".trust_level=\"trusted\""
        }));
        assert_eq!(invocation.command.last().unwrap(), AGENT_PROMPT);
        // The attempt's own argv states the model and effort it ran, so the
        // recorded evidence says what produced the work.
        assert!(invocation
            .command
            .iter()
            .any(|argument| argument == r#"model="gpt-5.6-sol""#));
        assert!(invocation
            .command
            .iter()
            .any(|argument| argument == r#"model_reasoning_effort="high""#));
        assert!(invocation.network);
        assert!(invocation.mounts.iter().any(|mount| mount.destination
            == Path::new("/tmp/agent-home/.codex")
            && mount.writable));
        assert_eq!(invocation.environment["HOME"], "/tmp/agent-home");
    }

    #[test]
    fn codex_layout_needs_no_workspace_directory_in_a_bubblewrap_rootfs() {
        let rootfs = std::env::temp_dir().join(format!("orka-agent-rootfs-{}", ulid::Ulid::new()));
        for directory in ["proc", "dev", "tmp"] {
            std::fs::create_dir_all(rootfs.join(directory)).unwrap();
        }

        let layout = SandboxLayout::default();
        let executable = stub_codex();
        let invocation = codex(&executable, &layout, "gpt-5.6-sol", Effort::High).unwrap();
        let mut mounts = vec![
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
            writable_mounts: driva::WritableMountMode::Direct,
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
