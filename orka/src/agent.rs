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
mod tests;
