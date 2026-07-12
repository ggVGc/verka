//! The agent outcome contract.
//!
//! The agent declares its outcome by writing `outcome.toml` into the
//! attempt's exchange directory (mounted writable in the isolated
//! environment, its path published as `ORKA_OUTCOME`):
//!
//! ```toml
//! outcome = "succeeded"        # or "failed"
//! outputs = ["src/thing.rs"]   # workspace-relative declared outputs
//! message = "add the thing"    # optional output commit message
//! notes = "what was done and why"
//! ```
//!
//! Interpreting the declaration is Orka's own concern: [`decide`] combines it
//! with the harness-observed exit code into an Orka [`AgentOutcome`], per the
//! failure matrix. A declaration is honored whatever the exit status (a nonzero
//! exit rides along as reportable backend trouble); no declaration plus exit
//! zero is a contract violation; no declaration plus a nonzero exit is an
//! interrupted attempt. The declaration is what the agent *claims* it did;
//! whether it completes the node is still Linka's version-checked call, made
//! only by trusted Orka code translating an [`AgentOutcome`] into a submission.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const OUTCOME_FILE: &str = "outcome.toml";
pub const PROMPT_FILE: &str = "prompt.md";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredOutcome {
    pub outcome: DeclaredKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
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
/// outcome, not a graph mutation: it carries the raw declared output strings,
/// which trusted Orka code later validates into `linka::ProjectPath` and
/// submits. It deliberately holds no Linka snapshot or version token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentOutcome {
    Succeeded {
        /// Workspace-relative paths, exactly as the agent declared them.
        outputs: Vec<String>,
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
        Some(declared) => match declared.outcome {
            DeclaredKind::Succeeded => Decision::Submit {
                outcome: AgentOutcome::Succeeded {
                    outputs: declared.outputs,
                    message: declared.message,
                    notes: declared.notes,
                },
                backend_failed: exit_code != 0,
            },
            // A failure declaration asserts no output provenance; claiming
            // outputs alongside it is a contradiction, not a submittable result.
            DeclaredKind::Failed if !declared.outputs.is_empty() => Decision::ContractViolation {
                reason: "declared failure also claimed outputs".into(),
            },
            DeclaredKind::Failed => Decision::Submit {
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
            "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\nnotes = \"did it\"\n",
        )
        .unwrap();
        let declared = read_declared(&dir).unwrap().unwrap();
        assert_eq!(declared.outcome, DeclaredKind::Succeeded);
        assert_eq!(declared.outputs, vec!["out.txt"]);

        std::fs::write(dir.join(OUTCOME_FILE), "outcome = \"maybe\"").unwrap();
        assert!(read_declared(&dir).is_err(), "garbage is an error");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_failure_matrix_decides_every_combination() {
        let succeeded = DeclaredOutcome {
            outcome: DeclaredKind::Succeeded,
            outputs: vec!["a".into()],
            message: None,
            notes: "n".into(),
        };
        // Declared success plus exit zero: submit.
        assert_eq!(
            decide(Some(succeeded.clone()), 0),
            Decision::Submit {
                outcome: AgentOutcome::Succeeded {
                    outputs: vec!["a".into()],
                    message: None,
                    notes: "n".into(),
                },
                backend_failed: false,
            }
        );
        // Declared success plus nonzero exit: still submit, but report.
        assert!(matches!(
            decide(Some(succeeded), 1),
            Decision::Submit {
                backend_failed: true,
                ..
            }
        ));
        // Declared failure is failure evidence.
        assert!(matches!(
            decide(
                Some(DeclaredOutcome {
                    outcome: DeclaredKind::Failed,
                    outputs: vec![],
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
        // A failure declaration that also claims outputs is a contradiction.
        assert!(matches!(
            decide(
                Some(DeclaredOutcome {
                    outcome: DeclaredKind::Failed,
                    outputs: vec!["a".into()],
                    message: None,
                    notes: "why".into(),
                }),
                0
            ),
            Decision::ContractViolation { .. }
        ));
        // No declaration: exit zero violates the contract; nonzero interrupts.
        assert!(matches!(
            decide(None, 0),
            Decision::ContractViolation { .. }
        ));
        assert!(matches!(decide(None, 137), Decision::Interrupted { .. }));
    }
}
