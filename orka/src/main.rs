//! The `orka` CLI: run graph work through isolated agent attempts.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use orka::attempt::{AttemptId, FsAttemptStore, SealedState};
use orka::config::{Config, CONFIG_FILE};
use orka::engine::{Engine, RunReport};
use orka::linka_graph::LinkaWorkGraph;
use orka::ports::NodeId;
use orka::workspace::GitWorkspaces;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "orka",
    about = "Orchestrate graph work: freeze a ready node, run an isolated agent attempt, submit a version-checked result"
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

    fn graph(&self) -> Result<LinkaWorkGraph> {
        LinkaWorkGraph::open(self.root.join(".linka"))
    }

    fn attempts(&self) -> FsAttemptStore {
        FsAttemptStore::new(self.root.join(".orka"))
    }

    fn workspaces(&self, graph: &LinkaWorkGraph) -> GitWorkspaces {
        GitWorkspaces::new(graph.project_root(), self.root.join(".orka/worktrees"))
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

fn run(cli: Cli) -> Result<()> {
    let workbench = Workbench::locate(cli.workbench)?;
    match cli.command {
        Command::Run { node } => {
            let config = workbench.config()?;
            let graph = workbench.graph()?;
            let executor = config.executor()?;
            let workspaces = workbench.workspaces(&graph);
            let attempts = workbench.attempts();
            let engine = Engine {
                graph: &graph,
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy(),
            };
            let report = match node {
                Some(id) => Some(engine.run_node(&NodeId(id))?),
                None => engine.run_next()?,
            };
            match report {
                None => println!("nothing ready"),
                Some(report) => print_run(&report),
            }
        }
        Command::Ready => {
            let graph = workbench.graph()?;
            let ready = orka::ports::WorkGraph::select_ready(&graph)?;
            if ready.is_empty() {
                println!("nothing ready");
            }
            for item in ready {
                println!("{}  {}", item.id, item.title);
            }
        }
        Command::Attempts => {
            let attempts = workbench.attempts();
            for id in attempts.list()? {
                let snapshot = attempts.load(&id)?;
                println!(
                    "{}  {}  {:?}",
                    id,
                    snapshot.record.frozen.node,
                    snapshot.phase()
                );
            }
        }
        Command::Show { attempt } => {
            let attempts = workbench.attempts();
            let id = AttemptId(attempt);
            let snapshot = attempts.load(&id)?;
            println!("attempt   {id}");
            println!("node      {}", snapshot.record.frozen.node);
            println!("phase     {:?}", snapshot.phase());
            println!("input     {}", snapshot.record.frozen.input_commit);
            if let Some(ws) = &snapshot.workspace {
                println!("workspace {} (branch {})", ws.path.display(), ws.branch);
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
        }
        Command::Recover => {
            let config = workbench.config()?;
            let graph = workbench.graph()?;
            let executor = config.executor()?;
            let workspaces = workbench.workspaces(&graph);
            let attempts = workbench.attempts();
            let engine = Engine {
                graph: &graph,
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy(),
            };
            let reports = engine.recover()?;
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
    if report.backend_failed {
        println!("warning: the agent command exited nonzero; its outcome was still handled");
    }
    println!("cleanup {:?}", report.cleanup);
}

fn seal_line(state: &SealedState) -> String {
    match state {
        SealedState::Submitted { output_commit } => match output_commit {
            Some(commit) => format!("submitted (output commit {commit})"),
            None => "submitted (no project output)".into(),
        },
        SealedState::StaleAtSubmit { reasons } => {
            format!("stale at submit:\n  {}", reasons.join("\n  "))
        }
        SealedState::FailureRecorded => "failure recorded".into(),
        SealedState::Interrupted { reason } => format!("interrupted: {reason}"),
        SealedState::ContractViolation { reason } => format!("contract violation: {reason}"),
    }
}
