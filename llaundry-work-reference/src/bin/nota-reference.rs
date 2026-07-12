use anyhow::Result;
use clap::{Parser, Subcommand};
use linka_core::{ArtifactRef, ResultVersion};
use linka_review::{
    publish_exact, Candidate, CandidateStore, Decision, DecisionKind, FsCandidateStore,
    GitPublisher,
};

#[derive(Parser)]
#[command(
    name = "nota",
    about = "Review immutable candidates independently of their work provider"
)]
struct Cli {
    #[arg(long, env = "LINKA_DIR", default_value = ".linka")]
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
        attempt: String,
        #[arg(long)]
        branch: String,
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
    Publish {
        id: String,
        #[arg(long)]
        repository: std::path::PathBuf,
        #[arg(long, default_value = "main")]
        target: String,
        #[arg(long)]
        expected: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = FsCandidateStore::new(cli.store);
    match cli.command {
        Command::Add {
            id,
            subject,
            attempt,
            branch,
            result_metadata,
            result_notes,
            artifact_scheme,
            artifact_repository,
            artifact,
        } => {
            store.create_candidate(&Candidate {
                id,
                subject,
                attempt,
                branch,
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
        Command::Publish {
            id,
            repository,
            target,
            expected,
        } => {
            let candidate = store.candidate(&id)?;
            let decision = store
                .decision(&id)?
                .ok_or_else(|| anyhow::anyhow!("candidate has no decision"))?;
            if decision.kind != DecisionKind::Accepted {
                anyhow::bail!("only an accepted candidate may be published");
            }
            let expected = ArtifactRef {
                scheme: "git-commit".into(),
                repository: repository.to_string_lossy().into_owned(),
                id: expected,
            };
            publish_exact(
                &GitPublisher::new(repository),
                &candidate,
                &target,
                &expected,
            )
            .map_err(|error| match error {
                linka_review::PublishError::NotFastForward => {
                    anyhow::anyhow!("candidate cannot fast-forward `{target}`")
                }
                linka_review::PublishError::Backend(error) => error,
            })?;
        }
    }
    Ok(())
}
