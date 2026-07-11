use anyhow::{Context, Result};
use clap::Parser;
use llaundry_core::{handle_request, Envelope, FsGraphStore, GraphRequest};
use std::io::BufRead;

#[derive(Parser)]
#[command(
    name = "llaundry-core",
    about = "Versioned JSON-lines interface to a llaundry work graph"
)]
struct Cli {
    #[arg(long, default_value = ".llaundry")]
    store: std::path::PathBuf,
}

fn main() -> Result<()> {
    let store = FsGraphStore::open(Cli::parse().store)?;
    for line in std::io::BufReader::new(std::io::stdin()).lines() {
        let line = line?;
        let request: Envelope<GraphRequest> =
            serde_json::from_str(&line).context("parsing request envelope")?;
        let response = request
            .validate()
            .map(|request| handle_request(&store, request))
            .unwrap_or_else(llaundry_core::GraphResponse::Error);
        println!("{}", serde_json::to_string(&Envelope::new(response))?);
    }
    Ok(())
}
