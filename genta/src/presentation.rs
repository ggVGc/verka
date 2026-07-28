//! Provider-owned presentation of decoded events.
//!
//! Decoding preserves what the provider reported. Presenters may interpret
//! that provider-specific representation for the human-facing pretty view;
//! raw mode always falls back to the stable event vocabulary unchanged.

use crate::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};
use serde_json::Value;

trait EventPresenter: Sync {
    fn pretty_summary(&self, _event: &AgentEvent) -> Option<String> {
        None
    }

    fn pretty_detail(&self, _event: &AgentEvent) -> Option<Vec<DetailBlock>> {
        None
    }
}

struct CodexPresenter;
struct ClaudePresenter;

static CODEX: CodexPresenter = CodexPresenter;
static CLAUDE: ClaudePresenter = ClaudePresenter;

impl Protocol {
    fn presenter(self) -> &'static dyn EventPresenter {
        match self {
            Protocol::CodexJsonl | Protocol::CodexAppServer => &CODEX,
            Protocol::ClaudeJsonl => &CLAUDE,
        }
    }

    /// A collapsed-line summary under this provider's presentation rules.
    pub fn presented_summary(self, event: &AgentEvent, mode: PresentationMode) -> String {
        if mode == PresentationMode::Pretty {
            if let Some(summary) = self.presenter().pretty_summary(event) {
                return crate::event::truncate_summary(&summary);
            }
        }
        event.summary()
    }

    /// Structured detail under this provider's presentation rules.
    pub fn presented_detail(self, event: &AgentEvent, mode: PresentationMode) -> Vec<DetailBlock> {
        if mode == PresentationMode::Pretty {
            if let Some(detail) = self.presenter().pretty_detail(event) {
                return detail;
            }
        }
        event.presented_detail_default(mode)
    }
}

impl EventPresenter for ClaudePresenter {
    fn pretty_summary(&self, event: &AgentEvent) -> Option<String> {
        claude_bash_command(event)
    }

    fn pretty_detail(&self, event: &AgentEvent) -> Option<Vec<DetailBlock>> {
        command_detail(claude_bash_command(event)?, event)
    }
}

impl EventPresenter for CodexPresenter {
    fn pretty_summary(&self, event: &AgentEvent) -> Option<String> {
        codex_bash_command(event)
    }

    fn pretty_detail(&self, event: &AgentEvent) -> Option<Vec<DetailBlock>> {
        command_detail(codex_bash_command(event)?, event)
    }
}

fn claude_bash_command(event: &AgentEvent) -> Option<String> {
    let (name, detail) = match event {
        AgentEvent::ToolStarted { name, detail, .. }
        | AgentEvent::ToolCompleted { name, detail, .. } => (name, detail),
        _ => return None,
    };
    if name != "Bash" {
        return None;
    }
    serde_json::from_str::<Value>(detail)
        .ok()?
        .get("command")?
        .as_str()
        .map(str::to_owned)
}

fn codex_bash_command(event: &AgentEvent) -> Option<String> {
    let command = match event {
        AgentEvent::CommandStarted { command } | AgentEvent::CommandCompleted { command, .. } => {
            command
        }
        _ => return None,
    };
    let (executable, rest) = command.split_once(char::is_whitespace)?;
    let shell = executable.rsplit('/').next()?;
    if !matches!(shell, "bash" | "sh" | "zsh") {
        return None;
    }
    let (flags, command) = rest.trim_start().split_once(char::is_whitespace)?;
    if !(flags == "-c" || (flags.starts_with('-') && flags.contains('c'))) {
        return None;
    }
    parse_shell_word(command.trim_start())
}

/// Parse the single shell word used as the argument to `bash -c`. Codex quotes
/// that argument using ordinary POSIX single/double quotes; interpreting just
/// one word keeps this presentation code small and never executes the text.
fn parse_shell_word(input: &str) -> Option<String> {
    #[derive(Clone, Copy)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut output = String::new();
    let mut chars = input.char_indices();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            output.push(ch);
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Quote::None, '\\') | (Quote::Double, '\\') => escaped = true,
            (Quote::None, '\'') => quote = Quote::Single,
            (Quote::None, '"') => quote = Quote::Double,
            (Quote::Single, '\'') | (Quote::Double, '"') => quote = Quote::None,
            (Quote::None, ch) if ch.is_whitespace() => {
                return input[index..].trim().is_empty().then_some(output);
            }
            _ => output.push(ch),
        }
    }
    if escaped || !matches!(quote, Quote::None) {
        None
    } else {
        Some(output)
    }
}

fn command_detail(command: String, event: &AgentEvent) -> Option<Vec<DetailBlock>> {
    let mut blocks = vec![DetailBlock::Code {
        language: Some("bash".into()),
        text: command,
    }];
    let output = match event {
        AgentEvent::ToolCompleted { output, .. } | AgentEvent::CommandCompleted { output, .. } => {
            output
        }
        _ => return Some(blocks),
    };
    if !output.is_empty() {
        blocks.push(DetailBlock::Code {
            language: None,
            text: output.clone(),
        });
    }
    Some(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_unwraps_a_bash_login_command_but_keeps_output() {
        let event = AgentEvent::CommandCompleted {
            command: "/usr/bin/bash -lc 'cargo test --all'".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "ok\n".into(),
        };
        assert_eq!(
            Protocol::CodexAppServer.presented_summary(&event, PresentationMode::Pretty),
            "cargo test --all"
        );
        assert_eq!(
            Protocol::CodexAppServer.presented_detail(&event, PresentationMode::Pretty),
            vec![
                DetailBlock::Code {
                    language: Some("bash".into()),
                    text: "cargo test --all".into(),
                },
                DetailBlock::Code {
                    language: None,
                    text: "ok\n".into(),
                },
            ]
        );
        assert!(matches!(
            Protocol::CodexAppServer
                .presented_detail(&event, PresentationMode::Raw)
                .first(),
            Some(DetailBlock::Text(text)) if text.contains("/usr/bin/bash -lc")
        ));
    }

    #[test]
    fn codex_leaves_non_shell_commands_alone() {
        let event = AgentEvent::CommandStarted {
            command: "cargo test".into(),
        };
        assert_eq!(
            Protocol::CodexJsonl.presented_summary(&event, PresentationMode::Pretty),
            "cargo test (running)"
        );
    }

    #[test]
    fn codex_unwraps_shell_escaped_quotes() {
        let event = AgentEvent::CommandStarted {
            command: r#"/bin/bash -lc 'printf '"'"'%s\n'"'"' hello'"#.into(),
        };
        assert_eq!(
            Protocol::CodexJsonl.presented_summary(&event, PresentationMode::Pretty),
            "printf '%s\\n' hello"
        );
    }
}
