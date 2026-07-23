//! Agent profiles: the only agent-specific knowledge in Styra.
//!
//! A profile names the isolated command, the wire protocol it speaks, the Driva
//! policy it needs, and how an operator message is encoded as one protocol
//! input line. Driva remains the isolation executor; interpretation of the
//! streams belongs here and in [`crate::event`], exactly as Orka keeps provider
//! knowledge out of Driva.

use crate::event::Protocol;
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A host path exposed at an isolated destination. Mirrors Orka's mount spec so
/// the Driva translation in [`crate::session`] is a direct mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub writable: bool,
}

/// Stable paths inside one isolated Styra session. The workspace is where the
/// operator's project (or a throwaway worktree) is mounted writable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLayout {
    pub workspace: PathBuf,
}

impl Default for SandboxLayout {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("/tmp/styra/workspace"),
        }
    }
}

/// How an operator message becomes one line written to the agent's stdin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageFormat {
    /// A codex protocol submission envelope carrying the text as a user turn.
    CodexSubmission,
    /// The bare message text as a single line, for agents that read plain
    /// stdin turns.
    PlainLine,
}

/// Everything Styra needs to launch and drive one agent. The workspace bind
/// mount is added by the session from the operator's `--workspace`; the profile
/// contributes only its own agent-specific mounts (credentials, state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub command: Vec<String>,
    pub protocol: Protocol,
    pub mounts: Vec<MountSpec>,
    pub environment: BTreeMap<String, String>,
    pub network: bool,
    pub message_format: MessageFormat,
    /// The agent reads one prompt to end-of-input, then runs to completion (a
    /// one-shot `exec` agent). The session closes stdin after the operator's
    /// message so the turn can start; a further message is not possible.
    pub single_turn: bool,
}

impl Profile {
    /// Resolve a built-in profile by name.
    pub fn builtin(name: &str, layout: &SandboxLayout) -> Result<Profile> {
        match name {
            "codex" => Ok(codex(layout)),
            other => bail!("unknown agent profile {other:?}; known profiles: codex"),
        }
    }

    /// Encode an operator message as one newline-terminated protocol input line.
    pub fn encode_message(&self, text: &str) -> Vec<u8> {
        let mut line = match self.message_format {
            MessageFormat::CodexSubmission => codex_submission(text),
            MessageFormat::PlainLine => text.replace('\n', " "),
        };
        line.push('\n');
        line.into_bytes()
    }
}

/// The built-in codex profile.
///
/// Isolation follows Orka's proven codex shape: the workspace is trusted so
/// codex does not prompt, its inner sandbox is disabled in favour of Driva's
/// outer Bubblewrap isolation, `~/.codex/auth.json` is mounted writable so
/// credential refreshes persist, and stable `HOME`/`TERM` are set because
/// Bubblewrap clears the environment.
///
/// The command is `codex exec --json -`: a single-turn run that reads the
/// prompt from stdin and streams `thread`/`turn`/`item` events, verified
/// against codex-cli 0.145. codex has no bidirectional protocol subcommand in
/// this line — true multi-turn interaction needs the experimental `app-server`
/// JSON-RPC protocol, which would be a new [`Protocol`] variant and decoder and
/// a non-`single_turn` profile. Until then, one session is one turn.
pub fn codex(layout: &SandboxLayout) -> Profile {
    let workspace = layout.workspace.to_string_lossy();
    let trust = format!("projects.{workspace:?}.trust_level=\"trusted\"");
    Profile {
        name: "codex".into(),
        command: vec![
            "codex".into(),
            "-c".into(),
            trust,
            "--sandbox".into(),
            "danger-full-access".into(),
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--json".into(),
            "-".into(),
        ],
        protocol: Protocol::CodexJsonl,
        // HOME lives under /tmp, the writable tmpfs Driva always provides, so
        // codex has a disposable, always-present home without depending on
        // /root existing in the host rootfs. The auth file is bound in below it.
        mounts: vec![MountSpec {
            source: "~/.codex/auth.json".into(),
            destination: "/tmp/agent-home/.codex/auth.json".into(),
            writable: true,
        }],
        environment: BTreeMap::from([
            ("HOME".into(), "/tmp/agent-home".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]),
        network: true,
        message_format: MessageFormat::PlainLine,
        single_turn: true,
    }
}

/// Build a codex protocol submission line carrying the operator's text.
///
/// The envelope shape may need to track the installed codex; it is kept in one
/// place for that reason. The submission id is unique per process.
fn codex_submission(text: &str) -> String {
    let submission = serde_json::json!({
        "id": submission_id(),
        "op": {
            "type": "user_input",
            "items": [{ "type": "text", "text": text }],
        }
    });
    submission.to_string()
}

fn submission_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("styra-{now}-{seq}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn codex_profile_isolates_the_workspace_and_speaks_the_decoded_protocol() {
        let layout = SandboxLayout::default();
        let profile = Profile::builtin("codex", &layout).unwrap();

        assert_eq!(profile.protocol, Protocol::CodexJsonl);
        assert!(profile.network);
        assert!(profile.single_turn);
        assert_eq!(profile.command[0], "codex");
        assert!(profile.command.iter().any(|arg| arg == "danger-full-access"));
        assert!(profile.command.iter().any(|arg| arg == "exec"));
        assert!(profile.command.iter().any(|arg| arg == "--json"));
        assert_eq!(profile.command.last().unwrap(), "-", "prompt is read from stdin");
        assert!(profile
            .command
            .iter()
            .any(|arg| arg.contains("/tmp/styra/workspace") && arg.contains("trusted")));
        assert!(profile.mounts.iter().any(|mount| {
            mount.destination == std::path::Path::new("/tmp/agent-home/.codex/auth.json")
                && mount.writable
        }));
        assert_eq!(profile.environment.get("HOME"), Some(&"/tmp/agent-home".to_string()));
    }

    #[test]
    fn unknown_profile_is_rejected() {
        assert!(Profile::builtin("gpt5", &SandboxLayout::default()).is_err());
    }

    fn codex_submission_profile() -> Profile {
        Profile {
            message_format: MessageFormat::CodexSubmission,
            ..codex(&SandboxLayout::default())
        }
    }

    #[test]
    fn codex_submission_is_valid_json_carrying_the_text_and_one_line() {
        let profile = codex_submission_profile();
        let encoded = profile.encode_message("fix the bug\nand test it");
        assert_eq!(*encoded.last().unwrap(), b'\n');
        assert_eq!(
            encoded.iter().filter(|&&b| b == b'\n').count(),
            1,
            "a submission must be exactly one input line"
        );
        let line = std::str::from_utf8(&encoded).unwrap().trim_end();
        let value: Value = serde_json::from_str(line).expect("submission is valid JSON");
        assert_eq!(value["op"]["items"][0]["text"], "fix the bug\nand test it");
        assert!(value["id"].is_string());
    }

    #[test]
    fn distinct_submissions_get_distinct_ids() {
        let profile = codex_submission_profile();
        let a = String::from_utf8(profile.encode_message("a")).unwrap();
        let b = String::from_utf8(profile.encode_message("b")).unwrap();
        let id = |s: &str| {
            serde_json::from_str::<Value>(s.trim_end()).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_ne!(id(&a), id(&b));
    }

    #[test]
    fn plain_line_format_flattens_to_a_single_line() {
        let profile = Profile {
            message_format: MessageFormat::PlainLine,
            ..codex(&SandboxLayout::default())
        };
        let encoded = profile.encode_message("one\ntwo");
        assert_eq!(encoded, b"one two\n");
    }
}
