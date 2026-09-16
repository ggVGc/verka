//! Hidden entry point that owns the auxiliary shell inside one sandbox.
//!
//! A Styra interaction still speaks to its agent over the agent's inherited standard
//! streams. The broker only wraps that launch: it starts a detached tmux server
//! in the same Bubblewrap sandbox, launches the agent with unchanged stdio, and
//! tears tmux down when the agent exits.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

pub(crate) const BROKER_ENV: &str = "STYRA_SANDBOX_BROKER";
pub(crate) const AGENT_COMMAND_ENV: &str = "STYRA_SANDBOX_AGENT_COMMAND";
pub(crate) const TMUX_ENV: &str = "STYRA_SANDBOX_TMUX";
pub(crate) const TMUX_SOCKET_ENV: &str = "STYRA_SANDBOX_TMUX_SOCKET";
pub(crate) const WORKDIR_ENV: &str = "STYRA_SANDBOX_WORKDIR";

/// Run the hidden broker when the launch sentinel is present.
///
/// Both Styra binaries call this before parsing their public CLI. On success it
/// exits with the wrapped agent's status so Driva observes exactly the outcome
/// it would have observed from launching the agent directly.
pub fn exit_if_requested() -> Option<Result<()>> {
    std::env::var_os(BROKER_ENV)?;
    Some(match BrokerConfig::from_env().and_then(run) {
        Ok(status) => {
            std::process::exit(status.code().unwrap_or(128));
        }
        Err(error) => Err(error),
    })
}

#[derive(Debug)]
struct BrokerConfig {
    agent_command: Vec<String>,
    tmux: PathBuf,
    tmux_socket: PathBuf,
    workdir: PathBuf,
}

impl BrokerConfig {
    fn from_env() -> Result<Self> {
        let command = required_env(AGENT_COMMAND_ENV)?;
        let agent_command: Vec<String> =
            serde_json::from_str(&command).context("decoding the sandbox agent command")?;
        if agent_command.first().is_none_or(String::is_empty) {
            bail!("the sandbox agent command is empty");
        }
        Ok(Self {
            agent_command,
            tmux: required_env(TMUX_ENV)?.into(),
            tmux_socket: required_env(TMUX_SOCKET_ENV)?.into(),
            workdir: required_env(WORKDIR_ENV)?.into(),
        })
    }
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set for the sandbox broker"))
}

fn run(config: BrokerConfig) -> Result<ExitStatus> {
    start_tmux(&config)?;

    let mut agent = match Command::new(&config.agent_command[0])
        .args(&config.agent_command[1..])
        // Broker configuration is an implementation detail, not agent input.
        .env_remove(BROKER_ENV)
        .env_remove(AGENT_COMMAND_ENV)
        .env_remove(TMUX_ENV)
        .env_remove(TMUX_SOCKET_ENV)
        .env_remove(WORKDIR_ENV)
        .spawn()
        .with_context(|| format!("starting agent {}", config.agent_command[0]))
    {
        Ok(agent) => agent,
        Err(error) => {
            stop_tmux(&config);
            return Err(error);
        }
    };
    let status = agent.wait().context("waiting for the sandbox agent")?;
    stop_tmux(&config);
    Ok(status)
}

fn start_tmux(config: &BrokerConfig) -> Result<()> {
    let status = tmux_command(config)
        .args([
            "new-session",
            "-d",
            "-s",
            "shell",
            "-c",
            &config.workdir.to_string_lossy(),
            "/bin/sh",
        ])
        // The detached server must not retain the agent protocol pipes.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("starting tmux {}", config.tmux.display()))?;
    if !status.status.success() {
        bail!(
            "could not start sandbox tmux: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    if !config.tmux_socket.exists() {
        bail!(
            "sandbox tmux started without creating {}",
            config.tmux_socket.display()
        );
    }
    Ok(())
}

fn stop_tmux(config: &BrokerConfig) {
    let _ = tmux_command(config)
        .args(["kill-server"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn tmux_command(config: &BrokerConfig) -> Command {
    let mut command = Command::new(&config.tmux);
    command.arg("-S").arg(&config.tmux_socket);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn broker_preserves_agent_stdio_and_cleans_up_tmux() {
        let root = std::env::temp_dir().join(format!("styra-broker-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let fake_tmux = root.join("tmux");
        std::fs::write(
            &fake_tmux,
            "#!/bin/sh\nsocket=\"$2\"\ncase \"$3\" in\nnew-session) : > \"$socket\" ;;\nkill-server) rm -f \"$socket\" ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket = root.join("tmux.sock");
        let config = BrokerConfig {
            agent_command: vec!["/bin/sh".into(), "-c".into(), "exit 7".into()],
            tmux: fake_tmux,
            tmux_socket: socket.clone(),
            workdir: root.clone(),
        };

        let status = run(config).unwrap();
        assert_eq!(status.code(), Some(7));
        assert!(!socket.exists(), "the broker should stop its tmux server");
        std::fs::remove_dir_all(root).ok();
    }
}
