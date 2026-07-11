use super::backend::{Backend, Session};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::process::Command;

/// Backend that shells out to Claude Code (`claude -p`): the llaundry MCP for
/// graph operations, the built-in file tools for real work — no shell, no other
/// MCP servers, web tools only when `network` is set.
pub(crate) struct ClaudeCode {
    binary: String,
    model: Option<String>,
    network: bool,
}

impl ClaudeCode {
    pub(crate) fn new(binary: String, model: Option<String>, network: bool) -> Self {
        Self {
            binary,
            model,
            network,
        }
    }

    /// Build the `claude` invocation for a session. Kept separate from [`Backend::run`]
    /// so it can be inspected in tests and printed by `--dry-run` without executing.
    fn command(&self, session: &Session) -> Command {
        let mut servers = serde_json::Map::new();
        servers.insert(
            session.mcp.name.clone(),
            json!({ "command": session.mcp.command, "args": session.mcp.args }),
        );
        let mcp_config = json!({ "mcpServers": Value::Object(servers) });

        let mut allowed = vec![
            format!("mcp__{}", session.mcp.name),
            "Read(./**)".into(),
            "Glob(./**)".into(),
            "Grep(./**)".into(),
            "Edit(./**)".into(),
            "Write(./**)".into(),
        ];
        if self.network {
            allowed.push("WebFetch".into());
            allowed.push("WebSearch".into());
        }

        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&session.project_root);
        cmd.arg("-p")
            .arg(&session.prompt)
            .arg("--mcp-config")
            .arg(mcp_config.to_string())
            .arg("--strict-mcp-config")
            .arg("--allowedTools")
            .arg(allowed.join(","))
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        cmd
    }
}

impl Backend for ClaudeCode {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn run(&self, session: &Session, log: &mut dyn std::io::Write) -> Result<bool> {
        use std::io::{BufRead, BufReader};
        let mut child = self
            .command(session)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch `{}` — is Claude Code installed and on PATH?",
                    self.binary
                )
            })?;
        let stdout = child.stdout.take().expect("stdout was piped");
        for line in BufReader::new(stdout).lines() {
            let line = line.context("reading backend output")?;
            println!("{line}");
            writeln!(log, "{line}").context("writing work log")?;
            log.flush().context("flushing work log")?;
        }
        Ok(child.wait()?.success())
    }

    fn describe(&self, session: &Session) -> String {
        let cmd = self.command(session);
        let mut parts = vec![
            "cd".into(),
            shell_quote(&session.project_root.to_string_lossy()),
            "&&".into(),
            cmd.get_program().to_string_lossy().into_owned(),
        ];
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

    fn sample_session() -> Session {
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
    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
    fn backend(network: bool) -> ClaudeCode {
        ClaudeCode::new("claude".into(), None, network)
    }

    #[test]
    fn claude_command_grants_scoped_file_tools_but_no_shell() {
        let cmd = backend(false).command(&sample_session());
        assert_eq!(cmd.get_program().to_string_lossy(), "claude");
        assert_eq!(
            cmd.get_current_dir(),
            Some(std::path::Path::new("/wb/project"))
        );
        let args = args_of(&cmd);
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        let k = args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(args[k + 1], "stream-json");
        assert!(args.contains(&"--verbose".to_string()));
        let i = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(
            args[i + 1].split(',').collect::<Vec<_>>(),
            [
                "mcp__llaundry",
                "Read(./**)",
                "Glob(./**)",
                "Grep(./**)",
                "Edit(./**)",
                "Write(./**)"
            ]
        );
        assert!(!args
            .iter()
            .any(|a| a.contains("Bash") || a.contains("WebFetch")));
        assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));
        let j = args.iter().position(|a| a == "--mcp-config").unwrap();
        let cfg: Value = serde_json::from_str(&args[j + 1]).unwrap();
        assert_eq!(cfg["mcpServers"]["llaundry"]["command"], "llaundry-mcp");
        assert_eq!(cfg["mcpServers"]["llaundry"]["args"][1], "/wb/.llaundry");
    }

    #[test]
    fn web_tools_are_granted_only_with_network() {
        let args = args_of(&backend(true).command(&sample_session()));
        let i = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert!(args[i + 1].split(',').any(|t| t == "WebFetch"));
        assert!(args[i + 1].split(',').any(|t| t == "WebSearch"));
        assert!(!args[i + 1].contains("Bash"));
    }

    #[test]
    fn model_is_forwarded_only_when_set() {
        assert!(
            !args_of(&backend(false).command(&sample_session())).contains(&"--model".to_string())
        );
        let args = args_of(
            &ClaudeCode::new("claude".into(), Some("opus".into()), false)
                .command(&sample_session()),
        );
        let i = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[i + 1], "opus");
    }

    #[test]
    fn describe_is_a_copy_pasteable_command() {
        let d = backend(false).describe(&sample_session());
        assert!(d.starts_with("cd /wb/project && claude "));
        assert!(d.contains("mcp__llaundry"));
        assert!(d.contains("'do the work'"));
    }
}
