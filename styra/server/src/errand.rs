//! An errand: one cheap, sandboxed question Styra asks an agent for itself.
//!
//! Styra's own work sometimes needs a sentence of natural language turned into
//! a few words — naming a branch after the prompt that started it ([`crate::naming`])
//! is the first such case. That is a different kind of run from an
//! interaction, and the difference is worth a type rather than an ad-hoc
//! subprocess at each call site:
//!
//! - **It is the operator's agent, on the cheap tier.** An errand goes to the
//!   provider the operator already selected — their credentials, their
//!   installed agent — but on that provider's cheapest model and lowest effort
//!   ([`Provider::cheapest_model`]). A phrase in and three words out is not
//!   work for the model their session runs on.
//! - **It is one-shot.** The prompt is written to stdin, stdin is closed, and
//!   the agent's final message is the answer. There is no journal, no Session,
//!   no resume, and nothing for a client to attach to; an errand that fails is
//!   an `Err` to whoever asked for it, never an interaction the operator has
//!   to see.
//! - **It is bounded.** Styra runs errands while an operator waits on
//!   something else, so an agent that is slow, wedged, or waiting on
//!   authentication is killed at [`Errand::DEFAULT_TIMEOUT`] and the caller
//!   decides what to do without it.
//! - **It is the narrowest sandbox Styra builds.** The errand holds nothing of
//!   the operator's: no workspace, no checkout, no Git metadata, no template,
//!   no operator mounts, no broker. It gets an empty tmpfs to stand in, the
//!   agent executable it has to run, and the provider profile's own mounts —
//!   the credential and state directory without which the agent cannot
//!   authenticate at all. Every launch flag the agent has for trusting its
//!   surroundings is then safe to keep, because those surroundings are empty:
//!   the errand reads a prompt that may be anything the operator pasted, and
//!   there is nothing in reach for it to act on.

use crate::agent::{Profile, Provider, SandboxLayout, Selection};
use crate::event::{decode_line, AgentEvent, Protocol};
use anyhow::{bail, Context, Result};
use driva::{
    ExecutionControl, ExecutionIo, ExecutionRequest, Mount, MountAccess, WritableMountMode,
};
use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where an errand stands inside its sandbox: an empty, discarded tmpfs.
///
/// An agent still needs a working directory, and codex additionally wants to
/// know whether it trusts the one it is in — so there is one, it is the
/// directory the profile is built around, and it contains nothing.
const SCRATCH: &str = "/tmp/styra/errand";

/// One question, and the agent that will answer it.
pub struct Errand {
    provider: Provider,
    prompt: String,
    timeout: Duration,
}

impl Errand {
    /// How long an errand may take before it is killed. Styra runs errands in
    /// front of something an operator is waiting for, so the bound is short
    /// enough to be worth waiting through and long enough for a small model's
    /// cold start.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

