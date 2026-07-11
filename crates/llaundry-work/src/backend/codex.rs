use super::{event_model, Backend, RunOutcome, Session};
use anyhow::{Context, Result};
use std::process::Command;

/// Backend that runs OpenAI Codex non-interactively in the execution worktree.
pub struct OpenAiCodex {
    binary: String,
    model: Option<String>,
    network: bool,
}

impl OpenAiCodex {
    pub fn new(binary: String, model: Option<String>, network: bool) -> Self {
        Self {
            binary,
            model,
            network,
        }
    }

    fn command(&self, session: &Session) -> Command {
        let mcp_command = toml::Value::String(session.mcp.command.clone()).to_string();
        let mcp_args = toml::Value::Array(
            session
                .mcp
                .args
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        )
        .to_string();

        let mut cmd = Command::new(&self.binary);
        cmd.arg("exec")
            .arg("--json")
            .arg("--color")
            .arg("never")
            .arg("--ephemeral")
            .arg("--ignore-user-config")
            .arg("--sandbox")
            .arg("workspace-write")
            .arg("--cd")
            .arg(&session.project_root)
            .arg("--config")
            .arg("approval_policy=\"never\"")
            .arg("--config")
            .arg(format!(
                "mcp_servers.{}.command={mcp_command}",
                session.mcp.name
            ))
            .arg("--config")
            .arg(format!("mcp_servers.{}.args={mcp_args}", session.mcp.name))
            .arg("--config")
            .arg(format!(
                "sandbox_workspace_write.network_access={}",
                self.network
            ));
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        cmd.arg(&session.prompt);
        cmd
    }
}

impl Backend for OpenAiCodex {
    fn name(&self) -> &str {
        "openai-codex"
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn run(&self, session: &Session, log: &mut dyn std::io::Write) -> Result<RunOutcome> {
        use std::io::{BufRead, BufReader};
        let mut child = self
            .command(session)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch `{}` — is OpenAI Codex installed and on PATH?",
                    self.binary
                )
            })?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let mut model = None;
        for line in BufReader::new(stdout).lines() {
            let line = line.context("reading backend output")?;
            println!("{line}");
            writeln!(log, "{line}").context("writing work log")?;
            log.flush().context("flushing work log")?;
            if model.is_none() {
                model = event_model(&line);
            }
        }
        Ok(RunOutcome {
            success: child.wait()?.success(),
            model,
        })
    }

    fn describe(&self, session: &Session) -> String {
        let cmd = self.command(session);
        let mut parts = vec![cmd.get_program().to_string_lossy().into_owned()];
        parts.extend(cmd.get_args().map(|a| shell_quote(&a.to_string_lossy())));
        parts.join(" ")
    }
}

fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::McpServer;

    fn session() -> Session {
        Session {
            node_id: "node-1".into(),
            prompt: "do the work".into(),
            project_root: "/wb/project".into(),
            mcp: McpServer {
                name: "llaundry".into(),
                command: "llaundry-mcp".into(),
                args: vec!["--store".into(), "/wb/.llaundry".into()],
            },
        }
    }

    fn args(network: bool, model: Option<&str>) -> Vec<String> {
        OpenAiCodex::new("codex".into(), model.map(str::to_string), network)
            .command(&session())
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn command_is_noninteractive_scoped_and_jsonl() {
        let cmd = OpenAiCodex::new("codex".into(), None, false).command(&session());
        assert_eq!(cmd.get_program(), "codex");
        let args = args(false, None);
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".into()));
        assert!(args.contains(&"--ephemeral".into()));
        assert!(args.contains(&"--ignore-user-config".into()));
        assert!(args
            .windows(2)
            .any(|a| a == ["--sandbox", "workspace-write"]));
        assert!(args.windows(2).any(|a| a == ["--cd", "/wb/project"]));
        assert!(args.iter().any(|a| a == "approval_policy=\"never\""));
        assert!(args
            .iter()
            .any(|a| a == "mcp_servers.llaundry.command=\"llaundry-mcp\""));
        assert!(args
            .iter()
            .any(|a| a.contains("mcp_servers.llaundry.args=") && a.contains("/wb/.llaundry")));
        assert!(args
            .iter()
            .any(|a| a == "sandbox_workspace_write.network_access=false"));
        assert_eq!(args.last().unwrap(), "do the work");
    }

    #[test]
    fn model_and_network_are_forwarded() {
        let args = args(true, Some("gpt-5-codex"));
        assert!(args.windows(2).any(|a| a == ["--model", "gpt-5-codex"]));
        assert!(args
            .iter()
            .any(|a| a == "sandbox_workspace_write.network_access=true"));
    }
}
