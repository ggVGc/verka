use anyhow::Result;
use clap::Parser;
use linka_core::Envelope;
use orka::{AttemptStore, FsAttemptStore, GitWorkspaceManager, Request, WorkspaceManager};
use std::io::BufRead;

#[derive(Parser)]
#[command(
    name = "orka-exec",
    about = "JSON-lines execution/worktree service for any work provider"
)]
struct Cli {
    #[arg(long)]
    store: std::path::PathBuf,
    #[arg(long)]
    repository: std::path::PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = FsAttemptStore::new(cli.store);
    let workspaces = GitWorkspaceManager::new(cli.repository);
    for line in std::io::BufReader::new(std::io::stdin()).lines() {
        let envelope: Envelope<Request> = serde_json::from_str(&line?)?;
        let response = match envelope.validate() {
            Err(error) => serde_json::json!({ "status": "error", "value": error }),
            Ok(Request::Get { id }) => match store.read(&id) {
                Ok(attempt) => serde_json::json!({ "status": "attempt", "value": attempt }),
                Err(error) => {
                    serde_json::json!({ "status": "error", "value": format!("{error:#}") })
                }
            },
            Ok(Request::Prepare { attempt }) => match store
                .create(&attempt)
                .and_then(|()| workspaces.prepare(&attempt))
            {
                Ok(workspace) => serde_json::json!({ "status": "workspace", "value": workspace }),
                Err(error) => {
                    serde_json::json!({ "status": "error", "value": format!("{error:#}") })
                }
            },
            Ok(Request::Finish { id, final_record }) => match store.finish(&id, &final_record) {
                Ok(()) => serde_json::json!({ "status": "ok" }),
                Err(error) => {
                    serde_json::json!({ "status": "error", "value": format!("{error:#}") })
                }
            },
        };
        println!("{}", serde_json::to_string(&Envelope::new(response))?);
    }
    Ok(())
}