    /// Ask `provider` the question in `prompt`.
    pub fn new(provider: Provider, prompt: impl Into<String>) -> Self {
        Self {
            provider,
            prompt: prompt.into(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Bound this errand differently from [`Errand::DEFAULT_TIMEOUT`].
    pub fn within(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// What this errand runs on: the one-shot form of the operator's provider,
    /// on its cheapest model and lowest effort.
    ///
    /// Interactive codex speaks a multi-turn app-server protocol that an
    /// errand has no use for, so an errand for `codex` is a `codex-exec` run.
    /// Claude Code's own `--print` mode is already one-shot.
    pub fn selection(&self) -> Selection {
        let provider = match self.provider {
            Provider::Codex | Provider::CodexExec => Provider::CodexExec,
            Provider::Claude => Provider::Claude,
        };
        let model = crate::agent::cheapest_model_for(self.provider).to_owned();
        let effort = crate::agent::cheapest_effort_for(provider, &model);
        Selection {
            provider,
            model,
            effort,
        }
    }

    /// Run the errand and return the agent's final message, trimmed.
    ///
    /// Fails when the agent cannot be located or launched, when it is killed
    /// at the timeout, or when it ends its turn without saying anything. Every
    /// one of those is ordinary — an agent may be uninstalled, logged out, or
    /// out of quota — so callers are expected to have an answer of their own
    /// for the failure rather than to propagate it.
    pub fn answer(&self) -> Result<String> {
        let base = driva::BaseConfig::default();
        let profile = self
            .selection()
            .resolve(&SandboxLayout::same_path(SCRATCH))
            .with_context(|| format!("resolving the {} errand profile", self.provider.as_str()))?;
        let request = request(&profile, &base)?;
        let backend = driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: None,
            base,
        };
        let io = Streams::open(&profile.encode_message(&self.prompt))?;
        let output = io.output.clone();
        self.execute(&backend, &request, io)?;
        let text = std::fs::read_to_string(&output)
            .with_context(|| format!("reading the errand's output at {}", output.display()))?;
        std::fs::remove_file(&output).ok();
        answer_from(profile.protocol, &text)
            .with_context(|| format!("the {} errand ended without an answer", profile.name))
    }

    /// Run `request` to completion, or kill it at the timeout.
    ///
    /// Driva blocks for the run, so the wait happens on this thread and the
    /// sandbox on another: the errand is told to terminate through the same
    /// cooperative control an interaction uses when an operator stops it.
    fn execute(
        &self,
        backend: &driva::BwrapIsolation,
        request: &ExecutionRequest,
        io: Streams,
    ) -> Result<()> {
        let control = ExecutionControl::default();
        let deadline = Instant::now() + self.timeout;
        std::thread::scope(|scope| {
            let running =
                scope.spawn(|| driva::execute_controlled(backend, request, io.io, &control));
            while !running.is_finished() {
                if Instant::now() >= deadline {
                    control.terminate();
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let outcome = running
                .join()
                .map_err(|_| anyhow::anyhow!("the errand's sandbox thread panicked"))?;
            if control.termination_requested() {
                bail!("the errand did not answer within {:?}", self.timeout);
            }
            outcome.map(|_| ()).context("running the errand's sandbox")
        })
    }
}

/// The policy one errand runs under.
///
/// Deliberately assembled here and not from an [`crate::interaction::InteractionSpec`]:
/// that type's fields are the layers an interaction has (workspace, repository,
/// operator, template, broker), and an errand's whole point is that it has
/// none of them. Listing the three it does have — scratch, the agent
/// executable, the provider's own credential mounts — is what makes that
/// visible, and reviewable, in one place.
fn request(profile: &Profile, base: &driva::BaseConfig) -> Result<ExecutionRequest> {
    let executable = profile
        .command
        .first()
        .context("the errand profile names no executable")?;
    let mut mounts = vec![Mount::Temporary {
        destination: PathBuf::from(SCRATCH),
    }];
    mounts.extend(
        crate::tooling::executable_mounts(&[PathBuf::from(executable)], base)?
            .into_iter()
            .map(|mount| bind(&mount, MountAccess::ReadOnly)),
    );
    mounts.extend(profile.mounts.iter().map(|mount| {
        bind(
            mount,
            if mount.writable {
                MountAccess::ReadWrite
            } else {
                MountAccess::ReadOnly
            },
        )
    }));
    Ok(ExecutionRequest {
        command: profile.command.iter().map(OsString::from).collect(),
        working_directory: PathBuf::from(SCRATCH),
        mounts,
        writable_mounts: WritableMountMode::Direct,
        environment: profile
            .environment
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect(),
        // The agent has to reach its provider; that is the entire errand.
        network: profile.network,
        interactive: false,
        new_session: true,
    })
}

fn bind(mount: &crate::agent::MountSpec, access: MountAccess) -> Mount {
    Mount::Bind {
        source: mount.source.clone(),
        destination: mount.destination.clone(),
        access,
    }
}

/// The agent's final message, decoded from its own wire protocol.
///
/// Reasoning, tool traffic and turn bookkeeping share the stream with it, and
/// an agent may say several things before it stops; the answer is the last
/// thing it actually said.
fn answer_from(protocol: Protocol, output: &str) -> Option<String> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match decode_line(protocol, line) {
            AgentEvent::AgentMessage { text, .. } => Some(text.trim().to_owned()),
            _ => None,
        })
        .rfind(|text| !text.is_empty())
}

/// The three files an errand's sandbox is wired to.
///
/// Files rather than pipes: an errand writes one prompt and reads one answer,
/// with no interleaving to manage, so staging both on disk keeps the run to a
/// single blocking call instead of a reader thread per stream. Its stderr is
/// the agent's own diagnostics, which nothing here can show anyone; it is
/// discarded rather than mixed into the answer.
struct Streams {
    io: ExecutionIo,
    output: PathBuf,
}

impl Streams {
    fn open(prompt: &[u8]) -> Result<Self> {
        let tag = format!(
            "styra-errand-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        );
        let input = std::env::temp_dir().join(format!("{tag}.in"));
        let output = std::env::temp_dir().join(format!("{tag}.out"));
        write_prompt(&input, prompt)?;
        let io = ExecutionIo {
            stdin: File::open(&input)
                .with_context(|| format!("reopening {} for the errand", input.display()))?,
            stdout: File::create(&output)
                .with_context(|| format!("creating {}", output.display()))?,
            stderr: File::create("/dev/null").context("opening /dev/null for the errand")?,
        };
        // The prompt file is only a way to hand the agent a read-only stdin;
        // it is unlinked immediately, and the open handle outlives the name.
        std::fs::remove_file(&input).ok();
        Ok(Self { io, output })
    }
}

fn write_prompt(path: &Path, prompt: &[u8]) -> Result<()> {
    let mut file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    file.write_all(prompt)
        .with_context(|| format!("writing the errand prompt to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Effort;
    use std::os::unix::fs::PermissionsExt;

    /// An errand profile built against a stub agent, so the test states what
    /// the sandbox holds on any host — installed agent or not.
    fn stub_profile(provider: Provider) -> Profile {
        let directory = std::env::temp_dir().join(format!(
            "styra-errand-stub-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(provider.executable());
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        Errand::new(provider, "name this")
            .selection()
            .resolve_on_path(
                &SandboxLayout::same_path(SCRATCH),
                &OsString::from(&directory),
            )
            .unwrap()
    }

    /// An errand is the operator's own agent on its cheap tier, in the
    /// one-shot form — never the model their session runs on. Cheap, but still
    /// launchable: the rung is the lowest that model actually accepts, and the
    /// model is the cheapest one that takes a rung at all.
    #[test]
    fn an_errand_runs_on_the_providers_cheapest_launchable_model() {
        let codex = Errand::new(Provider::Codex, "name this").selection();
        assert_eq!(codex.provider, Provider::CodexExec);
        assert_eq!(codex.model, "gpt-5.6-luna");
        assert_eq!(codex.effort, Effort::Low);

        let claude = Errand::new(Provider::Claude, "name this").selection();
        assert_eq!(claude.provider, Provider::Claude);
        // Not Haiku 4.5, which is cheaper but takes no effort setting at all.
        assert_eq!(claude.model, "claude-sonnet-5");
        assert_eq!(claude.effort, Effort::Low);

        for provider in crate::agent::PROVIDERS {
            let errand = Errand::new(provider, "name this").selection();
            assert_ne!(errand.model, provider.default_model());
            crate::agent::validate_selection(&Selection {
                provider,
                ..errand.clone()
            })
            .unwrap();
        }
    }

    /// The sandbox holds the scratch directory, the agent binary, and the
    /// provider's own credentials — and nothing of the operator's. This is the
    /// property the whole type exists for, so it is asserted exhaustively:
    /// every mount is one of the three, not merely "none of the bad ones".
    #[test]
    fn the_sandbox_holds_nothing_but_scratch_the_agent_and_its_credentials() {
        let profile = stub_profile(Provider::Claude);
        let base = driva::BaseConfig::default();
        let request = request(&profile, &base).unwrap();

        assert_eq!(request.working_directory, PathBuf::from(SCRATCH));
        assert!(matches!(
            request.mounts[0],
            Mount::Temporary { ref destination } if destination == Path::new(SCRATCH)
        ));
        let profile_destinations: Vec<&Path> = profile
            .mounts
            .iter()
            .map(|mount| mount.destination.as_path())
            .collect();
        let executable = PathBuf::from(&profile.command[0]);
        for mount in &request.mounts[1..] {
            let destination = mount.destination();
            assert!(
                profile_destinations.contains(&destination)
                    || executable.starts_with(destination)
                    || destination.starts_with(&executable),
                "{destination:?} is neither the agent nor a profile mount"
            );
        }
        assert!(request.new_session);
        assert!(!request.interactive);
    }

    /// The prompt reaches the agent as a message in its own input format, not
    /// as a command-line argument — a first prompt can be anything at all.
    #[test]
    fn the_prompt_is_written_to_stdin_in_the_agents_own_format() {
        let profile = stub_profile(Provider::Codex);
        let request = request(&profile, &driva::BaseConfig::default()).unwrap();

        assert_eq!(request.command.last().unwrap(), "-");
        let encoded = String::from_utf8(profile.encode_message("Fix the flaky test")).unwrap();
        assert_eq!(encoded, "Fix the flaky test\n");
        assert!(!request
            .command
            .iter()
            .any(|argument| argument.to_string_lossy().contains("Fix the flaky test")));
    }

    /// The answer is what the agent said last, not its reasoning, its tool
    /// calls, or an earlier draft.
    #[test]
    fn the_answer_is_the_agents_last_message() {
        let output = [
            r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"first-guess"}}"#,
            r#"{"type":"item.completed","item":{"id":"m2","type":"agent_message","text":" fix-flaky-checkout-test\n"}}"#,
        ]
        .join("\n");

        assert_eq!(
            answer_from(Protocol::CodexJsonl, &output).as_deref(),
            Some("fix-flaky-checkout-test")
        );
        assert_eq!(answer_from(Protocol::CodexJsonl, "not json\n"), None);
        assert_eq!(answer_from(Protocol::CodexJsonl, ""), None);
    }

    /// The prompt is handed over as an unlinked file: the agent reads it, and
    /// nothing on the host is left holding what the operator typed.
    #[test]
    fn the_prompt_file_does_not_outlive_the_call_that_staged_it() {
        let streams = Streams::open(b"name this\n").unwrap();
        let staged: Vec<PathBuf> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "in")
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("styra-errand-"))
            })
            .collect();

        assert!(staged.is_empty(), "{staged:?} outlived the errand's setup");
        assert!(streams.output.exists());
        std::fs::remove_file(streams.output).ok();
    }
}
