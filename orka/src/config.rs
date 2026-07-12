//! Optional per-store configuration, read from `<store>/config.toml`.
//!
//! It sets *defaults* for the work driver — which backend to run, which model
//! and executables to use — so a workbench can pin its choices once instead of
//! spelling them on every `orka` invocation. Everything is optional: a
//! missing file, or any missing field, means "use the built-in default". An
//! explicit `--flag` always wins over the file. The file is plain TOML,
//! versioned with the rest of the store:
//!
//! ```toml
//! [work]
//! backend = "openai-codex"  # default backend when --backend is not given
//! mcp-bin = "linka-mcp"  # the MCP server binary the model may use
//!
//! [work.claude-code]        # per-backend settings, keyed by backend name
//! model = "opus"            # model to request (backend default if unset)
//! bin   = "claude"          # the Claude Code executable
//!
//! [work.openai-codex]
//! model = "gpt-5-codex"     # model to request (backend default if unset)
//! bin   = "codex"           # the OpenAI Codex executable
//! ```

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// The config file's name inside the store root.
pub const CONFIG_FILE: &str = "config.toml";

/// The starter `config.toml` written by `linka init`: the built-in defaults
/// spelled out so a user can edit them in place. Removing a line (or the whole
/// file) falls back to the built-in default; an explicit `--flag` always wins.
pub const DEFAULT_CONFIG: &str = "\
# Defaults for the work driver (orka). Every line is optional:
# remove one to fall back to the built-in default. An explicit --flag
# always wins over this file.

[work]
backend = \"openai-codex\"  # default backend when --backend is not given
mcp-bin = \"linka-mcp\"  # the MCP server binary the model may use

[work.claude-code]         # per-backend settings, keyed by backend name
# model = \"opus\"           # model to request (backend default if unset)
bin = \"claude\"            # the Claude Code executable

# Optional Codex settings:
# [work.openai-codex]
# model = \"gpt-5-codex\"     # model to request (backend default if unset)
# bin = \"codex\"             # the OpenAI Codex executable
";

/// The parsed `config.toml`. Absent sections and fields default, so the
/// all-defaults value (a missing file) is simply `Config::default()`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub work: WorkConfig,
}

/// Defaults for `orka`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct WorkConfig {
    /// Default backend when `--backend` is not given (e.g. `"claude-code"`).
    pub backend: Option<String>,
    /// The `linka-mcp` executable the model is allowed to use.
    pub mcp_bin: Option<String>,
    /// Settings for the Claude Code backend.
    pub claude_code: ClaudeCodeConfig,
    /// Settings for the OpenAI Codex backend.
    pub openai_codex: OpenAiCodexConfig,
}

/// Defaults for the Claude Code backend.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct ClaudeCodeConfig {
    /// Model to request (backend default if unset).
    pub model: Option<String>,
    /// The Claude Code executable.
    pub bin: Option<String>,
}

/// Defaults for the OpenAI Codex backend.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct OpenAiCodexConfig {
    /// Model to request (backend default if unset).
    pub model: Option<String>,
    /// The OpenAI Codex executable.
    pub bin: Option<String>,
}

impl Config {
    /// Load `<store_root>/config.toml`, returning the all-defaults value if the
    /// file does not exist. A present-but-unreadable or malformed file is an
    /// error — a typo in the config should be surfaced, not silently ignored.
    pub fn load(store_root: &Path) -> Result<Self> {
        let path = store_root.join(CONFIG_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Write the starter [`DEFAULT_CONFIG`] to `<store_root>/config.toml` so
    /// users have a file to edit. Leaves an existing file untouched. Returns
    /// whether the file was created.
    pub fn write_default(store_root: &Path) -> Result<bool> {
        let path = store_root.join(CONFIG_FILE);
        if path.exists() {
            return Ok(false);
        }
        std::fs::write(&path, DEFAULT_CONFIG)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, contents: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), contents).unwrap();
    }

    #[test]
    fn missing_file_is_all_defaults() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config::load(&dir).unwrap();
        assert!(cfg.work.backend.is_none());
        assert!(cfg.work.mcp_bin.is_none());
        assert!(cfg.work.claude_code.model.is_none());
        assert!(cfg.work.claude_code.bin.is_none());
        assert!(cfg.work.openai_codex.model.is_none());
        assert!(cfg.work.openai_codex.bin.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_backend_and_per_backend_settings() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-parse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            r#"
                [work]
                backend = "claude-code"
                mcp-bin = "/opt/linka-mcp"

                [work.claude-code]
                model = "opus"
                bin = "claude"
            "#,
        );
        let cfg = Config::load(&dir).unwrap();
        assert_eq!(cfg.work.backend.as_deref(), Some("claude-code"));
        assert_eq!(cfg.work.mcp_bin.as_deref(), Some("/opt/linka-mcp"));
        assert_eq!(cfg.work.claude_code.model.as_deref(), Some("opus"));
        assert_eq!(cfg.work.claude_code.bin.as_deref(), Some("claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_config_leaves_the_rest_defaulted() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "[work.claude-code]\nmodel = \"sonnet\"\n");
        let cfg = Config::load(&dir).unwrap();
        assert_eq!(cfg.work.claude_code.model.as_deref(), Some("sonnet"));
        assert!(cfg.work.backend.is_none());
        assert!(cfg.work.claude_code.bin.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_openai_codex_settings() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            "[work.openai-codex]\nmodel = \"gpt-5-codex\"\nbin = \"/opt/codex\"\n",
        );
        let cfg = Config::load(&dir).unwrap();
        assert_eq!(cfg.work.openai_codex.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(cfg.work.openai_codex.bin.as_deref(), Some("/opt/codex"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_field_is_rejected() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-unknown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "[work]\nbakend = \"claude-code\"\n"); // typo
        assert!(Config::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_config_parses_and_matches_builtin_defaults() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Config::write_default(&dir).unwrap());
        let cfg = Config::load(&dir).unwrap();
        assert_eq!(cfg.work.backend.as_deref(), Some("openai-codex"));
        assert_eq!(cfg.work.mcp_bin.as_deref(), Some("linka-mcp"));
        assert!(cfg.work.claude_code.model.is_none());
        assert_eq!(cfg.work.claude_code.bin.as_deref(), Some("claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_default_leaves_an_existing_file_untouched() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "[work.claude-code]\nmodel = \"sonnet\"\n");
        assert!(!Config::write_default(&dir).unwrap());
        let cfg = Config::load(&dir).unwrap();
        assert_eq!(cfg.work.claude_code.model.as_deref(), Some("sonnet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("linka-cfg-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "this is not = = toml");
        assert!(Config::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
