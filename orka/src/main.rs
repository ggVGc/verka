//! The `orka` CLI: orchestrate isolated agent attempts for work in a Linka
//! store.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use linka::{NodeId, Store};
use orka::attempt::{AttemptId, FsAttemptStore, SealedState};
use orka::candidate::Candidates;
use orka::config::{Config, CONFIG_FILE};
use orka::engine::{Engine, RunProgress, RunReport};
use orka::linka_work::LinkaWork;
use orka::workspace::GitWorkspaces;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "orka",
    about = "Orchestrate isolated agent attempts for work in a Linka store: snapshot a ready node, run an agent, submit a version-checked result"
)]
struct Cli {
    /// Workbench root (holds .linka/, project/, orka.toml). Defaults to the
    /// nearest ancestor of the current directory containing .linka/.
    #[arg(long, global = true)]
    workbench: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a default orka.toml using Driva's codex-exec template.
    Init,
    /// Run one attempt: the given node, or the first ready one.
    Run {
        /// Node id to run; omitted, the first ready machine-workable node.
        node: Option<String>,
    },
    /// List ready work as the orchestrator sees it.
    Ready,
    /// List recorded attempts.
    Attempts,
    /// Show one attempt's durable record.
    Show { attempt: String },
    /// List project candidates and the Linka nodes that produced them.
    Candidates,
    /// Show a Linka candidate (candidate id or producing attempt id).
    Candidate { candidate: String },
    /// Accept an exact Linka candidate for its recorded target branch.
    Accept {
        candidate: String,
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// Reject a Linka candidate and make its source node retryable.
    Reject {
        candidate: String,
        #[arg(long)]
        notes: String,
    },
    /// Publish an accepted candidate by recoverable fast-forward.
    Publish { candidate: String },
    /// Classify unfinished attempts and finish what can be finished.
    Recover,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

struct Workbench {
    root: PathBuf,
}

impl Workbench {
    fn locate(given: Option<PathBuf>) -> Result<Self> {
        if let Some(root) = given {
            if !root.join(".linka").is_dir() {
                bail!("no .linka store under {}", root.display());
            }
            return Ok(Self { root });
        }
        let mut dir = std::env::current_dir()?;
        loop {
            if dir.join(".linka").is_dir() {
                return Ok(Self { root: dir });
            }
            if !dir.pop() {
                bail!("no workbench found: no ancestor directory contains .linka/");
            }
        }
    }

    /// Open the Linka store Orka orchestrates.
    fn linka_store(&self) -> Result<Store> {
        Store::open(self.root.join(".linka"))
    }

    fn attempts(&self) -> FsAttemptStore {
        FsAttemptStore::new(self.root.join(".orka"))
    }

    fn workspaces(&self, store: &Store) -> GitWorkspaces {
        GitWorkspaces::new(store.project_root(), self.root.join(".orka/worktrees"))
    }

    fn config(&self) -> Result<Config> {
        let path = self.root.join(CONFIG_FILE);
        Config::load(&path).with_context(|| {
            format!(
                "orka needs {} (see orka/DESIGN.md for an example)",
                path.display()
            )
        })
    }
}

/// Parse a CLI node argument as a Linka node id, reporting Linka's validation
/// error at the command boundary.
fn parse_node(arg: String) -> Result<NodeId> {
    arg.parse()
        .map_err(|e| anyhow::anyhow!("invalid node id: {e}"))
}

fn run(cli: Cli) -> Result<()> {
    let workbench = Workbench::locate(cli.workbench)?;
    match cli.command {
        Command::Init => {
            let path = workbench.root.join(CONFIG_FILE);
            Config::init(&path)?;
            println!("created {}", path.display());
        }
        Command::Run { node } => {
            let config = workbench.config()?;
            let store = workbench.linka_store()?;
            let executor = config.executor()?;
            let workspaces = workbench.workspaces(&store);
            let attempts = workbench.attempts();
            let engine = Engine {
                linka: LinkaWork::new(&store),
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy()?,
            };
            let mut progress = print_progress;
            let report = match node {
                Some(arg) => Some(engine.run_node_with_progress(&parse_node(arg)?, &mut progress)?),
                None => engine.run_next_with_progress(&mut progress)?,
            };
            match report {
                None => println!("nothing ready"),
                Some(report) => print_run(&report),
            }
        }
        Command::Ready => {
            let store = workbench.linka_store()?;
            let linka = LinkaWork::new(&store);
            let ready = linka.ready_for_machine()?;
            if ready.is_empty() {
                println!("nothing ready");
            }
            for item in ready {
                println!("{}  {}", item.node, item.title);
            }
        }
        Command::Attempts => {
            let attempts = workbench.attempts();
            for id in attempts.list()? {
                let snapshot = attempts.load(&id)?;
                println!(
                    "{}  {}  {:?}",
                    id,
                    snapshot.record.input.node(),
                    snapshot.phase()
                );
            }
        }
        Command::Show { attempt } => {
            let attempts = workbench.attempts();
            let id = AttemptId(attempt);
            let snapshot = attempts.load(&id)?;
            let input = &snapshot.record.input;
            println!("attempt   {id}");
            println!("node      {}", input.node());
            println!("phase     {:?}", snapshot.phase());
            println!("input     {}", input.input_commit());
            if let Some(ws) = &snapshot.workspace {
                println!("workspace {} (branch {})", ws.path.display(), ws.branch);
                println!("candidate {}", ws.branch);
            }
            if let Some(evidence) = &snapshot.evidence {
                println!(
                    "exit      {} via {} backend",
                    evidence.exit_code, evidence.backend
                );
            }
            if let Some(seal) = &snapshot.seal {
                println!("sealed    {}", seal_line(&seal.state));
            }
            let transcript = attempts.transcript_path(&id);
            if transcript.exists() {
                println!("transcript {}", transcript.display());
            }
            let store = workbench.linka_store()?;
            if let Ok(candidate) = Candidates::new(&store).get(&id.0) {
                println!("linka     {} ({})", candidate.id, candidate.status());
            }
        }
        Command::Candidates => {
            let store = workbench.linka_store()?;
            let candidates = Candidates::new(&store).list()?;
            if candidates.is_empty() {
                println!("no project candidates");
            }
            for candidate in candidates {
                println!(
                    "{}  node {}  {}  {} -> {}{}",
                    candidate.id,
                    candidate.node,
                    candidate.status(),
                    candidate.branch,
                    candidate.target,
                    candidate
                        .attempt
                        .as_ref()
                        .map(|attempt| format!("  attempt {attempt}"))
                        .unwrap_or_default()
                );
            }
        }
        Command::Candidate { candidate } => {
            let store = workbench.linka_store()?;
            let candidates = Candidates::new(&store);
            let candidate = candidates.get(&candidate)?;
            println!("candidate {}", candidate.id);
            println!("node      {}", candidate.node);
            println!("status    {}", candidate.status());
            println!("branch    {}", candidate.branch);
            println!("target    {}", candidate.target);
            println!("input     {}", candidate.input_commit);
            println!("head      {}", candidate.head_commit);
            if let Some(attempt) = &candidate.attempt {
                println!("attempt   {attempt}");
            }
            let patch = candidates.patch(&candidate.id.0)?;
            if patch.is_empty() {
                println!("\n(no diff)");
            } else {
                println!("\n{patch}");
            }
        }
        Command::Accept { candidate, notes } => {
            let store = workbench.linka_store()?;
            let accepted = Candidates::new(&store).accept(&candidate, notes)?;
            println!("accepted {} for {}", accepted.id, accepted.target);
        }
        Command::Reject { candidate, notes } => {
            let store = workbench.linka_store()?;
            let rejected = Candidates::new(&store).reject(&candidate, notes)?;
            println!("rejected {}", rejected.id);
        }
        Command::Publish { candidate } => {
            let store = workbench.linka_store()?;
            let published = Candidates::new(&store).publish(&candidate)?;
            println!("published {} at {}", published.id, published.head_commit);
        }
        Command::Recover => {
            let config = workbench.config()?;
            let store = workbench.linka_store()?;
            let executor = config.executor()?;
            let workspaces = workbench.workspaces(&store);
            let attempts = workbench.attempts();
            let engine = Engine {
                linka: LinkaWork::new(&store),
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy()?,
            };
            let reports = engine.recover()?;
            for candidate in Candidates::new(&store).recover_publications()? {
                println!("{candidate}  recovered publication");
            }
            if reports.is_empty() {
                println!("no attempts recorded");
            }
            for report in reports {
                println!("{}  {}  {}", report.attempt, report.node, report.action);
            }
        }
    }
    Ok(())
}

fn print_run(report: &RunReport) {
    println!("attempt {}  node {}", report.attempt, report.node);
    println!("exit    {}", report.exit_code);
    println!("sealed  {}", seal_line(&report.sealed));
    if let Some(candidate) = &report.candidate {
        println!(
            "candidate {}  (view: `orka candidate {}`; accept: `orka accept {}`; publish: `orka publish {}`)",
            candidate, candidate, candidate, candidate
        );
    }
    if report.backend_failed {
        println!("warning: the agent command exited nonzero; its outcome was still handled");
    }
    println!("cleanup {:?}", report.cleanup);
}

fn print_progress(progress: &RunProgress) {
    match progress {
        RunProgress::Selected { node } => eprintln!("[orka] selected node {node}"),
        RunProgress::AttemptCreated { attempt } => {
            eprintln!("[orka] created {attempt}")
        }
        RunProgress::WorkspacePrepared { attempt } => {
            eprintln!("[orka] {attempt}: workspace prepared")
        }
        RunProgress::ExecutionStarted {
            attempt,
            transcript,
        } => eprintln!(
            "[orka] {attempt}: agent running (transcript: {})",
            transcript.display()
        ),
        RunProgress::ExecutionFinished { attempt, exit_code } => {
            eprintln!("[orka] {attempt}: agent exited with code {exit_code}")
        }
        RunProgress::Sealed { attempt, state } => {
            eprintln!("[orka] {attempt}: {}", seal_line(state))
        }
    }
}

fn seal_line(state: &SealedState) -> String {
    match state {
        SealedState::Submitted { output_commit } => match output_commit {
            Some(commit) => format!("submitted (output commit {commit})"),
            None => "submitted (no project output)".into(),
        },
        SealedState::StaleAtSubmit { conflicts } => {
            format!("stale at submit: {conflicts:?}")
        }
        SealedState::FailureRecorded => "failure recorded".into(),
        SealedState::Interrupted { reason } => format!("interrupted: {reason}"),
        SealedState::ContractViolation { reason } => format!("contract violation: {reason}"),
    }
}
