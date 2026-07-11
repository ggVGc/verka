use anyhow::Result;
use clap::{Parser, Subcommand};
use llaundry_core::{ArtifactRef, ResultVersion};
use llaundry_review::{Candidate, CandidateStore, Decision, DecisionKind, FsCandidateStore};

#[derive(Parser)]
#[command(
    name = "llaundry-review",
    about = "Review immutable candidates independently of their work provider"
)]
struct Cli {
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: std::path::PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Add {
        id: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        result_metadata: String,
        #[arg(long)]
        result_notes: Option<String>,
        #[arg(long, default_value = "git-commit")]
        artifact_scheme: String,
        #[arg(long, default_value = "")]
        artifact_repository: String,
        #[arg(long)]
        artifact: String,
    },
    Show {
        id: String,
    },
    Accept {
        id: String,
        #[arg(long, default_value = "")]
        notes: String,
    },
    Reject {
        id: String,
        #[arg(long)]
        notes: String,
        #[arg(long)]
        suggestion: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = FsCandidateStore::new(cli.store);
    match cli.command {
        Command::Add {
            id,
            subject,
            result_metadata,
            result_notes,
            artifact_scheme,
            artifact_repository,
            artifact,
        } => {
            store.create_candidate(&Candidate {
                id,
                subject,
                result: ResultVersion {
                    metadata: result_metadata,
                    notes: result_notes,
                },
                artifact: ArtifactRef {
                    scheme: artifact_scheme,
                    repository: artifact_repository,
                    id: artifact,
                },
            })?;
        }
        Command::Show { id } => {
            let candidate = store.candidate(&id)?;
            let decision = store.decision(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema": 1, "candidate": candidate, "decision": decision
                }))?
            );
        }
        Command::Accept { id, notes } => store.record_decision(&Decision {
            candidate: id,
            kind: DecisionKind::Accepted,
            notes,
            suggestion: None,
        })?,
        Command::Reject {
            id,
            notes,
            suggestion,
        } => {
            let candidate = store.candidate(&id)?;
            store.record_decision(&Decision {
                candidate: id,
                kind: DecisionKind::Rejected,
                notes,
                suggestion: suggestion.map(|id| ArtifactRef {
                    scheme: candidate.artifact.scheme,
                    repository: candidate.artifact.repository,
                    id,
                }),
            })?;
        }
    }
    Ok(())
}
