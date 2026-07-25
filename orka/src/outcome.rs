//! The agent outcome contract.
//!
//! The agent declares its outcome by writing `outcome.toml` into the
//! attempt's exchange directory (mounted writable in the isolated
//! environment, its path published as `ORKA_OUTCOME`):
//!
//! ```toml
//! outcome = "succeeded"        # or "failed"
//! message = "add the thing"    # optional output commit message
//! notes = "what was done and why"
//! ```
//!
//! The set of files an attempt produced is not part of this declaration.
//! The agent is required to commit all its work with Git before declaring
//! success; Orka captures the diff between the frozen input commit and the
//! committed worktree. A declared success that leaves uncommitted changes has
//! not captured its output and is rejected as a contract violation.
//!
//! Interpreting the declaration is Orka's own concern: [`decide`] combines it
//! with the harness-observed exit code into an Orka [`AgentOutcome`], per the
//! failure matrix. A success declaration is honored only when the command exits
//! zero; a nonzero exit makes the attempt interrupted and cannot complete the
//! node. A declared failure remains usable failure evidence regardless of exit
//! status. No declaration plus exit zero is a contract violation; no declaration
//! plus a nonzero exit is an interrupted attempt. The declaration is what the
//! agent *claims* it did; whether it completes the node is still Linka's
//! version-checked call, made only by trusted Orka code translating an
//! [`AgentOutcome`] into a submission.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const OUTCOME_FILE: &str = "outcome.toml";
pub const PROMPT_FILE: &str = "prompt.md";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredOutcome {
    pub outcome: DeclaredKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredKind {
    Succeeded,
    Failed,
}

/// Orka's interpretation of what the agent said it did. This is an execution
/// outcome, not a graph mutation. The produced file set is not carried here:
/// trusted Orka code discovers it from the workspace at submission time. This
/// deliberately holds no Linka snapshot or version token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentOutcome {
    Succeeded {
        message: Option<String>,
        notes: String,
    },
    Failed {
        notes: String,
    },
}

/// Read the agent's declared outcome from the exchange directory. Absence is
/// an answer (`None`); an unreadable or unparsable declaration is an error.
pub fn read_declared(io_dir: &Path) -> Result<Option<DeclaredOutcome>> {
    let path = io_dir.join(OUTCOME_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let declared = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(declared))
}

/// What the attempt's evidence says should happen next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A declared outcome to submit to Linka. `backend_failed` notes a nonzero
    /// exit that rode along with the declaration.
    Submit {
        outcome: AgentOutcome,
        backend_failed: bool,
    },
    /// The command exited zero without a usable declaration; nothing to submit.
    ContractViolation { reason: String },
    /// The command ended without a declaration; nothing to submit.
    Interrupted { reason: String },
}

pub fn decide(declared: Option<DeclaredOutcome>, exit_code: i32) -> Decision {
    match declared {
        Some(declared) => match (declared.outcome, exit_code) {
            (DeclaredKind::Succeeded, 0) => Decision::Submit {
                outcome: AgentOutcome::Succeeded {
                    message: declared.message,
                    notes: declared.notes,
                },
                backend_failed: false,
            },
            (DeclaredKind::Succeeded, exit_code) => Decision::Interrupted {
                reason: format!("command exited {exit_code} after declaring success"),
            },
            (DeclaredKind::Failed, _) => Decision::Submit {
                outcome: AgentOutcome::Failed {
                    notes: if declared.notes.is_empty() {
                        "agent declared failure without notes".into()
                    } else {
                        declared.notes
                    },
                },
                backend_failed: exit_code != 0,
            },
        },
        None if exit_code == 0 => Decision::ContractViolation {
            reason: "command exited zero without declaring an outcome".into(),
        },
        None => Decision::Interrupted {
            reason: format!("command exited {exit_code} without declaring an outcome"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_round_trip_and_absence_is_an_answer() {
        let dir = std::env::temp_dir().join(format!("orka-outcome-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_declared(&dir).unwrap(), None);

        std::fs::write(
            dir.join(OUTCOME_FILE),
            "outcome = \"succeeded\"\nnotes = \"did it\"\n",
        )
        .unwrap();
        let declared = read_declared(&dir).unwrap().unwrap();
        assert_eq!(declared.outcome, DeclaredKind::Succeeded);
        assert_eq!(declared.notes, "did it");

        std::fs::write(
            dir.join(OUTCOME_FILE),
            "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\n",
        )
        .unwrap();
        assert!(read_declared(&dir).is_err(), "unknown fields are errors");

        std::fs::write(dir.join(OUTCOME_FILE), "outcome = \"maybe\"").unwrap();
        assert!(read_declared(&dir).is_err(), "garbage is an error");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_failure_matrix_decides_every_combination() {
        let succeeded = DeclaredOutcome {
            outcome: DeclaredKind::Succeeded,
            message: None,
            notes: "n".into(),
        };
        // Declared success plus exit zero: submit.
        assert_eq!(
            decide(Some(succeeded.clone()), 0),
            Decision::Submit {
                outcome: AgentOutcome::Succeeded {
                    message: None,
                    notes: "n".into(),
                },
                backend_failed: false,
            }
        );
        // Declared success plus nonzero exit cannot complete the node.
        assert!(matches!(
            decide(Some(succeeded), 1),
            Decision::Interrupted { reason }
                if reason == "command exited 1 after declaring success"
        ));
        // Declared failure is failure evidence.
        assert!(matches!(
            decide(
                Some(DeclaredOutcome {
                    outcome: DeclaredKind::Failed,
                    message: None,
                    notes: "why".into(),
                }),
                0
            ),
            Decision::Submit {
                outcome: AgentOutcome::Failed { .. },
                ..
            }
        ));
        // No declaration: exit zero violates the contract; nonzero interrupts.
        assert!(matches!(
            decide(None, 0),
            Decision::ContractViolation { .. }
        ));
        assert!(matches!(decide(None, 137), Decision::Interrupted { .. }));
    }
}
